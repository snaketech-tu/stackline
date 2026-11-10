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

mod export;
mod query;
mod retention;
mod schema;

use export::*;
use query::*;
use retention::*;
use schema::*;

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

/// Remembers how far a source has been read.
const SET_CURSOR: &str = "INSERT INTO cursors (conn, source, ts) VALUES (?1, ?2, ?3)
     ON CONFLICT (conn, source) DO UPDATE SET ts = excluded.ts";

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
    /// The range ends now: lines stamped after `to` count too. The remote
    /// host's clock can be ahead of this computer's, and its newest lines
    /// would otherwise be left out until this clock caught up.
    pub open_end: bool,
}

/// Where a page of lines starts: lines older (`Before`) or newer (`After`)
/// than this line, by (ts, id) as the stream is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Before(i64, i64),
    After(i64, i64),
}

impl Page {
    /// Reads `before=<ts>:<id>` or `after=<ts>:<id>`.
    pub fn parse(before: Option<&str>, after: Option<&str>) -> Option<Page> {
        let key = |v: &str| {
            let (ts, id) = v.split_once(':')?;
            Some((ts.trim().parse().ok()?, id.trim().parse().ok()?))
        };
        if let Some((ts, id)) = before.and_then(key) {
            return Some(Page::Before(ts, id));
        }
        after.and_then(key).map(|(ts, id)| Page::After(ts, id))
    }
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
        // An open range keeps no upper bound where it ends.
        let to = if self.open_end && to >= self.to { i64::MAX } else { to };
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

/// What [`Store::check_host`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostCheck {
    /// The host the stored lines come from.
    Same,
    /// The first host seen for the connection; now remembered.
    First,
    /// Another host than the stored lines come from.
    Changed,
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
    /// More lines match beyond `entries` (older ones, or for an `After` page
    /// lines between it and the newest). Counted, unlike `total`, from the
    /// lines themselves, so a "Show more" is never missing.
    pub more: bool,
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
        insert_entries(&tx, conn, src, entries)?;
        tx.commit()?;
        Ok(())
    }

    /// Stores lines of a read and moves the source's cursor in one step,
    /// unless the lines of the connection were cleared since `epoch` (see
    /// [`Store::epoch`]): then nothing is stored and false comes back. The
    /// check holds the write lock, so a clear either comes first and the
    /// batch is dropped, or comes after and deletes it with the rest.
    pub fn store_batch(&self, conn: &str, src: &LogSource, entries: &[LogEntry], cursor: Option<i64>, epoch: u64) -> Result<bool> {
        let mut db = self.db.lock().unwrap();
        if self.epoch() != epoch {
            return Ok(false);
        }
        let tx = db.transaction()?;
        insert_entries(&tx, conn, src, entries)?;
        if let Some(ts) = cursor {
            tx.execute(SET_CURSOR, params![conn, src.key, ts])?;
        }
        tx.commit()?;
        Ok(true)
    }
}

/// Writes lines, their error patterns and their per-minute counts.
fn insert_entries(tx: &rusqlite::Transaction<'_>, conn: &str, src: &LogSource, entries: &[LogEntry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
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
    Ok(())
}

impl Store {
    pub fn cursor(&self, conn: &str, source: &str) -> Result<Option<i64>> {
        let db = self.db.lock().unwrap();
        Ok(db.query_row("SELECT ts FROM cursors WHERE conn = ?1 AND source = ?2", params![conn, source], |r| r.get(0)).optional()?)
    }

    pub fn set_cursor(&self, conn: &str, source: &str, ts: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        db.execute(SET_CURSOR, params![conn, source, ts])?;
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
        let epoch = self.epoch();
        Ok(self.store_events(conn, events, None, epoch)?.unwrap_or(0))
    }

