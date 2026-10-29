//! The local log store: SQLite with a trigram full-text index.
//!
//! Everything people do in the logs view (search, filters, the time graph,
//! error groups) is a query here, so nothing goes back to the Docker host.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;

use crate::docker::frames::StreamKind;
use crate::events::{Event, Kind};
use crate::logs::{Level, LogEntry};
use crate::model::LogSource;

pub struct Store {
    /// The one connection that writes. Its lock is held only briefly, so new
    /// lines are never stuck behind a slow read.
    db: Mutex<Connection>,
    /// Idle read-only connections. Thanks to WAL, reads see a consistent
    /// snapshot and run alongside writes and each other.
    readers: Mutex<Vec<Connection>>,
    /// The database file, to count its write-ahead log too. None in memory.
    path: Option<std::path::PathBuf>,
    /// Lines from this time on (µs) are all counted in `minutes`. Lines
    /// stored before the table existed are not, so older ranges read them.
    minutes_since: std::sync::atomic::AtomicI64,
    /// The (conn, service, ts) index exists. It is built in the background
    /// the first time, see [`Store::upgrade`].
    service_index: std::sync::atomic::AtomicBool,
    /// Counts clears of the store, so collectors read their sources again
    /// from the start instead of going on after lines that are gone.
    epoch: std::sync::atomic::AtomicU64,
}

/// Idle read connections kept open; more are opened while busy.
const IDLE_READERS: usize = 4;

/// Keeps the search index in step with new lines. Their per-minute counts
/// are added by [`Store::insert`], once per minute instead of once per line.
const INSERT_TRIGGER: &str = "CREATE TRIGGER entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts (rowid, text) VALUES (new.id, new.text);
END;";

/// Adds to the counts of a minute: for the service, and for the whole connection (service '').
const ADD_MINUTE: &str =
    "INSERT INTO minutes (conn, minute, service, stack, debug, info, warn, error, debug_exc, info_exc, warn_exc, error_exc)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
    ON CONFLICT (conn, minute, service) DO UPDATE SET
        debug = debug + excluded.debug, info = info + excluded.info, warn = warn + excluded.warn, error = error + excluded.error,
        debug_exc = debug_exc + excluded.debug_exc, info_exc = info_exc + excluded.info_exc,
        warn_exc = warn_exc + excluded.warn_exc, error_exc = error_exc + excluded.error_exc";

/// Keeps the search index and the per-minute counts in step with deleted
/// lines. Deleting every line drops it for a moment, so the table can be
/// emptied at once.
const DELETE_TRIGGER: &str = "CREATE TRIGGER entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts (entries_fts, rowid, text) VALUES ('delete', old.id, old.text);
    UPDATE minutes SET
        debug = debug - (old.level = 1), info = info - (old.level NOT IN (1, 3, 4)),
        warn = warn - (old.level = 3), error = error - (old.level = 4),
        debug_exc = debug_exc - (old.exc != 0 AND old.level = 1), info_exc = info_exc - (old.exc != 0 AND old.level NOT IN (1, 3, 4)),
        warn_exc = warn_exc - (old.exc != 0 AND old.level = 3), error_exc = error_exc - (old.exc != 0 AND old.level = 4)
    WHERE conn = old.conn AND minute = old.ts / 60000000 AND service IN (old.service, '');
END;";

const MINUTE: i64 = 60_000_000;

const SCHEMA: &str = r#"
PRAGMA auto_vacuum = INCREMENTAL;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA journal_size_limit = 67108864;
CREATE TABLE IF NOT EXISTS entries (
    id INTEGER PRIMARY KEY,
    conn TEXT NOT NULL,
    ts INTEGER NOT NULL,
    stack TEXT NOT NULL,
    service TEXT NOT NULL,
    replica TEXT NOT NULL,
    node TEXT NOT NULL,
    source TEXT NOT NULL,
    stream INTEGER NOT NULL,
    level INTEGER NOT NULL,
    exc INTEGER NOT NULL,
    lines INTEGER NOT NULL,
    fp TEXT,
    text TEXT NOT NULL,
    raw TEXT
);
-- Covers the time graph and level counts, so they never read the rows themselves.
CREATE INDEX IF NOT EXISTS entries_conn_ts_level ON entries (conn, ts, level, exc);
DROP INDEX IF EXISTS entries_conn_ts;
-- Only lines that belong to an error group, with what the group list needs.
CREATE INDEX IF NOT EXISTS entries_conn_ts_fp ON entries (conn, ts, fp, level, exc, service, node, source) WHERE fp IS NOT NULL;
CREATE INDEX IF NOT EXISTS entries_conn_fp ON entries (conn, fp, ts) WHERE fp IS NOT NULL;
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5 (text, content = 'entries', content_rowid = 'id', tokenize = 'trigram');
-- Lines per minute, service and level, and per minute of the whole
-- connection (service ''): the time graph and level counts of long ranges
-- read these instead of every line.
CREATE TABLE IF NOT EXISTS minutes (
    conn TEXT NOT NULL,
    minute INTEGER NOT NULL,
    service TEXT NOT NULL,
    stack TEXT NOT NULL,
    debug INTEGER NOT NULL,
    info INTEGER NOT NULL,
    warn INTEGER NOT NULL,
    error INTEGER NOT NULL,
    debug_exc INTEGER NOT NULL,
    info_exc INTEGER NOT NULL,
    warn_exc INTEGER NOT NULL,
    error_exc INTEGER NOT NULL,
    PRIMARY KEY (conn, minute, service)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS patterns (conn TEXT NOT NULL, fp TEXT NOT NULL, pattern TEXT NOT NULL, PRIMARY KEY (conn, fp));
CREATE TABLE IF NOT EXISTS cursors (conn TEXT NOT NULL, source TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (conn, source));
CREATE TABLE IF NOT EXISTS hosts (conn TEXT PRIMARY KEY, identity TEXT NOT NULL);
-- Container starts, stops and crashes; see events.rs. The columns the log
-- filters use are named as in `entries`, so the same filters apply.
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    conn TEXT NOT NULL,
    key TEXT NOT NULL,
    ts INTEGER NOT NULL,
    stack TEXT NOT NULL,
    service TEXT NOT NULL,
    replica TEXT NOT NULL,
    node TEXT NOT NULL,
    source TEXT NOT NULL,
    kind TEXT NOT NULL,
    exit_code INTEGER,
    detail TEXT,
    UNIQUE (conn, key)
);
CREATE INDEX IF NOT EXISTS events_conn_ts ON events (conn, ts);
-- What each service ran at its last deploy; see deploys.rs.
CREATE TABLE IF NOT EXISTS deploy_state (conn TEXT NOT NULL, service TEXT NOT NULL, ts INTEGER NOT NULL, config TEXT NOT NULL, PRIMARY KEY (conn, service));
"#;

/// Filters shared by queries and exports. Times are Unix microseconds.
#[derive(Clone, Debug, Default)]
pub struct LogFilter {
    pub from: i64,
    pub to: i64,
    pub stacks: Vec<String>,
    /// Substring match, from the search box.
    pub services: Vec<String>,
    /// Exact full service names, from the source picker. Matches any of these
    /// services or any service of `pick_stacks`.
    pub exact_services: Vec<String>,
    /// Whole stacks from the source picker, combined with `exact_services` by OR.
    pub pick_stacks: Vec<String>,
    pub levels: Vec<Level>,
    pub nodes: Vec<String>,
    pub replicas: Vec<String>,
    pub fps: Vec<String>,
    pub sources: Vec<String>,
    pub text: Vec<String>,
    /// Every level was switched off: match nothing.
    pub no_levels: bool,
}

impl LogFilter {
    /// Applies the search box syntax: `service:api level:error node:x replica:2 fp:… task:…` plus free text.
    pub fn apply_query(&mut self, q: &str) {
        for tok in q.split_whitespace() {
            match tok.split_once(':') {
                Some((k, v)) if !v.is_empty() => match k.to_ascii_lowercase().as_str() {
                    "service" | "svc" => self.services.push(v.to_string()),
                    "stack" | "project" => self.stacks.push(v.to_string()),
                    "level" => {
                        if let Some(l) = Level::parse_name(v) {
                            self.levels.retain(|x| *x == l);
                            if self.levels.is_empty() {
                                self.levels.push(l);
                            }
                        }
                    }
                    "node" | "host" => self.nodes.push(v.to_string()),
                    "replica" => self.replicas.push(v.to_string()),
                    "fp" | "group" => self.fps.push(v.to_string()),
                    "task" | "container" => self.sources.push(v.to_string()),
                    _ => self.text.push(tok.to_string()),
                },
                _ => self.text.push(tok.to_string()),
            }
        }
    }

    fn where_clause(&self, conn: &str, with_levels: bool) -> (String, Vec<Sql>) {
        self.where_clause_at(conn, with_levels, "ts", self.from, self.to)
    }

    /// The same filter on another time column: `minute` of the per-minute counts.
    fn where_clause_at(&self, conn: &str, with_levels: bool, time: &str, from: i64, to: i64) -> (String, Vec<Sql>) {
        let mut w = vec!["conn = ?".to_string(), format!("{time} >= ?"), format!("{time} < ?")];
        let mut p: Vec<Sql> = vec![Sql::Text(conn.into()), Sql::Integer(from), Sql::Integer(to)];
        let any_of = |col: &str, vals: &[String], like: bool, w: &mut Vec<String>, p: &mut Vec<Sql>| {
            if vals.is_empty() {
                return;
            }
            let parts: Vec<String> = vals
                .iter()
                .map(|v| {
                    if like {
                        p.push(Sql::Text(format!("%{}%", escape_like(v))));
                        format!("{col} LIKE ? ESCAPE '\\'")
                    } else {
                        p.push(Sql::Text(v.clone()));
                        format!("{col} = ?")
                    }
                })
                .collect();
            w.push(format!("({})", parts.join(" OR ")));
        };
        any_of("service", &self.services, true, &mut w, &mut p);
        if !self.exact_services.is_empty() || !self.pick_stacks.is_empty() {
            let mut parts = Vec::new();
            for v in &self.exact_services {
                parts.push("service = ?");
                p.push(Sql::Text(v.clone()));
            }
            for v in &self.pick_stacks {
                parts.push("stack = ?");
                p.push(Sql::Text(v.clone()));
            }
            w.push(format!("({})", parts.join(" OR ")));
        }
        any_of("stack", &self.stacks, false, &mut w, &mut p);
        any_of("node", &self.nodes, true, &mut w, &mut p);
        any_of("replica", &self.replicas, false, &mut w, &mut p);
        any_of("fp", &self.fps, false, &mut w, &mut p);
        any_of("source", &self.sources, true, &mut w, &mut p);
        if with_levels && self.no_levels {
            w.push("0".into());
        }
        if with_levels && !self.levels.is_empty() {
            let l: Vec<String> = self.levels.iter().map(|l| l.as_i64().to_string()).collect();
            w.push(format!("level IN ({})", l.join(",")));
        }
        let (fts, short): (Vec<&String>, Vec<&String>) = self.text.iter().partition(|t| t.chars().count() >= 3);
        if !fts.is_empty() {
            let expr: Vec<String> = fts.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
            w.push("id IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH ?)".into());
            p.push(Sql::Text(expr.join(" AND ")));
        }
        for t in short {
            w.push("text LIKE ? ESCAPE '\\'".into());
            p.push(Sql::Text(format!("%{}%", escape_like(t))));
        }
        (w.join(" AND "), p)
    }
}

