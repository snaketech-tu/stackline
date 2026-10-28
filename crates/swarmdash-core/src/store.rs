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
use crate::logs::{Level, LogEntry};
use crate::model::LogSource;

pub struct Store {
    db: Mutex<Connection>,
    /// The database file, to count its write-ahead log too. None in memory.
    path: Option<std::path::PathBuf>,
}

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
CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts (entries_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TABLE IF NOT EXISTS patterns (conn TEXT NOT NULL, fp TEXT NOT NULL, pattern TEXT NOT NULL, PRIMARY KEY (conn, fp));
CREATE TABLE IF NOT EXISTS cursors (conn TEXT NOT NULL, source TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (conn, source));
CREATE TABLE IF NOT EXISTS hosts (conn TEXT PRIMARY KEY, identity TEXT NOT NULL);
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
        let mut w = vec!["conn = ?".to_string(), "ts >= ?".to_string(), "ts < ?".to_string()];
        let mut p: Vec<Sql> = vec![Sql::Text(conn.into()), Sql::Integer(self.from), Sql::Integer(self.to)];
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
        Ok(Store { db: Mutex::new(db), path })
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
            for e in entries {
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

    pub fn query(&self, conn: &str, f: &LogFilter, buckets: usize, limit: usize) -> Result<QueryResult> {
        let db = self.db.lock().unwrap();
        let buckets = buckets.clamp(1, 500) as i64;
        let span = (f.to - f.from).max(1);

        // One pass over (ts, level, exc), read from the covering index, gives the level
        // counts (which ignore the level filter, so the chips show what each would add)
        // and the histogram. Bucketing here is about ten times faster than a GROUP BY,
        // which makes SQLite sort every row of the range.
        let (w_all, p_all) = f.where_clause(conn, false);
        let wanted = |l: Level| !f.no_levels && (f.levels.is_empty() || f.levels.contains(&l));
        let mut levels = LevelCounts::default();
        let mut histogram = vec![Bucket::default(); buckets as usize];
        let mut total = 0u64;
        let mut st = db.prepare(&format!("SELECT ts, level, exc FROM entries WHERE {w_all}"))?;
        let mut rows = st.query(params_from_iter(p_all.iter()))?;
        while let Some(r) = rows.next()? {
            let (ts, level, exc): (i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            let level = Level::from_i64(level);
            *levels.get_mut(level) += 1;
            if !wanted(level) {
                continue;
            }
            let bk = &mut histogram[((ts - f.from) * buckets / span).clamp(0, buckets - 1) as usize];
            total += 1;
            bk.exc += (exc != 0) as u64;
            *bk.get_mut(level) += 1;
        }
        drop(rows);
        drop(st);

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

        let mut st = db.prepare(&format!(
            "SELECT id, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw
             FROM entries WHERE {w} ORDER BY ts DESC, id DESC LIMIT {}",
            limit.clamp(1, 5000)
        ))?;
        let entries = st.query_map(params_from_iter(p.iter()), row_to_entry)?.collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(QueryResult { total, levels, histogram, groups, entries })
    }

    pub fn error_summary(&self, conn: &str, now: i64) -> Result<ErrorSummary> {
        const HOUR: i64 = 3_600_000_000;
        let db = self.db.lock().unwrap();
        let err = Level::Error.as_i64();
        let mut by_service = HashMap::new();
        let mut st = db.prepare_cached(
            "SELECT service, COUNT(*) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 GROUP BY service",
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
        let new_groups = new_groups(&db, conn, now, oldest)?;
        Ok(ErrorSummary { by_service, last_hour, prev_hour, spark, oldest, new_groups })
    }

    /// Writes every matching entry, oldest first. Returns the number written.
    pub fn export(&self, conn: &str, f: &LogFilter, format: ExportFormat, out: &mut dyn Write) -> Result<u64> {
        let db = self.db.lock().unwrap();
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
        }
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
        let db = self.db.lock().unwrap();
        let mode: i64 = db.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        if mode != 2 {
            // Stores made before 0.3 could never shrink: switch them over once.
            tracing::info!("rebuilding the log store once so it can give space back");
            db.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")?;
        } else if deleted > 0 {
            db.execute_batch("PRAGMA incremental_vacuum;")?;
        }
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(deleted)
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
    /// It reads every line, so on a file store it uses its own read-only
    /// connection: thanks to WAL it does not hold up new lines or queries.
    pub fn usage(&self, conn: &str) -> Result<Vec<ServiceUsage>> {
        match &self.path {
            Some(p) => usage(&Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?, conn),
            None => usage(&self.db.lock().unwrap(), conn),
        }
    }

    /// Deletes the stored lines of one service, in batches.
    pub fn clear_service(&self, conn: &str, service: &str) -> Result<u64> {
        let mut deleted = 0u64;
        loop {
            let n = self.db.lock().unwrap().execute(
                "DELETE FROM entries WHERE id IN (SELECT id FROM entries WHERE conn = ?1 AND service = ?2 LIMIT 20000)",
                params![conn, service],
            )?;
            deleted += n as u64;
            if n < 20000 {
                break;
            }
        }
        if deleted > 0 {
            self.merge_index()?;
            self.db.lock().unwrap().execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(deleted)
    }

    /// Counts the error entries of a connection in `[from, to)` (µs).
    pub fn error_rate(&self, conn: &str, from: i64, to: i64) -> Result<ErrorRate> {
        let db = self.db.lock().unwrap();
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
        let top: Option<(String, String, i64)> = db
            .query_row(
                "SELECT fp, service, MAX(id) FROM entries WHERE conn = ?1 AND ts >= ?2 AND ts < ?3 AND level = ?4 AND fp IS NOT NULL
                 GROUP BY fp ORDER BY COUNT(*) DESC LIMIT 1",
                params![conn, from, to, err],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((fp, service, id)) = top {
            let pattern: Option<String> =
                db.query_row("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2", params![conn, fp], |r| r.get(0)).optional()?;
            let text = match pattern.filter(|p| !p.is_empty()) {
                Some(p) => p,
                None => db.query_row("SELECT text FROM entries WHERE id = ?1", params![id], |r| r.get::<_, String>(0))?,
            };
            rate.top = Some(text.lines().next().unwrap_or("").chars().take(200).collect());
            rate.top_service = Some(service);
        }
        Ok(rate)
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
        if changed {
            db.execute("DELETE FROM entries WHERE conn = ?1", params![conn])?;
            db.execute("DELETE FROM patterns WHERE conn = ?1", params![conn])?;
            db.execute("DELETE FROM cursors WHERE conn = ?1", params![conn])?;
        }
        db.execute(
            "INSERT INTO hosts (conn, identity) VALUES (?1, ?2) ON CONFLICT (conn) DO UPDATE SET identity = excluded.identity",
            params![conn, identity],
        )?;
        Ok(changed)
    }

    pub fn stats(&self) -> Result<StoreStats> {
        let db = self.db.lock().unwrap();
        let entries = db.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        let mut oldest: Option<i64> = None;
        for c in conns(&db)? {
            let t: Option<i64> = db.query_row("SELECT MIN(ts) FROM entries WHERE conn = ?1", params![c], |r| r.get(0))?;
            oldest = oldest.into_iter().chain(t).min();
        }
        Ok(StoreStats { bytes: self.used_bytes(&db)?, entries, oldest })
    }

    pub fn clear(&self, conn: Option<&str>) -> Result<()> {
        let db = self.db.lock().unwrap();
        match conn {
            Some(c) => {
                db.execute("DELETE FROM entries WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM patterns WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM cursors WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM hosts WHERE conn = ?1", params![c])?;
            }
            None => db.execute_batch("DELETE FROM entries; DELETE FROM patterns; DELETE FROM cursors; DELETE FROM hosts;")?,
        }
        db.execute_batch("PRAGMA incremental_vacuum;")?;
        Ok(())
    }
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
        let dir = std::env::temp_dir().join(format!("swarmdash-retain-{}", std::process::id()));
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
    /// `cargo test --release -p swarmdash-core bench_retain_large -- --ignored --nocapture`
    fn bench_retain_large() {
        let dir = std::env::temp_dir().join("swarmdash-bench-retain");
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
        store.retain(0, live / 10 * 8).unwrap();
        eprintln!("retain to 80%: {:?}, live {} MB", t.elapsed(), live_bytes(&store.db.lock().unwrap()).unwrap() >> 20);
        let t = std::time::Instant::now();
        store.retain(0, live).unwrap();
        eprintln!("retain no-op: {:?}", t.elapsed());
    }
}