    /// Like [`Store::insert_events`], and moves the event cursor to `cursor`
    /// in the same step, unless the connection was cleared since `epoch`
    /// (then None comes back and nothing is stored).
    pub fn store_events(&self, conn: &str, events: &[Event], cursor: Option<i64>, epoch: u64) -> Result<Option<usize>> {
        let mut db = self.db.lock().unwrap();
        if self.epoch() != epoch {
            return Ok(None);
        }
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
        if let Some(ts) = cursor {
            tx.execute(SET_CURSOR, params![conn, crate::events::CURSOR, ts])?;
        }
        tx.commit()?;
        Ok(Some(n))
    }

    /// The newest events in the filter's time range and sources, oldest
    /// first. Levels, error groups and search words do not apply to events.
    pub fn events(&self, conn: &str, f: &LogFilter, limit: usize) -> Result<Vec<Event>> {
        self.events_of(conn, f, &[], limit)
    }

    /// Like [`Store::events`], only events of these kinds (all when empty).
    pub fn events_of(&self, conn: &str, f: &LogFilter, kinds: &[Kind], limit: usize) -> Result<Vec<Event>> {
        let f = LogFilter { fps: vec![], text: vec![], levels: vec![], no_levels: false, ..f.clone() };
        let (mut w, mut p) = f.where_clause(conn, false);
        if !kinds.is_empty() {
            w.push_str(&format!(" AND kind IN ({})", vec!["?"; kinds.len()].join(",")));
            p.extend(kinds.iter().map(|k| Sql::Text(k.as_str().into())));
        }
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
        self.query_page(conn, f, buckets, limit, None)
    }