/// Error groups whose first entry is in the last 15 minutes. Needs at least an
/// hour of stored history, or every group would look new after a fresh start.
fn new_groups(db: &Connection, conn: &str, now: i64, oldest: Option<i64>) -> Result<Vec<NewGroup>> {
    const WINDOW: i64 = 15 * 60_000_000;
    const HISTORY: i64 = 3_600_000_000;
    if oldest.is_none_or(|o| o > now - HISTORY) {
        return Ok(Vec::new());
    }
    let since = now - WINDOW;
    let mut st = db.prepare_cached(
        "SELECT fp, COUNT(*), MIN(ts), MIN(service), MAX(id) FROM entries
         WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 AND fp IS NOT NULL GROUP BY fp",
    )?;
    let recent = st
        .query_map(params![conn, since, now, Level::Error.as_i64()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut before = db.prepare_cached("SELECT 1 FROM entries WHERE conn = ?1 AND fp = ?2 AND ts < ?3 LIMIT 1")?;
    let mut pat = db.prepare_cached("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2")?;
    let mut text = db.prepare_cached("SELECT text FROM entries WHERE id = ?1")?;
    let mut out = Vec::new();
    for (fp, count, first, service, last_id) in recent {
        if before.exists(params![conn, fp, since])? {
            continue;
        }
        let pattern = pat.query_row(params![conn, fp], |r| r.get(0)).optional()?.unwrap_or_default();
        let sample = text.query_row(params![last_id], |r| r.get(0)).optional()?.unwrap_or_default();
        out.push(NewGroup { fp, pattern, count, service, first, sample });
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.count));
    Ok(out)
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LevelCounts {
    #[serde(rename = "ERROR")]
    pub error: u64,
    #[serde(rename = "WARN")]
    pub warn: u64,
    #[serde(rename = "INFO")]
    pub info: u64,
    #[serde(rename = "DEBUG")]
    pub debug: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Bucket {
    pub error: u64,
    pub warn: u64,
    pub info: u64,
    pub debug: u64,
    pub exc: u64,
}

impl LevelCounts {
    fn get_mut(&mut self, l: Level) -> &mut u64 {
        match l {
            Level::Error => &mut self.error,
            Level::Warn => &mut self.warn,
            Level::Info => &mut self.info,
            Level::Debug => &mut self.debug,
        }
    }
}

impl Bucket {
    fn get_mut(&mut self, l: Level) -> &mut u64 {
        match l {
            Level::Error => &mut self.error,
            Level::Warn => &mut self.warn,
            Level::Info => &mut self.info,
            Level::Debug => &mut self.debug,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub fp: String,
    pub pattern: String,
    pub count: u64,
    pub level: Level,
    pub first: i64,
    pub last: i64,
    pub services: Vec<String>,
    pub nodes: Vec<String>,
    pub sources: u64,
    pub exc: bool,
    pub sample: String,
    pub spark: Vec<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryOut {
    pub id: i64,
    pub ts: i64,
    pub stack: String,
    pub service: String,
    pub replica: String,
    pub node: String,
    pub source: String,
    pub stream: &'static str,
    pub level: Level,
    pub exc: bool,
    pub lines: u32,
    pub fp: Option<String>,
    pub text: String,
    pub raw: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResult {
    pub total: u64,
    pub levels: LevelCounts,
    pub histogram: Vec<Bucket>,
    pub groups: Vec<Group>,
    pub entries: Vec<EntryOut>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUsage {
    pub service: String,
    pub entries: u64,
    /// Bytes of log text; the search index adds to this on disk.
    pub text_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStats {
    pub bytes: u64,
    pub entries: u64,
    pub oldest: Option<i64>,
}

/// Error counts for the overview: per service in the last hour, the hour
/// before, and a 12 hour sparkline in 24 buckets.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorSummary {
    pub by_service: HashMap<String, u64>,
    pub last_hour: u64,
    pub prev_hour: u64,
    pub spark: Vec<u64>,
    /// The oldest stored entry of this connection (µs), for the "Auto" time range.
    pub oldest: Option<i64>,
    /// Error groups seen for the first time in the last 15 minutes.
    pub new_groups: Vec<NewGroup>,
    /// Services that logged steadily and then stopped logging.
    pub quiet: Vec<Quiet>,
}

/// A service that wrote lines in most minutes of the two hours before and
/// nothing in the last ten: often a process that hangs while its container
/// still runs.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Quiet {
    pub service: String,
    /// The end of the last minute with a line, µs.
    pub since: i64,
    /// Minutes with lines in the two hours before it went quiet, of 120.
    pub busy_minutes: u32,
}

/// An error group that did not exist before the last 15 minutes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGroup {
    pub fp: String,
    pub pattern: String,
    pub count: u64,
    pub service: String,
    /// µs
    pub first: i64,
    pub sample: String,
}

/// Errors of one connection in a recent window, for the error-rate alert.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorRate {
    pub count: u64,
    /// The most frequent error of the window and the service it came from.
    pub top: Option<String>,
    pub top_service: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Log,
    Jsonl,
    Csv,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?, Some(path.to_path_buf()))
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(db: Connection, path: Option<std::path::PathBuf>) -> Result<Store> {
        db.execute_batch(SCHEMA)?;
        // The triggers and the start of the per-minute counts change together:
        // every line stored after this is counted.
        db.execute_batch(&format!(
            "BEGIN IMMEDIATE;
             DROP TRIGGER IF EXISTS entries_ai;
             DROP TRIGGER IF EXISTS entries_ad;
             {INSERT_TRIGGER}
             {DELETE_TRIGGER}
             COMMIT;"
        ))?;
        let since = match db.query_row("SELECT value FROM meta WHERE key = 'minutes_since'", [], |r| r.get(0)).optional()? {
            Some(v) => v,
            None => {
                // Lines already stored are not counted: start after the newest.
                let mut newest: Option<i64> = None;
                for c in conns(&db)? {
                    let t: Option<i64> = db.query_row("SELECT MAX(ts) FROM entries WHERE conn = ?1", params![c], |r| r.get(0))?;
                    newest = newest.max(t);
                }
                let since = newest.map_or(0, |t| (t.div_euclid(MINUTE) + 1) * MINUTE);
                db.execute("INSERT INTO meta (key, value) VALUES ('minutes_since', ?1)", params![since])?;
                since
            }
        };
        let service_index: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'entries_conn_service_ts')",
            [],
            |r| r.get(0),
        )?;
        Ok(Store {
            db: Mutex::new(db),
            readers: Mutex::new(Vec::new()),
            path,
            minutes_since: since.into(),
            service_index: service_index.into(),
            epoch: Default::default(),
        })
    }

    /// Builds what older stores lack. On a large store this takes a while, so
    /// it runs once in the background; queries work without it, only slower.
    pub fn upgrade(&self) -> Result<()> {
        if !self.service_index.load(std::sync::atomic::Ordering::Relaxed) {
            let started = std::time::Instant::now();
            self.db.lock().unwrap().execute_batch("CREATE INDEX IF NOT EXISTS entries_conn_service_ts ON entries (conn, service, ts);")?;
            self.service_index.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("built the service index of the log store in {:.1} s", started.elapsed().as_secs_f64());
        }
        Ok(())
    }

    /// Runs a read on a read-only connection of its own. In memory there is
    /// only the one connection.
    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let Some(path) = &self.path else { return f(&self.db.lock().unwrap()) };
        let idle = self.readers.lock().unwrap().pop();
        let conn = match idle {
            Some(c) => c,
            None => {
                let c = Connection::open_with_flags(
                    path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                // Waits out the moment a checkpoint resets the write-ahead log.
                c.busy_timeout(std::time::Duration::from_secs(10))?;
                c
            }
        };
        let r = f(&conn);
        let mut idle = self.readers.lock().unwrap();
        if idle.len() < IDLE_READERS {
            idle.push(conn);
        }
        r
    }

    pub fn insert(&self, conn: &str, src: &LogSource, entries: &[LogEntry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO entries (conn, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            )?;
            let mut pat = tx.prepare_cached(
                "INSERT INTO patterns (conn, fp, pattern) VALUES (?1, ?2, ?3)
                 ON CONFLICT (conn, fp) DO UPDATE SET pattern = excluded.pattern WHERE pattern != excluded.pattern",
            )?;
            // Counts per minute: debug, info, warn, error, then the same with a stack trace.
            let mut minutes: HashMap<i64, [i64; 8]> = HashMap::new();
            for e in entries {
                let m = minutes.entry(e.ts.div_euclid(MINUTE)).or_default();
                let l = e.level.as_i64() as usize - 1;
                m[l] += 1;
                m[4 + l] += e.exc as i64;
                ins.execute(params![
                    conn,
                    e.ts,
                    src.stack,
                    src.service,
                    src.replica,
                    src.node,
                    src.key,
                    if e.stream == StreamKind::Stderr { 2 } else { 1 },
                    e.level.as_i64(),
                    e.exc as i64,
                    e.lines,
                    e.fp,
                    e.text,
                    e.raw,
                ])?;
                if let (Some(fp), Some(p)) = (&e.fp, &e.pattern) {
                    pat.execute(params![conn, fp, p])?;
                }
            }
            let mut add = tx.prepare_cached(ADD_MINUTE)?;
            for (minute, c) in minutes {
                for (service, stack) in [(src.service.as_str(), src.stack.as_str()), ("", "")] {
                    add.execute(params![conn, minute, service, stack, c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn cursor(&self, conn: &str, source: &str) -> Result<Option<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row("SELECT ts FROM cursors WHERE conn = ?1 AND source = ?2", params![conn, source], |r| r.get(0)).optional()?)
    }

    pub fn set_cursor(&self, conn: &str, source: &str, ts: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO cursors (conn, source, ts) VALUES (?1, ?2, ?3)
             ON CONFLICT (conn, source) DO UPDATE SET ts = excluded.ts",
            params![conn, source, ts],
        )?;
        Ok(())
    }

    /// What each service of a connection ran at its last deploy: time (µs) and config JSON.
    pub fn deploy_state(&self, conn: &str) -> Result<HashMap<String, (i64, String)>> {
        self.read(|db| {
            let mut st = db.prepare_cached("SELECT service, ts, config FROM deploy_state WHERE conn = ?1")?;
            let rows = st.query_map(params![conn], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?;
            Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
        })
    }

    pub fn set_deploy_state(&self, conn: &str, service: &str, ts: i64, config: &str) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO deploy_state (conn, service, ts, config) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (conn, service) DO UPDATE SET ts = excluded.ts, config = excluded.config",
            params![conn, service, ts, config],
        )?;
        Ok(())
    }

    /// Error groups of a service that first showed up in `[from, to)`, after a
    /// deploy at `from`: none of their lines is older. Needs lines of the
    /// connection from at least 30 minutes before, or everything would be new.
    pub fn new_since(&self, conn: &str, service: &str, from: i64, to: i64) -> Result<Vec<NewGroup>> {
        self.read(|db| {
            let oldest: Option<i64> = db.query_row("SELECT MIN(ts) FROM entries WHERE conn = ?1", params![conn], |r| r.get(0))?;
            if oldest.is_none_or(|o| o > from - 1_800_000_000) {
                return Ok(Vec::new());
            }
            let mut st = db.prepare_cached(
                "SELECT fp, COUNT(*), MIN(ts), MAX(id) FROM entries
                 WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 AND fp IS NOT NULL AND service = ?5 GROUP BY fp",
            )?;
            let found = st
                .query_map(params![conn, from, to, Level::Error.as_i64(), service], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut before = db.prepare_cached("SELECT 1 FROM entries WHERE conn = ?1 AND fp = ?2 AND ts < ?3 LIMIT 1")?;
            let mut pat = db.prepare_cached("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2")?;
            let mut text = db.prepare_cached("SELECT text FROM entries WHERE id = ?1")?;
            let mut out = Vec::new();
            for (fp, count, first, last_id) in found {
                if before.exists(params![conn, fp, from])? {
                    continue;
                }
                let pattern = pat.query_row(params![conn, fp], |r| r.get(0)).optional()?.unwrap_or_default();
                let sample = text.query_row(params![last_id], |r| r.get(0)).optional()?.unwrap_or_default();
                out.push(NewGroup { fp, pattern, count, service: service.to_string(), first, sample });
            }
            out.sort_by_key(|g| std::cmp::Reverse(g.count));
            Ok(out)
        })
    }

    /// Stores events read from Docker; ones already stored are skipped.
    /// Returns how many were new.
    pub fn insert_events(&self, conn: &str, events: &[Event]) -> Result<usize> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        let mut n = 0;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR IGNORE INTO events (conn, key, ts, stack, service, replica, node, source, kind, exit_code, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for e in events {
                n += ins.execute(params![
                    conn,
                    e.key,
                    e.ts,
                    e.stack,
                    e.service,
                    e.replica,
                    e.node,
                    e.source,
                    e.kind.as_str(),
                    e.exit_code,
                    e.detail
                ])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// The newest events in the filter's time range and sources, oldest
    /// first. Levels, error groups and search words do not apply to events.
    pub fn events(&self, conn: &str, f: &LogFilter, limit: usize) -> Result<Vec<Event>> {
        let f = LogFilter { fps: vec![], text: vec![], levels: vec![], no_levels: false, ..f.clone() };
        let (w, mut p) = f.where_clause(conn, false);
        p.push(Sql::Integer(limit as i64));
        self.read(|db| {
            let mut stmt = db.prepare(&format!(
                "SELECT key, ts, stack, service, replica, node, source, kind, exit_code, detail FROM events WHERE {w} ORDER BY ts DESC LIMIT ?"
            ))?;
            let mut out: Vec<Event> = stmt
                .query_map(params_from_iter(p), |r| {
                    Ok(Event {
                        key: r.get(0)?,
                        ts: r.get(1)?,
                        stack: r.get(2)?,
                        service: r.get(3)?,
                        replica: r.get(4)?,
                        node: r.get(5)?,
                        source: r.get(6)?,
                        kind: Kind::parse(&r.get::<_, String>(7)?).unwrap_or(Kind::Crash),
                        exit_code: r.get(8)?,
                        detail: r.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            out.reverse();
            Ok(out)
        })
    }

    pub fn query(&self, conn: &str, f: &LogFilter, buckets: usize, limit: usize) -> Result<QueryResult> {
        let since = self.minutes_since.load(std::sync::atomic::Ordering::Relaxed);
        let indexed = self.service_index.load(std::sync::atomic::Ordering::Relaxed);
        self.read(|db| query(db, conn, f, buckets, limit, since, indexed))
    }

    pub fn error_summary(&self, conn: &str, now: i64) -> Result<ErrorSummary> {
        let since = self.minutes_since.load(std::sync::atomic::Ordering::Relaxed);
        self.read(|db| {
            let mut s = error_summary(db, conn, now)?;
            s.quiet = quiet(db, conn, now, since)?;
            Ok(s)
        })
    }

    /// Writes every matching entry, oldest first. Returns the number written.
    pub fn export(&self, conn: &str, f: &LogFilter, format: ExportFormat, out: &mut dyn Write) -> Result<u64> {
        self.read(|db| export(db, conn, f, format, out))
    }
}

impl Store {
    /// Deletes entries older than `before` (µs) and the oldest entries beyond
    /// `max_bytes`, then gives the space back to the system.
    pub fn retain(&self, before: i64, max_bytes: u64) -> Result<u64> {
        const BATCH: usize = 20_000;
        let mut deleted = 0u64;
        // Per connection, so the (conn, ts) index finds old lines without
        // reading the table. The lock is taken per batch, so queries and new
        // lines get through between batches.
        let conns = conns(&self.db.lock().unwrap())?;
        for conn in conns {
            loop {
                let n = self.db.lock().unwrap().execute(
                    "DELETE FROM entries WHERE id IN (SELECT id FROM entries WHERE conn = ?1 AND ts < ?2 LIMIT ?3)",
                    params![conn, before, BATCH],
                )?;
                deleted += n as u64;
                if n < BATCH {
                    break;
                }
            }
            // Counts of minutes with no lines left.
            self.db
                .lock()
                .unwrap()
                .execute("DELETE FROM minutes WHERE conn = ?1 AND minute < ?2", params![conn, before.div_euclid(MINUTE)])?;
        }
        self.db.lock().unwrap().execute("DELETE FROM events WHERE ts < ?1", params![before])?;
        // Over the size limit: drop the oldest lines first, down to 90% of the
        // limit so this does not run again a minute later. Ids grow with
        // arrival, so the rowid order finds them without sorting the table.
        // Deleted lines stay in the search index until it is merged, so the
        // index is optimized before the size is measured again.
        for _ in 0..4 {
            let live = live_bytes(&self.db.lock().unwrap())?;
            if live <= max_bytes {
                break;
            }
            let total: u64 = self.db.lock().unwrap().query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
            if total == 0 {
                break;
            }
            let target = max_bytes / 10 * 9;
            let mut left = ((total as u128 * (live - target) as u128).div_ceil(live as u128) as u64).clamp(1, total);
            while left > 0 {
                let n = self.db.lock().unwrap().execute(
                    "DELETE FROM entries WHERE id IN (SELECT id FROM entries ORDER BY id LIMIT ?1)",
                    params![left.min(BATCH as u64)],
                )? as u64;
                deleted += n;
                left = left.saturating_sub(n);
                if n == 0 {
                    break;
                }
            }
            self.merge_index()?;
        }
        {
            let db = self.db.lock().unwrap();
            let mode: i64 = db.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
            if mode != 2 {
                // Stores made before 0.3 could never shrink: switch them over once.
                tracing::info!("rebuilding the log store once so it can give space back");
                db.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
            }
        }
        // Also when nothing was deleted now: up to 0.3.1 free space was never
        // given back, so stores made by those versions shrink here once.
        self.shrink()?;
        Ok(deleted)
    }

    /// Gives free pages back to the system and moves the write-ahead log
    /// into the file, so the file shrinks on disk. In steps that release the
    /// lock in between, so queries and new lines get through.
    fn shrink(&self) -> Result<()> {
        const STEP: u64 = 4096;
        let mut last = u64::MAX;
        loop {
            let db = self.db.lock().unwrap();
            let free: u64 = db.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
            if free == 0 || free >= last {
                return checkpoint(&db);
            }
            last = free;
            // Each step of this pragma frees one page, so it is run to the end:
            // run once, as `execute_batch` does, it freed a single page.
            let mut st = db.prepare(&format!("PRAGMA incremental_vacuum({})", free.min(STEP)))?;
            let mut rows = st.query([])?;
            while rows.next()?.is_some() {}
        }
    }

    /// Merges the search index so deleted lines leave it, like FTS5's
    /// `optimize`, but in steps that release the lock in between: on a large
    /// store a full optimize would block queries and new lines for minutes.
    fn merge_index(&self) -> Result<()> {
        loop {
            let db = self.db.lock().unwrap();
            let before = db.total_changes();
            db.execute("INSERT INTO entries_fts (entries_fts, rank) VALUES ('merge', -500)", [])?;
            if db.total_changes() - before <= 1 {
                return Ok(());
            }
        }
    }

    /// Space on disk: the database file plus its write-ahead log (kept under 64 MB).
    fn used_bytes(&self, db: &Connection) -> Result<u64> {
        let file =
            |suffix: &str| self.path.as_ref().and_then(|p| std::fs::metadata(format!("{}{suffix}", p.display())).ok()).map(|m| m.len());
        match file("") {
            Some(main) => Ok(main + file("-wal").unwrap_or(0)),
            None => live_bytes(db),
        }
    }

    /// Lines and stored text per service of a connection, largest first.
    pub fn usage(&self, conn: &str) -> Result<Vec<ServiceUsage>> {
        self.read(|db| usage(db, conn))
    }

    /// Deletes the stored lines of one service, in batches.
    pub fn clear_service(&self, conn: &str, service: &str) -> Result<u64> {
        let deleted = self.delete_entries(Some(conn), Some(service))?;
        if deleted > 0 {
            self.merge_index()?;
            self.shrink()?;
        }
        Ok(deleted)
    }

    /// Deletes the lines of a connection (all when None), or of one of its
    /// services. Every deleted line also has to leave the search index, which
    /// is the slow part, so it runs in small batches and releases the lock in
    /// between. When nothing would be left, the table and the index are
    /// emptied at once instead.
    fn delete_entries(&self, conn: Option<&str>, service: Option<&str>) -> Result<u64> {
        const BATCH: i64 = 5_000;
        {
            let db = self.db.lock().unwrap();
            let everything = match (conn, service) {
                (None, _) => true,
                (Some(c), None) => conns(&db)? == [c],
                (Some(_), Some(_)) => false,
            };
            if everything {
                let n: u64 = db.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
                db.execute_batch(&format!(
                    "BEGIN;
                     INSERT INTO entries_fts (entries_fts) VALUES ('delete-all');
                     DROP TRIGGER entries_ad;
                     DELETE FROM entries;
                     DELETE FROM minutes;
                     UPDATE meta SET value = 0 WHERE key = 'minutes_since';
                     {DELETE_TRIGGER}
                     COMMIT;"
                ))?;
                // Nothing older is left, so every range can use the counts.
                self.minutes_since.store(0, std::sync::atomic::Ordering::Relaxed);
                return Ok(n);
            }
        }
        let (conn, mut deleted, mut after) = (conn.unwrap_or_default(), 0u64, 0i64);
        loop {
            let db = self.db.lock().unwrap();
            // Walks the rows in id order, so each batch starts where the last
            // one stopped instead of reading the table from the start again.
            let n = match service {
                Some(svc) => {
                    let last: Option<i64> = db.query_row(
                        "SELECT MAX(id) FROM (SELECT id FROM entries WHERE id > ?1 AND conn = ?2 AND service = ?3 ORDER BY id LIMIT ?4)",
                        params![after, conn, svc, BATCH],
                        |r| r.get(0),
                    )?;
                    let Some(last) = last else { break };
                    let n = db.execute(
                        "DELETE FROM entries WHERE id > ?1 AND id <= ?2 AND conn = ?3 AND service = ?4",
                        params![after, last, conn, svc],
                    )?;
                    after = last;
                    n
                }
                // The (conn, ts) index finds them; deleted rows leave it.
                None => {
                    db.execute("DELETE FROM entries WHERE id IN (SELECT id FROM entries WHERE conn = ?1 LIMIT ?2)", params![conn, BATCH])?
                }
            };
            deleted += n as u64;
            if service.is_none() && (n as i64) < BATCH {
                break;
            }
        }
        Ok(deleted)
    }

    /// Counts the error entries of a connection in `[from, to)` (µs).
    pub fn error_rate(&self, conn: &str, from: i64, to: i64) -> Result<ErrorRate> {
        self.read(|db| error_rate(db, conn, from, to))
    }

    /// Remembers which host or swarm a connection's logs come from. When a
    /// connection now reaches a different one (its address was edited, or an
    /// alias points elsewhere), the old lines are deleted so the two never mix.
    /// Returns true when lines were deleted.
    pub fn bind_host(&self, conn: &str, identity: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        let known: Option<String> = db.query_row("SELECT identity FROM hosts WHERE conn = ?1", params![conn], |r| r.get(0)).optional()?;
        if known.as_deref() == Some(identity) {
            return Ok(false);
        }
        let changed = known.is_some();
        drop(db);
        if changed {
            self.clear(Some(conn))?;
        }
        self.db.lock().unwrap().execute(
            "INSERT INTO hosts (conn, identity) VALUES (?1, ?2) ON CONFLICT (conn) DO UPDATE SET identity = excluded.identity",
            params![conn, identity],
        )?;
        Ok(changed)
    }

    pub fn stats(&self) -> Result<StoreStats> {
        self.read(|db| {
            let entries = db.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
            let mut oldest: Option<i64> = None;
            for c in conns(db)? {
                let t: Option<i64> = db.query_row("SELECT MIN(ts) FROM entries WHERE conn = ?1", params![c], |r| r.get(0))?;
                oldest = oldest.into_iter().chain(t).min();
            }
            Ok(StoreStats { bytes: self.used_bytes(db)?, entries, oldest })
        })
    }

    /// Changes whenever stored lines of a whole connection are cleared.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn clear(&self, conn: Option<&str>) -> Result<()> {
        // First, so lines a collector is reading right now are not stored
        // after the clear with a cursor that would skip everything before them.
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let deleted = self.delete_entries(conn, None)?;
        if deleted > 0 {
            self.merge_index()?;
        }
        let db = self.db.lock().unwrap();
        match conn {
            Some(c) => {
                db.execute("DELETE FROM minutes WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM patterns WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM cursors WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM hosts WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM events WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM deploy_state WHERE conn = ?1", params![c])?;
            }
            None => {
                db.execute_batch("DELETE FROM minutes; DELETE FROM patterns; DELETE FROM cursors; DELETE FROM hosts; DELETE FROM events; DELETE FROM deploy_state;")?
            }
        }
        drop(db);
        self.shrink()
    }
}

fn query(
    db: &Connection,
    conn: &str,
    f: &LogFilter,
    buckets: usize,
    limit: usize,
    minutes_since: i64,
    indexed: bool,
) -> Result<QueryResult> {
    let buckets = buckets.clamp(1, 500) as i64;
    let span = (f.to - f.from).max(1);
    // Picked stacks become their services, so the service index finds them.
    let resolved;
    let f = match indexed && !f.pick_stacks.is_empty() {
        true => {
            let mut g = f.clone();
            let found = services_of_stacks(db, conn, &g.pick_stacks)?;
            // With none found the stacks have no lines, and the filter stays as it is.
            if !found.is_empty() {
                g.exact_services.extend(found.into_iter().filter(|s| !f.exact_services.contains(s)));
                g.pick_stacks.clear();
            }
            resolved = g;
            &resolved
        }
        false => f,
    };

    // The level counts (which ignore the level filter, so the chips show what
    // each would add) and the histogram. Whole minutes come from the
    // per-minute counts when the filter only uses what they know; the rest is
    // one pass over (ts, level, exc), read from a covering index. Bucketing
    // here is about ten times faster than a GROUP BY, which makes SQLite sort
    // every row of the range.
    let wanted = |l: Level| !f.no_levels && (f.levels.is_empty() || f.levels.contains(&l));
    let mut levels = LevelCounts::default();
    let mut histogram = vec![Bucket::default(); buckets as usize];
    let mut total = 0u64;
    let mut count = |ts: i64, level: Level, n: u64, exc: u64| {
        *levels.get_mut(level) += n;
        if wanted(level) {
            let bk = &mut histogram[((ts - f.from) * buckets / span).clamp(0, buckets - 1) as usize];
            total += n;
            bk.exc += exc;
            *bk.get_mut(level) += n;
        }
    };
    let countable = f.text.is_empty() && f.fps.is_empty() && f.nodes.is_empty() && f.replicas.is_empty() && f.sources.is_empty();
    let lo = (f.from.div_euclid(MINUTE) + (f.from.rem_euclid(MINUTE) != 0) as i64).max(minutes_since.div_euclid(MINUTE));
    let hi = f.to.div_euclid(MINUTE);
    let mut raw = vec![(f.from, f.to)];
    if countable && hi - lo >= 30 {
        raw = vec![(f.from, f.from.max(lo * MINUTE)), (f.to.min(hi * MINUTE), f.to)];
        let (mut w, p) = f.where_clause_at(conn, false, "minute", lo, hi);
        // Without a service filter the rows of the whole connection are enough.
        let whole = f.services.is_empty() && f.exact_services.is_empty() && f.pick_stacks.is_empty() && f.stacks.is_empty();
        w.push_str(if whole { " AND service = ''" } else { " AND service != ''" });
        let mut st = db.prepare_cached(&format!(
            "SELECT minute, debug, info, warn, error, debug_exc, info_exc, warn_exc, error_exc FROM minutes WHERE {w}"
        ))?;
        let mut rows = st.query(params_from_iter(p.iter()))?;
        while let Some(r) = rows.next()? {
            let ts = r.get::<_, i64>(0)? * MINUTE;
            for (i, l) in [Level::Debug, Level::Info, Level::Warn, Level::Error].into_iter().enumerate() {
                let (n, x): (i64, i64) = (r.get(1 + i)?, r.get(5 + i)?);
                if n > 0 {
                    count(ts, l, n as u64, x.max(0) as u64);
                }
            }
        }
    }
    for (from, to) in raw.into_iter().filter(|(a, b)| a < b) {
        let (w, p) = f.where_clause_at(conn, false, "ts", from, to);
        let mut st = db.prepare_cached(&format!("SELECT ts, level, exc FROM entries WHERE {w}"))?;
        let mut rows = st.query(params_from_iter(p.iter()))?;
        while let Some(r) = rows.next()? {
            let (ts, level, exc): (i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            count(ts, Level::from_i64(level), 1, (exc != 0) as u64);
        }
    }

    // Error groups, aggregated the same way. The partial index holds only lines with a fingerprint.
    let (w, p) = f.where_clause(conn, true);
    // The sparklines (24 buckets per group) come from the same pass.
    struct Agg {
        g: Group,
        sources: std::collections::HashSet<String>,
        max_id: i64,
    }
    let mut by_fp: HashMap<String, Agg> = HashMap::new();
    let mut st = db.prepare(&format!(
        "SELECT fp, level, ts, exc, service, node, source, id FROM entries WHERE {w} AND fp IS NOT NULL AND level >= 3"
    ))?;
    let mut rows = st.query(params_from_iter(p.iter()))?;
    while let Some(r) = rows.next()? {
        // Borrowed strings: most rows only bump counters of a group that exists already.
        let fp = r.get_ref(0)?.as_str()?;
        let (level, ts, exc, id): (i64, i64, i64, i64) = (r.get(1)?, r.get(2)?, r.get(3)?, r.get(7)?);
        let level = Level::from_i64(level);
        let a = match by_fp.get_mut(fp) {
            Some(a) => a,
            None => by_fp.entry(fp.to_string()).or_insert(Agg {
                g: Group {
                    fp: fp.to_string(),
                    pattern: String::new(),
                    count: 0,
                    level,
                    first: ts,
                    last: ts,
                    services: vec![],
                    nodes: vec![],
                    sources: 0,
                    exc: false,
                    sample: String::new(),
                    spark: vec![0; 24],
                },
                sources: Default::default(),
                max_id: id,
            }),
        };
        let g = &mut a.g;
        g.count += 1;
        g.level = g.level.max(level);
        g.first = g.first.min(ts);
        g.last = g.last.max(ts);
        g.exc |= exc != 0;
        g.spark[((ts - f.from) * 24 / span).clamp(0, 23) as usize] += 1;
        let service = r.get_ref(4)?.as_str()?;
        if !g.services.iter().any(|x| x == service) {
            g.services.push(service.to_string());
        }
        let node = r.get_ref(5)?.as_str()?;
        if !g.nodes.iter().any(|x| x == node) {
            g.nodes.push(node.to_string());
        }
        let source = r.get_ref(6)?.as_str()?;
        if !a.sources.contains(source) {
            a.sources.insert(source.to_string());
        }
        a.max_id = a.max_id.max(id);
    }
    drop(rows);
    drop(st);
    let mut groups: Vec<Group> = by_fp
        .into_values()
        .map(|mut a| {
            a.g.sources = a.sources.len() as u64;
            a.g.sample = a.max_id.to_string();
            a.g
        })
        .collect();
    groups.sort_by(|a, b| b.count.cmp(&a.count).then(b.last.cmp(&a.last)));
    groups.truncate(50);
    if !groups.is_empty() {
        let mut pat = db.prepare_cached("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2")?;
        let mut sample = db.prepare_cached("SELECT text FROM entries WHERE id = ?1")?;
        for g in &mut groups {
            g.pattern = pat.query_row(params![conn, g.fp], |r| r.get(0)).optional()?.unwrap_or_default();
            let id: i64 = g.sample.parse().unwrap_or(0);
            g.sample = sample.query_row(params![id], |r| r.get(0)).optional()?.unwrap_or_default();
        }
    }

    // The newest lines. With picked services each one walks its own part of
    // the service index, newest first, and the results are merged: a quiet
    // service would otherwise mean reading the whole range.
    let limit = limit.clamp(1, 5000);
    let per_service = indexed && f.pick_stacks.is_empty() && (1..=50).contains(&f.exact_services.len());
    let parts: Vec<(String, Vec<Sql>)> = if per_service {
        f.exact_services.iter().map(|s| LogFilter { exact_services: vec![s.clone()], ..f.clone() }.where_clause(conn, true)).collect()
    } else {
        vec![(w, p)]
    };
    let mut entries = Vec::new();
    for (w, p) in parts {
        let mut st = db.prepare_cached(&format!(
            "SELECT id, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw
             FROM entries WHERE {w} ORDER BY ts DESC, id DESC LIMIT {limit}"
        ))?;
        entries.extend(st.query_map(params_from_iter(p.iter()), row_to_entry)?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    if per_service && f.exact_services.len() > 1 {
        entries.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.id.cmp(&a.id)));
        entries.truncate(limit);
    }

    Ok(QueryResult { total, levels, histogram, groups, entries })
}

/// The services with stored lines in these stacks, found by skipping through
/// the service index.
fn services_of_stacks(db: &Connection, conn: &str, stacks: &[String]) -> Result<Vec<String>> {
    let mut st = db.prepare_cached(
        "WITH RECURSIVE s(service) AS (
             SELECT MIN(service) FROM entries WHERE conn = ?1
             UNION ALL SELECT (SELECT MIN(service) FROM entries WHERE conn = ?1 AND service > s.service) FROM s WHERE s.service IS NOT NULL
         ) SELECT service, (SELECT stack FROM entries WHERE conn = ?1 AND service = s.service LIMIT 1) FROM s WHERE service IS NOT NULL",
    )?;
    let rows = st.query_map(params![conn], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (service, stack) = row?;
        if stacks.contains(&stack) {
            out.push(service);
        }
    }
    Ok(out)
}

fn error_summary(db: &Connection, conn: &str, now: i64) -> Result<ErrorSummary> {
    const HOUR: i64 = 3_600_000_000;
    let err = Level::Error.as_i64();
    let mut by_service = HashMap::new();
    // Without the hint SQLite walks the service index of the whole store to skip a sort.
    let mut st = db.prepare_cached(
        "SELECT service, COUNT(*) FROM entries INDEXED BY entries_conn_ts_level
         WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 GROUP BY service",
    )?;
    let rows = st.query_map(params![conn, now - HOUR, now, err], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?;
    for row in rows {
        let (svc, n) = row?;
        by_service.insert(svc, n);
    }
    let last_hour = by_service.values().sum();
    let prev_hour: u64 = db.query_row(
        "SELECT COUNT(*) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4",
        params![conn, now - 2 * HOUR, now - HOUR, err],
        |r| r.get(0),
    )?;
    let mut spark = vec![0u64; 24];
    let from = now - 12 * HOUR;
    let mut st = db.prepare_cached(
        "SELECT (ts - ?2) * 24 / ?3 AS b, COUNT(*) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?4 AND level = ?5 GROUP BY b",
    )?;
    let rows = st.query_map(params![conn, from, 12 * HOUR, now, err], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u64>(1)?)))?;
    for row in rows {
        let (b, n) = row?;
        spark[b.clamp(0, 23) as usize] += n;
    }
    let oldest: Option<i64> = db.query_row("SELECT MIN(ts) FROM entries WHERE conn = ?1", params![conn], |r| r.get(0))?;
    let new_groups = new_groups(db, conn, now, oldest)?;
    Ok(ErrorSummary { by_service, last_hour, prev_hour, spark, oldest, new_groups, quiet: Vec::new() })
}

/// Services that went quiet: lines in at least half the minutes of two hours,
/// then none for ten minutes. Read from the per-minute counts, so it needs
/// two hours and ten minutes of counted history.
fn quiet(db: &Connection, conn: &str, now: i64, minutes_since: i64) -> Result<Vec<Quiet>> {
    const SILENT: i64 = 10;
    const BASELINE: i64 = 120;
    let cut = now.div_euclid(MINUTE) - SILENT;
    let lo = cut - BASELINE;
    if minutes_since > lo * MINUTE {
        return Ok(Vec::new());
    }
    let mut st = db.prepare_cached(
        "SELECT service, SUM(minute < ?3), MAX(minute) FROM minutes
         WHERE conn = ?1 AND service != '' AND minute >= ?2 AND debug + info + warn + error > 0
         GROUP BY service HAVING SUM(minute < ?3) >= ?4 AND MAX(minute) < ?3",
    )?;
    let rows = st.query_map(params![conn, lo, cut, BASELINE / 2], |r| {
        Ok(Quiet { service: r.get(0)?, busy_minutes: r.get(1)?, since: (r.get::<_, i64>(2)? + 1) * MINUTE })
    })?;
    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    out.sort_by_key(|q| q.since);
    Ok(out)
}

fn export(db: &Connection, conn: &str, f: &LogFilter, format: ExportFormat, out: &mut dyn Write) -> Result<u64> {
    let (w, p) = f.where_clause(conn, true);
    let mut st = db.prepare(&format!(
        "SELECT id, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw
             FROM entries WHERE {w} ORDER BY ts ASC, id ASC"
    ))?;
    let mut rows = st.query(params_from_iter(p.iter()))?;
    let mut n = 0;
    if format == ExportFormat::Csv {
        writeln!(out, "time,level,stack,service,replica,node,stream,message")?;
    }
    while let Some(r) = rows.next()? {
        let e = row_to_entry(r)?;
        let time = chrono::DateTime::from_timestamp_micros(e.ts)
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
            .unwrap_or_default();
        match format {
            ExportFormat::Log => {
                writeln!(out, "{time} {:<5} {}.{} {}", e.level.name(), e.service, e.replica, e.text)?;
            }
            ExportFormat::Jsonl => {
                let v = serde_json::json!({
                    "time": time, "level": e.level.name(), "stack": e.stack, "service": e.service,
                    "replica": e.replica, "node": e.node, "stream": e.stream, "message": e.text,
                    "raw": e.raw.as_deref().and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()),
                });
                writeln!(out, "{v}")?;
            }
            ExportFormat::Csv => {
                writeln!(
                    out,
                    "{},{},{},{},{},{},{},{}",
                    time,
                    e.level.name(),
                    csv(&e.stack),
                    csv(&e.service),
                    csv(&e.replica),
                    csv(&e.node),
                    e.stream,
                    csv(&e.text)
                )?;
            }
        }
        n += 1;
    }
    Ok(n)
}

fn error_rate(db: &Connection, conn: &str, from: i64, to: i64) -> Result<ErrorRate> {
    let err = Level::Error.as_i64();
    let count: u64 = db.query_row(
        "SELECT COUNT(*) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4",
        params![conn, from, to, err],
        |r| r.get(0),
    )?;
    let mut rate = ErrorRate { count, ..Default::default() };
    if count == 0 {
        return Ok(rate);
    }
    // The service comes from the group's newest line: a bare column next to
    // GROUP BY would be any line's, and a group can span several services.
    let top: Option<(String, i64)> = db
        .query_row(
            "SELECT fp, MAX(id) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 AND fp IS NOT NULL
                 GROUP BY fp ORDER BY COUNT(*) DESC LIMIT 1",
            params![conn, from, to, err],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((fp, id)) = top {
        let pattern: Option<String> =
            db.query_row("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2", params![conn, fp], |r| r.get(0)).optional()?;
        let (service, sample): (String, String) =
            db.query_row("SELECT service, text FROM entries WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let text = pattern.filter(|p| !p.is_empty()).unwrap_or(sample);
        rate.top = Some(text.lines().next().unwrap_or("").chars().take(200).collect());
        rate.top_service = Some(service);
    }
    Ok(rate)
}

/// Lines and stored text per service of a connection, largest first.
fn usage(db: &Connection, conn: &str) -> Result<Vec<ServiceUsage>> {
    let mut st = db.prepare(
        "SELECT service, COUNT(*), SUM(LENGTH(text) + COALESCE(LENGTH(raw), 0)) AS b FROM entries WHERE conn = ?1
         GROUP BY service ORDER BY b DESC",
    )?;
    let rows = st.query_map(params![conn], |r| Ok(ServiceUsage { service: r.get(0)?, entries: r.get(1)?, text_bytes: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The connections that have stored lines, found by skipping through the
/// index instead of reading every row.
fn conns(db: &Connection) -> Result<Vec<String>> {
    let mut st = db.prepare_cached(
        "WITH RECURSIVE c(conn) AS (
             SELECT MIN(conn) FROM entries
             UNION ALL SELECT (SELECT MIN(conn) FROM entries WHERE conn > c.conn) FROM c WHERE c.conn IS NOT NULL
         ) SELECT conn FROM c WHERE conn IS NOT NULL",
    )?;
    let rows = st.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

/// Moves the write-ahead log into the database and empties it. A read still
/// running (an export, say) keeps part of the log in use: then it is left
/// for the next time instead of waiting, which would hold up new lines.
fn checkpoint(db: &Connection) -> Result<()> {
    db.busy_timeout(std::time::Duration::ZERO)?;
    let r = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(r?)
}

/// Pages that hold data; free pages are given back by `incremental_vacuum`.
fn live_bytes(db: &Connection) -> Result<u64> {
    let pages: u64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let free: u64 = db.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages.saturating_sub(free) * size)
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryOut> {
    Ok(EntryOut {
        id: r.get(0)?,
        ts: r.get(1)?,
        stack: r.get(2)?,
        service: r.get(3)?,
        replica: r.get(4)?,
        node: r.get(5)?,
        source: r.get(6)?,
        stream: if r.get::<_, i64>(7)? == 2 { "stderr" } else { "stdout" },
        level: Level::from_i64(r.get(8)?),
        exc: r.get::<_, i64>(9)? != 0,
        lines: r.get(10)?,
        fp: r.get(11)?,
        text: r.get(12)?,
        raw: r.get(13)?,
    })
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::assemble::Assembler;

    fn src(service: &str, replica: &str) -> LogSource {
        LogSource {
            key: format!("{service}-{replica}"),
            via_task: false,
            stack: "shop".into(),
            service: service.into(),
            replica: replica.into(),
            node: "shop".into(),
            tty: false,
            running: true,
        }
    }

    fn entries(lines: &[(i64, &str)]) -> Vec<LogEntry> {
        let mut a = Assembler::new("svc");
        let mut out = vec![];
        for (t, l) in lines {
            a.push(*t, StreamKind::Stdout, l, &mut out);
        }
        a.flush(&mut out);
        out
    }

    #[test]
    fn query_filters_histogram_and_groups() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_700_000_000_000_000i64;
        let s1 = src("shop_app", "1");
        let mut lines = vec![];
        for i in 0..10 {
            lines.push((base + i * 10_000_000, "2026-10-27 ERROR Upstream timed out after 5000ms (order 42)".to_string()));
            lines.push((base + i * 10_000_000 + 2_000_000, format!("2026-10-27 INFO GET /api/items/{i} 200")));
        }
        let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
        store.insert("c1", &s1, &entries(&refs)).unwrap();
        store.insert("c2", &s1, &entries(&refs)).unwrap();

        let mut f = LogFilter { from: base, to: base + 100_000_000, ..Default::default() };
        let r = store.query("c1", &f, 10, 100).unwrap();
        assert_eq!(r.total, 20);
        assert_eq!((r.levels.error, r.levels.info), (10, 10));
        assert_eq!(r.histogram.iter().map(|b| b.error).sum::<u64>(), 10);
        assert_eq!(r.groups.len(), 1);
        assert_eq!(r.groups[0].count, 10);
        assert!(r.groups[0].pattern.contains("<n>ms (order <n>)"), "{}", r.groups[0].pattern);
        assert_eq!(r.groups[0].spark.iter().sum::<u64>(), 10);

        f.apply_query("level:error timed");
        let r = store.query("c1", &f, 10, 100).unwrap();
        assert_eq!(r.entries.len(), 10);
        assert_eq!(r.levels.info, 0, "free text applies to level counts too");

        let mut f2 = LogFilter { from: base, to: base + 100_000_000, ..Default::default() };
        f2.apply_query("service:app api");
        assert_eq!(store.query("c1", &f2, 10, 100).unwrap().entries.len(), 10);

        let mut buf = Vec::new();
        let n = store.export("c1", &f2, ExportFormat::Csv, &mut buf).unwrap();
        assert_eq!(n, 10);
        assert!(String::from_utf8(buf).unwrap().starts_with("time,level,"));

        assert_eq!(store.retain(base + 50_000_000, u64::MAX).unwrap(), 20);
        let all = LogFilter { from: base, to: base + 100_000_000, ..Default::default() };
        assert_eq!(store.query("c1", &all, 10, 100).unwrap().total, 10);
        store.set_cursor("c1", "x", 5).unwrap();
        assert_eq!(store.cursor("c1", "x").unwrap(), Some(5));
    }

    #[test]
    fn events_are_stored_once_filtered_like_lines_and_cleared() {
        let store = Store::open_in_memory().unwrap();
        let ev = |key: &str, ts: i64, service: &str, kind: Kind| Event {
            key: key.into(),
            ts,
            stack: "demo".into(),
            service: service.into(),
            replica: "1".into(),
            node: "box".into(),
            source: format!("{key}0123"),
            kind,
            exit_code: Some(1),
            detail: None,
        };
        let batch = [ev("a", 10, "demo_app", Kind::Crash), ev("b", 20, "demo_db", Kind::Start), ev("c", 30, "demo_app", Kind::Oom)];
        assert_eq!(store.insert_events("c1", &batch).unwrap(), 3);
        assert_eq!(store.insert_events("c1", &batch).unwrap(), 0);
        let all = LogFilter { from: 0, to: 100, ..Default::default() };
        let keys = |f: &LogFilter, limit| store.events("c1", f, limit).unwrap().into_iter().map(|e| e.key).collect::<Vec<_>>();
        assert_eq!(keys(&all, 10), ["a", "b", "c"]);
        assert_eq!(keys(&all, 2), ["b", "c"]);
        let mut app = LogFilter { exact_services: vec!["demo_app".into()], ..all.clone() };
        app.apply_query("level:error timeout container:c01");
        assert_eq!(keys(&app, 10), ["c"]);
        assert!(store.events("c2", &all, 10).unwrap().is_empty());
        store.retain(15, u64::MAX).unwrap();
        assert_eq!(keys(&all, 10), ["b", "c"]);
        store.clear(Some("c1")).unwrap();
        assert!(keys(&all, 10).is_empty());
    }

    #[test]
    fn picked_stacks_and_services_combine_with_or() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_700_000_000_000_000i64;
        let mut web = src("web_app", "1");
        web.stack = "web".into();
        store.insert("c1", &src("shop_api", "1"), &entries(&[(base, "INFO api")])).unwrap();
        store.insert("c1", &src("shop_worker", "1"), &entries(&[(base + 1, "INFO worker")])).unwrap();
        store.insert("c1", &web, &entries(&[(base + 2, "INFO web")])).unwrap();
        let f = |stacks: &[&str], services: &[&str]| LogFilter {
            from: base,
            to: base + 10,
            pick_stacks: stacks.iter().map(|s| s.to_string()).collect(),
            exact_services: services.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(store.query("c1", &f(&["web"], &["shop_api"]), 1, 10).unwrap().total, 2);
        assert_eq!(store.query("c1", &f(&["shop"], &[]), 1, 10).unwrap().total, 2);
        assert_eq!(store.query("c1", &f(&[], &["shop_worker", "web_app"]), 1, 10).unwrap().total, 2);
    }

    #[test]
    fn new_groups_need_history_and_a_first_sighting() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        let min = 60_000_000i64;
        let s1 = src("shop_api", "1");
        let old = "2026-10-27 ERROR Upstream timed out after 5000ms";
        let fresh = "2026-10-27 ERROR Cannot connect to redis:6379";
        store
            .insert("c1", &s1, &entries(&[(now - 120 * min, old), (now - 5 * min, old), (now - 4 * min, fresh), (now - 2 * min, fresh)]))
            .unwrap();
        let s = store.error_summary("c1", now).unwrap();
        assert_eq!(s.oldest, Some(now - 120 * min));
        assert_eq!(s.new_groups.len(), 1, "{:?}", s.new_groups);
        assert_eq!((s.new_groups[0].count, s.new_groups[0].service.as_str()), (2, "shop_api"));
        assert!(s.new_groups[0].sample.contains("redis"));

        // Less than an hour of history: nothing counts as new yet.
        store.insert("c2", &s1, &entries(&[(now - 10 * min, old), (now - 4 * min, fresh)])).unwrap();
        assert!(store.error_summary("c2", now).unwrap().new_groups.is_empty());
    }

    #[test]
    fn errors_new_since_a_deploy() {
        let store = Store::open_in_memory().unwrap();
        let min = 60_000_000i64;
        let deploy = 1_700_000_000_000_000i64;
        let old = "2026-10-27 ERROR Upstream timed out after 5000ms";
        let fresh = "2026-10-27 ERROR Column orders.total does not exist";
        let s1 = src("shop_api", "1");
        store
            .insert(
                "c1",
                &s1,
                &entries(&[(deploy - 60 * min, old), (deploy + min, old), (deploy + 2 * min, fresh), (deploy + 3 * min, fresh)]),
            )
            .unwrap();
        store.insert("c1", &src("shop_web", "1"), &entries(&[(deploy + 2 * min, "2026-10-27 ERROR web broke")])).unwrap();
        let n = store.new_since("c1", "shop_api", deploy, deploy + 10 * min).unwrap();
        assert_eq!(n.len(), 1, "{n:?}");
        assert_eq!((n[0].count, n[0].first), (2, deploy + 2 * min));
        assert!(n[0].sample.contains("orders.total"));
        // Before the next deploy only.
        assert!(store.new_since("c1", "shop_api", deploy, deploy + 2 * min).unwrap().is_empty());
        // Without lines from before the deploy nothing can be called new.
        assert!(store.new_since("c1", "shop_api", deploy - 50 * min, deploy + 10 * min).unwrap().is_empty());
    }

    #[test]
    fn a_service_that_stops_logging_goes_quiet() {
        let store = Store::open_in_memory().unwrap();
        let min = 60_000_000i64;
        let now = 1_700_000_000_000_000i64 / min * min + 30_000_000;
        let lines = |from: i64, to: i64, every: i64| -> Vec<(i64, String)> {
            (from..to).step_by(every as usize).map(|m| (now - m * min, format!("2026-10-27 INFO tick {m}"))).collect()
        };
        let put = |service: &str, l: Vec<(i64, String)>| {
            let l: Vec<(i64, &str)> = l.iter().map(|(t, s)| (*t, s.as_str())).collect();
            store.insert("c1", &src(service, "1"), &entries(&l)).unwrap();
        };
        // Every minute until 25 minutes ago: quiet.
        put("shop_hung", lines(25, 180, 1));
        // Every minute until now: fine.
        put("shop_busy", lines(0, 180, 1));
        // Every 5 minutes, last one 14 minutes ago: never logged steadily enough.
        put("shop_cron", lines(14, 180, 5));
        let s = store.error_summary("c1", now).unwrap();
        assert_eq!(s.quiet.len(), 1, "{:?}", s.quiet);
        assert_eq!(s.quiet[0].service, "shop_hung");
        assert_eq!(s.quiet[0].since, now - 30_000_000 - 24 * min);
        assert!(s.quiet[0].busy_minutes >= 100, "{:?}", s.quiet);
        // An hour earlier it still logged.
        assert!(store.error_summary("c1", now - 60 * min).unwrap().quiet.is_empty());
    }

    #[test]
    fn a_connection_that_reaches_another_host_drops_the_old_lines() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        let s1 = src("shop_api", "1");
        store.insert("c1", &s1, &entries(&[(now - 1_000_000, "2026-10-27 ERROR boom")])).unwrap();
        store.insert("c2", &s1, &entries(&[(now - 1_000_000, "2026-10-27 ERROR boom")])).unwrap();
        store.set_cursor("c1", &s1.key, now).unwrap();
        let all = LogFilter { from: 0, to: now, ..Default::default() };

        // The first host seen keeps what is stored.
        assert!(!store.bind_host("c1", "engine:A").unwrap());
        assert!(!store.bind_host("c1", "engine:A").unwrap());
        assert_eq!(store.query("c1", &all, 1, 10).unwrap().total, 1);

        // Another host under the same connection: its lines start from scratch.
        assert!(store.bind_host("c1", "engine:B").unwrap());
        assert_eq!(store.query("c1", &all, 1, 10).unwrap().total, 0);
        assert_eq!(store.cursor("c1", &s1.key).unwrap(), None);
        assert_eq!(store.query("c2", &all, 1, 10).unwrap().total, 1, "other connections are untouched");
    }

    #[test]
    fn error_rate_counts_the_window_and_names_the_top_error() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        let min = 60_000_000i64;
        let s1 = src("shop_api", "1");
        let a = "2026-10-27 ERROR Cannot connect to redis:6379";
        let b = "2026-10-27 ERROR Upstream timed out after 5000ms";
        store
            .insert(
                "c1",
                &s1,
                &entries(&[
                    (now - 30 * min, a),
                    (now - 4 * min, a),
                    (now - 3 * min, b),
                    (now - 2 * min, a),
                    (now - min, "2026-10-27 INFO ok"),
                ]),
            )
            .unwrap();
        let r = store.error_rate("c1", now - 5 * min, now).unwrap();
        assert_eq!(r.count, 3);
        assert!(r.top.as_deref().unwrap_or("").contains("redis"), "{r:?}");
        assert_eq!(r.top_service.as_deref(), Some("shop_api"));
        assert_eq!(store.error_rate("c1", now - min / 2, now).unwrap().count, 0);
    }

    #[test]
    fn the_size_limit_is_kept_and_space_goes_back_to_the_disk() {
        let dir = std::env::temp_dir().join(format!("stackline-retain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logs.db");
        // A store as older versions made it: auto_vacuum could not take effect after WAL was on.
        Connection::open(&path).unwrap().execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE x (a); DROP TABLE x;").unwrap();
        let store = Store::open(&path).unwrap();
        let now = 1_700_000_000_000_000i64;
        let s1 = src("shop_api", "1");
        let text = "2026-10-27 INFO ".to_string() + &"chatty ".repeat(60);
        for chunk in 0..20 {
            let lines: Vec<(i64, String)> = (0..1000).map(|i| (now + (chunk * 1000 + i) * 1000, format!("{text}{i}"))).collect();
            let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
            store.insert("c1", &s1, &entries(&refs)).unwrap();
        }
        let size = |p: &std::path::Path| std::fs::metadata(p).unwrap().len();
        let before = live_bytes(&store.db.lock().unwrap()).unwrap();
        let max = before / 3;
        store.retain(0, max).unwrap();
        let db = store.db.lock().unwrap();
        assert_eq!(db.query_row("PRAGMA auto_vacuum", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
        let live = live_bytes(&db).unwrap();
        assert!(live <= max && live > max / 2, "live {live}, limit {max}");
        assert!(store.used_bytes(&db).unwrap() <= max + max / 4, "on disk {} > limit {max}", store.used_bytes(&db).unwrap());
        drop(db);
        assert!(size(&path) <= max + max / 4, "file {} > limit {max}", size(&path));
        // The newest lines are the ones kept.
        let all = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        let r = store.query("c1", &all, 1, 1).unwrap();
        assert!(r.total > 0 && r.total < 20_000);
        assert_eq!(r.entries[0].ts, now + 19_999 * 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_lines_shrinks_the_file() {
        let dir = std::env::temp_dir().join(format!("stackline-shrink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logs.db");
        let store = Store::open(&path).unwrap();
        let now = 1_700_000_000_000_000i64;
        let text = "2026-10-27 INFO ".to_string() + &"chatty ".repeat(60);
        let fill = |svc: &str| {
            for chunk in 0..10 {
                let lines: Vec<(i64, String)> = (0..1000).map(|i| (now + (chunk * 1000 + i) * 1000, format!("{text}{i}"))).collect();
                let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
                store.insert("c1", &src(svc, "1"), &entries(&refs)).unwrap();
            }
        };
        fill("shop_api");
        fill("shop_web");
        let on_disk = || store.stats().unwrap().bytes;
        let full = on_disk();
        store.clear_service("c1", "shop_api").unwrap();
        let half = on_disk();
        assert!(half < full * 2 / 3, "{half} of {full} after deleting half the lines");
        // Free pages that older versions left behind go back at the next retention run.
        fill("shop_db");
        store.db.lock().unwrap().execute("DELETE FROM entries WHERE service = 'shop_db'", []).unwrap();
        let stale = on_disk();
        store.retain(0, u64::MAX).unwrap();
        assert!(on_disk() < stale * 4 / 5, "{} of {stale} after the retention run", on_disk());
        store.clear(None).unwrap();
        assert!(on_disk() < 1_000_000, "{} left after clearing everything", on_disk());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_noisy_service_never_pushes_the_store_past_its_limit() {
        // One service writes several times the limit, with stack traces and
        // varied text the search index cannot compress, while a quiet one
        // keeps logging. Retention runs after each round, as the runtime does
        // every minute, and the file on disk (what Preferences shows) must
        // stay under the limit every time.
        let dir = std::env::temp_dir().join(format!("stackline-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("logs.db")).unwrap();
        const LIMIT: u64 = 4 << 20;
        let now = 1_700_000_000_000_000i64;
        let mut t = now;
        let mut written = 0u64;
        for round in 0..20i64 {
            for burst in 0..4 {
                let mut lines = vec![];
                for i in 0..1500 {
                    let n = round * 6000 + burst * 1500 + i;
                    t += 1000;
                    if n % 10 == 0 {
                        lines.push((
                            t,
                            format!("2026-10-27 12:00:00.000 ERROR 1 --- [exec-{}] c.a.Checkout : Checkout failed for cart {n:x}", n % 8),
                        ));
                        lines.push((t, format!("java.lang.IllegalStateException: cart {:x} has no owner", n * 2654435761)));
                        for f in 0..6 {
                            lines.push((t, format!("\tat com.acme.shop.Layer{f}.call{}(Layer{f}.java:{})", n % 97, n % 400 + f)));
                        }
                    } else {
                        lines.push((
                            t,
                            format!(
                                "2026-10-27 12:00:00.000  INFO 1 --- [exec-{}] c.a.Api : GET /api/items/{n} 200 id={:016x} trace={:x}",
                                n % 8,
                                n * 2654435761,
                                n * 40503
                            ),
                        ));
                    }
                }
                written += lines.iter().map(|(_, l)| l.len() as u64).sum::<u64>();
                let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
                store.insert("c1", &src("shop_api", "1"), &entries(&refs)).unwrap();
                store
                    .insert("c1", &src("shop_web", "1"), &entries(&[(t, "2026-10-27 12:00:00.000  INFO 1 --- [main] c.a.Web : ok")]))
                    .unwrap();
            }
            // The open Logs page reads while lines come in.
            store.query("c1", &LogFilter { from: now, to: t, ..Default::default() }, 60, 300).unwrap();
            store.retain(0, LIMIT).unwrap();
            let s = store.stats().unwrap();
            assert!(s.bytes <= LIMIT, "round {round}: {} KB on disk, limit {} KB", s.bytes >> 10, LIMIT >> 10);
            assert!(s.entries > 0, "round {round}: retention deleted everything");
        }
        assert!(written > LIMIT * 3, "the service wrote only {} KB", written >> 10);
        // The newest lines stay, the oldest went.
        let r = store.query("c1", &LogFilter { from: 0, to: i64::MAX, ..Default::default() }, 1, 1).unwrap();
        assert_eq!(r.entries[0].ts, t);
        assert!(store.stats().unwrap().oldest.unwrap() > now + 1000 * 6000);
        // Search still finds recent lines and none of the deleted ones.
        let mut f = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        f.apply_query(&format!("id={:016x}", 119_999 * 2654435761u64));
        assert_eq!(store.query("c1", &f, 1, 1).unwrap().total, 1);
        let mut f = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        f.apply_query(&format!("id={:016x}", 2654435761u64));
        assert_eq!(store.query("c1", &f, 1, 1).unwrap().total, 0);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_keeps_the_search_index_in_step() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        let lines: Vec<(i64, String)> = (0..12_000).map(|i| (now + i, format!("2026-10-27 INFO order {i} paid"))).collect();
        let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
        store.insert("c1", &src("shop_api", "1"), &entries(&refs)).unwrap();
        store.insert("c1", &src("shop_web", "1"), &entries(&[(now, "2026-10-27 INFO web paid")])).unwrap();
        store.insert("c2", &src("shop_api", "1"), &entries(&[(now, "2026-10-27 INFO other paid")])).unwrap();
        let search = |conn: &str| {
            let mut f = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
            f.apply_query("paid");
            store.query(conn, &f, 1, 1).unwrap().total
        };
        // One service, across several batches.
        assert_eq!(store.clear_service("c1", "shop_api").unwrap(), 12_000);
        assert_eq!((search("c1"), search("c2")), (1, 1));
        // One connection of two goes batch by batch; the last one empties everything at once.
        store.clear(Some("c1")).unwrap();
        assert_eq!((search("c1"), search("c2")), (0, 1));
        store.clear(None).unwrap();
        assert_eq!(search("c2"), 0);
        // New lines are indexed and deletable as before.
        store.insert("c2", &src("shop_api", "1"), &entries(&[(now, "2026-10-27 INFO again paid")])).unwrap();
        assert_eq!(search("c2"), 1);
        assert_eq!(store.clear_service("c2", "shop_api").unwrap(), 1);
        assert_eq!(search("c2"), 0);
    }

    #[test]
    fn usage_and_clearing_one_service() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        store.insert("c1", &src("shop_api", "1"), &entries(&[(now, "2026-10-27 INFO a"), (now + 1, "2026-10-27 INFO b")])).unwrap();
        store.insert("c1", &src("shop_web", "1"), &entries(&[(now, "2026-10-27 INFO c")])).unwrap();
        let u = store.usage("c1").unwrap();
        assert_eq!((u[0].service.as_str(), u[0].entries), ("shop_api", 2));
        assert_eq!(store.clear_service("c1", "shop_api").unwrap(), 2);
        let u = store.usage("c1").unwrap();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].service, "shop_web");
    }

    #[test]
    #[ignore]
    /// `cargo test --release -p stackline-core bench_retain_large -- --ignored --nocapture`
    fn bench_retain_large() {
        let dir = std::env::temp_dir().join("stackline-bench-retain");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("logs.db")).unwrap();
        let now = 1_700_000_000_000_000i64;
        let words = ["GET", "/api/orders", "200", "user", "cart", "checkout", "redis", "timeout", "INFO", "worker", "job", "queue"];
        let start = std::time::Instant::now();
        for chunk in 0..120i64 {
            let lines: Vec<(i64, String)> = (0..2500i64)
                .map(|i| {
                    let n = chunk * 2500 + i;
                    let w: Vec<&str> = (0..14).map(|k| words[((n * 7 + k * 13) % 12) as usize]).collect();
                    (now + n * 1000, format!("2026-10-27 INFO req={n} {} id={:x}", w.join(" "), n * 2654435761))
                })
                .collect();
            let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
            store.insert("c1", &src(&format!("svc{}", chunk % 5), "1"), &entries(&refs)).unwrap();
        }
        let live = live_bytes(&store.db.lock().unwrap()).unwrap();
        eprintln!("insert 300k lines: {:?}, live {} MB", start.elapsed(), live >> 20);
        let t = std::time::Instant::now();
        store.usage("c1").unwrap();
        eprintln!("usage: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let n = store.clear_service("c1", "svc1").unwrap();
        eprintln!("clear_service {n} lines: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        store.retain(0, live / 10 * 8).unwrap();
        eprintln!("retain to 80%: {:?}, live {} MB", t.elapsed(), live_bytes(&store.db.lock().unwrap()).unwrap() >> 20);
        let t = std::time::Instant::now();
        store.retain(0, live).unwrap();
        eprintln!("retain no-op: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        store.clear(None).unwrap();
        eprintln!("clear all: {:?}", t.elapsed());
    }

    /// The per-minute counts and the per-service reads give the same answers
    /// as reading every line, also after lines were deleted.
    #[test]
    fn minute_counts_and_service_reads_match_full_reads() {
        let dir = std::env::temp_dir().join(format!("stackline-minutes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir.join("logs.db")).unwrap();
        store.upgrade().unwrap();
        let base = 1_700_000_000_000_000i64 / MINUTE * MINUTE;
        let kinds = ["INFO ok", "WARN slow call", "ERROR Upstream timed out after 5000ms", "DEBUG tick"];
        for svc in 0..6 {
            let mut s = src(&format!("svc{svc}"), "1");
            s.stack = format!("st{}", svc % 3);
            let lines: Vec<(i64, String)> = (0..3000i64)
                .map(|i| (base + ((i * 7919 + svc * 31) % 14_400) * 1_000_000 + i, format!("{} {i}", kinds[((i + svc) % 4) as usize])))
                .collect();
            let mut lines = lines;
            lines.sort();
            let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
            store.insert("c1", &s, &entries(&refs)).unwrap();
        }
        let check = |label: &str| {
            let db = store.db.lock().unwrap();
            let filters = [
                LogFilter { from: base + 17 * MINUTE + 1234, to: base + 230 * MINUTE - 99, ..Default::default() },
                LogFilter { from: base, to: base + 240 * MINUTE, levels: vec![Level::Error], ..Default::default() },
                LogFilter { from: base, to: base + 240 * MINUTE, exact_services: vec!["svc1".into(), "svc4".into()], ..Default::default() },
                LogFilter { from: base + 3, to: base + 200 * MINUTE, pick_stacks: vec!["st2".into()], ..Default::default() },
                LogFilter {
                    from: base,
                    to: base + 240 * MINUTE,
                    pick_stacks: vec!["st0".into()],
                    exact_services: vec!["svc1".into()],
                    levels: vec![Level::Warn, Level::Error],
                    ..Default::default()
                },
                LogFilter { from: base, to: base + 240 * MINUTE, services: vec!["vc5".into()], ..Default::default() },
            ];
            for f in &filters {
                // 48 buckets of 5 minutes: bucket edges fall on whole minutes.
                let fast = query(&db, "c1", f, 48, 300, 0, true).unwrap();
                let slow = query(&db, "c1", f, 48, 300, i64::MAX, false).unwrap();
                assert_eq!(fast.total, slow.total, "{label}: {f:?}");
                assert_eq!(serde_json::to_value(&fast.levels).unwrap(), serde_json::to_value(&slow.levels).unwrap(), "{label}: {f:?}");
                if f.from % MINUTE == 0 {
                    assert_eq!(
                        serde_json::to_value(&fast.histogram).unwrap(),
                        serde_json::to_value(&slow.histogram).unwrap(),
                        "{label}: {f:?}"
                    );
                }
                let ids = |r: &QueryResult| r.entries.iter().map(|e| e.id).collect::<Vec<_>>();
                assert_eq!(ids(&fast), ids(&slow), "{label}: {f:?}");
                assert!(fast.total > 0, "{label}: {f:?}");
            }
        };
        check("after insert");
        store.retain(base + 60 * MINUTE + 5, u64::MAX).unwrap();
        check("after age retention");
        store.clear_service("c1", "svc2").unwrap();
        check("after deleting a service");
        let live = live_bytes(&store.db.lock().unwrap()).unwrap();
        store.retain(0, live / 10 * 8).unwrap();
        check("after trimming to size");
        store.clear(None).unwrap();
        let db = store.db.lock().unwrap();
        let left: i64 = db.query_row("SELECT COUNT(*) FROM minutes", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
        drop(db);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store from before the per-minute counts: its lines are read one by
    /// one, newer lines are counted, and both together give the right answer.
    #[test]
    fn store_from_before_minute_counts() {
        let dir = std::env::temp_dir().join(format!("stackline-upgrade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("logs.db");
        let base = 1_700_000_000_000_000i64 / MINUTE * MINUTE;
        let lines = |from: i64, n: i64| -> Vec<(i64, String)> {
            (0..n).map(|i| (base + (from + i) * 7_000_000, format!("{} line {i}", ["INFO", "WARN", "ERROR"][(i % 3) as usize]))).collect()
        };
        {
            let store = Store::open(&path).unwrap();
            let old = lines(0, 1000);
            let refs: Vec<(i64, &str)> = old.iter().map(|(t, l)| (*t, l.as_str())).collect();
            store.insert("c1", &src("svc", "1"), &entries(&refs)).unwrap();
            // What 0.3 stores look like: no counts, no triggers for them.
            store
                .db
                .lock()
                .unwrap()
                .execute_batch(
                    "DROP TRIGGER entries_ai; DROP TRIGGER entries_ad; DROP TABLE minutes; DROP TABLE meta;
                     CREATE TRIGGER entries_ai AFTER INSERT ON entries BEGIN INSERT INTO entries_fts (rowid, text) VALUES (new.id, new.text); END;
                     CREATE TRIGGER entries_ad AFTER DELETE ON entries BEGIN INSERT INTO entries_fts (entries_fts, rowid, text) VALUES ('delete', old.id, old.text); END;",
                )
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        store.upgrade().unwrap();
        let since = store.minutes_since.load(std::sync::atomic::Ordering::Relaxed);
        assert!(since > base + 999 * 7_000_000, "counts start after the newest stored line");
        let new = lines(1000, 1000);
        let refs: Vec<(i64, &str)> = new.iter().map(|(t, l)| (*t, l.as_str())).collect();
        store.insert("c1", &src("svc", "1"), &entries(&refs)).unwrap();
        // 48 buckets of 5 minutes, so the counted minutes fall into the same buckets.
        let f = LogFilter { from: base, to: base + 240 * MINUTE, ..Default::default() };
        let r = store.query("c1", &f, 48, 10).unwrap();
        assert_eq!(r.total, 2000);
        assert_eq!((r.levels.info, r.levels.warn, r.levels.error), (668, 666, 666));
        let slow = query(&store.db.lock().unwrap(), "c1", &f, 48, 10, i64::MAX, false).unwrap();
        assert_eq!(serde_json::to_value(&r.histogram).unwrap(), serde_json::to_value(&slow.histogram).unwrap());
        // Reopening keeps where the counts start.
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.minutes_since.load(std::sync::atomic::Ordering::Relaxed), since);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A long read (an export) neither blocks new lines nor the cleanup.
    #[test]
    fn long_reads_do_not_hold_up_writes() {
        let dir = std::env::temp_dir().join(format!("stackline-reads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = std::sync::Arc::new(Store::open(&dir.join("logs.db")).unwrap());
        let base = 1_700_000_000_000_000i64;
        let lines: Vec<(i64, String)> = (0..5000).map(|i| (base + i * 1000, format!("INFO line {i}"))).collect();
        let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
        store.insert("c1", &src("svc", "1"), &entries(&refs)).unwrap();

        // An export to a slow reader: it stays inside its read for over a second.
        struct Slow(usize);
        impl Write for Slow {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                if self.0.is_multiple_of(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let reader = {
            let store = store.clone();
            std::thread::spawn(move || {
                let f = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
                store.export("c1", &f, ExportFormat::Log, &mut Slow(0)).unwrap()
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t = std::time::Instant::now();
        store.insert("c1", &src("svc", "1"), &entries(&[(base + 10_000_000, "INFO new")])).unwrap();
        store.retain(base + 1000, u64::MAX).unwrap();
        // The export still running proves the writes did not wait for it; the
        // time bound is loose because CI runners are sometimes slow.
        assert!(!reader.is_finished(), "the writes waited for the export ({:?})", t.elapsed());
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
        assert_eq!(reader.join().unwrap(), 5000);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