    /// Like [`Store::query`], with the lines of one page: older or newer than a line.
    pub fn query_page(&self, conn: &str, f: &LogFilter, buckets: usize, limit: usize, page: Option<Page>) -> Result<QueryResult> {
        let since = self.minutes_since.load(std::sync::atomic::Ordering::Relaxed);
        let indexed = self.service_index.load(std::sync::atomic::Ordering::Relaxed);
        self.read(|db| query(db, conn, f, buckets, limit, since, indexed, page))
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
    fn pages_walk_the_whole_range_without_gaps_or_repeats() {
        let store = Store::open_in_memory().unwrap();
        let base = 1_700_000_000_000_000i64;
        // Two services, and lines that share a microsecond, as a stack trace does.
        for (svc, off) in [("shop_app", 0), ("shop_db", 3)] {
            let lines: Vec<(i64, String)> =
                (0..1234).map(|i| (base + (i / 4) * 1000 + off, format!("2026-10-27 INFO {svc} line {i}"))).collect();
            let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
            // One entry per line: the assembler would join same-time lines.
            let mut es = vec![];
            for (t, l) in refs {
                es.extend(entries(&[(t, l)]));
            }
            store.insert("c1", &src(svc, "1"), &es).unwrap();
        }
        for picked in [vec![], vec!["shop_app".to_string(), "shop_db".to_string()]] {
            let f = LogFilter { from: base, to: base + 1_000_000_000, exact_services: picked.clone(), ..Default::default() };
            let first = store.query("c1", &f, 10, 300).unwrap();
            assert_eq!((first.entries.len(), first.more, first.total), (300, true, 2468));
            let mut seen: Vec<(i64, i64)> = first.entries.iter().map(|e| (e.ts, e.id)).collect();
            let mut more = first.more;
            while more {
                let &(ts, id) = seen.last().unwrap();
                let r = store.query_page("c1", &f, 10, 500, Some(Page::Before(ts, id))).unwrap();
                assert!(r.groups.is_empty() && r.total == 0, "an older page carries only lines");
                seen.extend(r.entries.iter().map(|e| (e.ts, e.id)));
                more = r.more;
            }
            assert_eq!(seen.len(), 2468, "{picked:?}");
            assert!(seen.windows(2).all(|w| w[0] > w[1]), "newest first, each line once");

            // The newest lines above a line: all of them, or `more` when they do not fit.
            let (ts, id) = seen[700];
            let r = store.query_page("c1", &f, 10, 5000, Some(Page::After(ts, id))).unwrap();
            assert_eq!((r.entries.len(), r.more), (700, false));
            let r = store.query_page("c1", &f, 10, 100, Some(Page::After(ts, id))).unwrap();
            assert_eq!((r.entries.len(), r.more), (100, true));
        }
        // The last page says there is nothing more.
        let f = LogFilter { from: base, to: base + 1_000_000_000, ..Default::default() };
        assert!(!store.query("c1", &f, 10, 5000).unwrap().more);
    }

    #[test]
    fn an_open_range_keeps_lines_stamped_after_its_end() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        // The host's clock is 2 minutes ahead of this one.
        store.insert("c1", &src("shop_app", "1"), &entries(&[(now + 120_000_000, "2026-10-27 ERROR from the future")])).unwrap();
        let mut f = LogFilter { from: now - 3_600_000_000, to: now, ..Default::default() };
        assert_eq!(store.query("c1", &f, 10, 10).unwrap().entries.len(), 0);
        f.open_end = true;
        let r = store.query("c1", &f, 10, 10).unwrap();
        assert_eq!((r.entries.len(), r.total, r.levels.error, r.groups.len()), (1, 1, 1, 1));
        assert_eq!(r.histogram.last().unwrap().error, 1, "counted in the last bucket");
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
    fn csv_cells_never_start_a_formula() {
        assert_eq!(csv("=HYPERLINK(\"http://x\")"), "\"'=HYPERLINK(\"\"http://x\"\")\"");
        assert_eq!(csv("-1 retries left"), "'-1 retries left");
        assert_eq!(csv("@admin logged in"), "'@admin logged in");
        assert_eq!(csv("plain, text"), "\"plain, text\"");
        assert_eq!(csv("ok"), "ok");
    }

    #[test]
    fn deploys_are_found_among_many_other_events() {
        let store = Store::open_in_memory().unwrap();
        let ev = |key: String, ts: i64, kind: Kind| Event {
            key,
            ts,
            stack: "demo".into(),
            service: "demo_app".into(),
            replica: "1".into(),
            node: "box".into(),
            source: "c0".into(),
            kind,
            exit_code: None,
            detail: None,
        };
        let mut batch = vec![ev("deploy".into(), 10, Kind::Deploy)];
        batch.extend((0..6000).map(|i| ev(format!("crash{i}"), 100 + i, Kind::Crash)));
        store.insert_events("c1", &batch).unwrap();
        let all = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        let deploys = store.events_of("c1", &all, &[Kind::Deploy], 50).unwrap();
        assert_eq!(deploys.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), ["deploy"]);
        assert_eq!(store.events_of("c1", &all, &[], 10).unwrap().len(), 10);
    }

    #[test]
    fn a_batch_read_before_a_clear_is_not_stored_after_it() {
        let store = Store::open_in_memory().unwrap();
        let epoch = store.epoch();
        assert!(store.store_batch("c1", &src("shop_api", "1"), &entries(&[(10, "before")]), Some(10), epoch).unwrap());
        // "Clear stored logs" while a read of the same source is under way.
        store.clear(Some("c1")).unwrap();
        let stored = store.store_batch("c1", &src("shop_api", "1"), &entries(&[(20, "read before the clear")]), Some(20), epoch).unwrap();
        assert!(!stored);
        let all = LogFilter { from: 0, to: 100, ..Default::default() };
        assert_eq!(store.query("c1", &all, 1, 10).unwrap().entries.len(), 0);
        // No cursor either, so the next read starts over and fills the range again.
        assert_eq!(store.cursor("c1", "shop_api-1").unwrap(), None);
        assert_eq!(store.store_events("c1", &[], Some(20), epoch).unwrap(), None);
        assert_eq!(store.cursor("c1", crate::events::CURSOR).unwrap(), None);
    }

    #[test]
    fn a_chatty_connection_does_not_push_out_a_quiet_one() {
        assert_eq!(over_share(&[("a".into(), 900), ("b".into(), 100)], 500), [("a".to_string(), 500)]);
        assert_eq!(over_share(&[("a".into(), 900), ("b".into(), 300)], 400), [("b".to_string(), 100), ("a".to_string(), 700)]);
        let dir = std::env::temp_dir().join(format!("stackline-fair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("logs.db")).unwrap();
        // The quiet connection logged first, long ago; the chatty one fills the store.
        store.insert("quiet", &src("db", "1"), &entries(&[(1, "the quiet server's oldest line")])).unwrap();
        let mut t = 1_000;
        for round in 0..40 {
            let lines: Vec<(i64, String)> = (0..2000)
                .map(|i| {
                    t += 1;
                    (t, format!("GET /api/items/{round}-{i} 200 id={:016x}", (round * 2000 + i) as u64 * 2654435761))
                })
                .collect();
            let refs: Vec<(i64, &str)> = lines.iter().map(|(t, l)| (*t, l.as_str())).collect();
            store.insert("chatty", &src("api", "1"), &entries(&refs)).unwrap();
        }
        store.retain(0, 2 << 20).unwrap();
        let all = LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        let quiet = store.query("quiet", &all, 1, 10).unwrap();
        assert_eq!(quiet.entries.len(), 1, "the quiet server lost its history");
        let chatty = store.query("chatty", &all, 1, 1).unwrap();
        assert!(chatty.total < 80_000 && chatty.entries[0].ts == t, "the chatty server keeps its newest lines");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
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
    fn another_host_keeps_the_old_lines_until_the_user_decides() {
        let store = Store::open_in_memory().unwrap();
        let now = 1_700_000_000_000_000i64;
        let s1 = src("shop_api", "1");
        store.insert("c1", &s1, &entries(&[(now - 1_000_000, "2026-10-27 ERROR boom")])).unwrap();
        store.insert("c2", &s1, &entries(&[(now - 1_000_000, "2026-10-27 ERROR boom")])).unwrap();
        store.set_cursor("c1", &s1.key, now).unwrap();
        let all = LogFilter { from: 0, to: now, ..Default::default() };
        let total = |c: &str| store.query(c, &all, 1, 10).unwrap().total;

        // The first host seen keeps what is stored.
        assert_eq!(store.check_host("c1", "engine:A").unwrap(), HostCheck::First);
        assert_eq!(store.check_host("c1", "engine:A").unwrap(), HostCheck::Same);
        assert_eq!(total("c1"), 1);

        // Another host under the same connection: reported, nothing deleted.
        assert_eq!(store.check_host("c1", "engine:B").unwrap(), HostCheck::Changed);
        assert_eq!(store.check_host("c1", "engine:B").unwrap(), HostCheck::Changed);
        assert_eq!(total("c1"), 1);

        // "Keep them": the new host is remembered, the old lines stay.
        store.accept_host("c1", "engine:B", true).unwrap();
        assert_eq!(store.check_host("c1", "engine:B").unwrap(), HostCheck::Same);
        assert_eq!(total("c1"), 1);

        // "Delete them": lines and cursors of that connection go, others stay.
        assert_eq!(store.check_host("c1", "engine:C").unwrap(), HostCheck::Changed);
        store.accept_host("c1", "engine:C", false).unwrap();
        assert_eq!(store.check_host("c1", "engine:C").unwrap(), HostCheck::Same);
        assert_eq!(total("c1"), 0);
        assert_eq!(store.cursor("c1", &s1.key).unwrap(), None);
        assert_eq!(total("c2"), 1, "other connections are untouched");
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
                let fast = query(&db, "c1", f, 48, 300, 0, true, None).unwrap();
                let slow = query(&db, "c1", f, 48, 300, i64::MAX, false, None).unwrap();
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
        let slow = query(&store.db.lock().unwrap(), "c1", &f, 48, 10, i64::MAX, false, None).unwrap();
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
