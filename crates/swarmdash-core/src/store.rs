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
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA auto_vacuum = INCREMENTAL;
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
CREATE INDEX IF NOT EXISTS entries_conn_ts ON entries (conn, ts);
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
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Store> {
        db.execute_batch(SCHEMA)?;
        Ok(Store { db: Mutex::new(db) })
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
            let mut pat = tx.prepare_cached("INSERT OR REPLACE INTO patterns (conn, fp, pattern) VALUES (?1, ?2, ?3)")?;
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

        // Level counts ignore the level filter, so the chips show what each would add.
        let (w_all, p_all) = f.where_clause(conn, false);
        let mut levels = LevelCounts::default();
        let mut st = db.prepare(&format!("SELECT level, COUNT(*) FROM entries WHERE {w_all} GROUP BY level"))?;
        let rows = st.query_map(params_from_iter(p_all.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, u64>(1)?)))?;
        for row in rows {
            let (l, n) = row?;
            match Level::from_i64(l) {
                Level::Error => levels.error += n,
                Level::Warn => levels.warn += n,
                Level::Info => levels.info += n,
                Level::Debug => levels.debug += n,
            }
        }

        let (w, p) = f.where_clause(conn, true);
        let mut histogram = vec![Bucket::default(); buckets as usize];
        let mut st =
            db.prepare(&format!("SELECT (ts - ?) * ? / ? AS b, level, COUNT(*), SUM(exc) FROM entries WHERE {w} GROUP BY b, level"))?;
        let mut hp = vec![Sql::Integer(f.from), Sql::Integer(buckets), Sql::Integer(span)];
        hp.extend(p.iter().cloned());
        let mut total = 0u64;
        let rows = st.query_map(params_from_iter(hp.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, u64>(2)?, r.get::<_, u64>(3)?))
        })?;
        for row in rows {
            let (b, l, n, exc) = row?;
            let Some(bk) = histogram.get_mut(b.clamp(0, buckets - 1) as usize) else { continue };
            total += n;
            bk.exc += exc;
            match Level::from_i64(l) {
                Level::Error => bk.error += n,
                Level::Warn => bk.warn += n,
                Level::Info => bk.info += n,
                Level::Debug => bk.debug += n,
            }
        }

        let mut groups = Vec::new();
        let mut st = db.prepare(&format!(
            "SELECT fp, COUNT(*), MAX(level), MIN(ts), MAX(ts), GROUP_CONCAT(DISTINCT service), GROUP_CONCAT(DISTINCT node),
                    COUNT(DISTINCT source), MAX(exc), MAX(id)
             FROM entries WHERE {w} AND fp IS NOT NULL AND level >= 3 GROUP BY fp ORDER BY COUNT(*) DESC LIMIT 50"
        ))?;
        let rows = st.query_map(params_from_iter(p.iter()), |r| {
            Ok(Group {
                fp: r.get(0)?,
                pattern: String::new(),
                count: r.get(1)?,
                level: Level::from_i64(r.get(2)?),
                first: r.get(3)?,
                last: r.get(4)?,
                services: split_list(r.get::<_, Option<String>>(5)?),
                nodes: split_list(r.get::<_, Option<String>>(6)?),
                sources: r.get(7)?,
                exc: r.get::<_, i64>(8)? != 0,
                sample: r.get::<_, i64>(9)?.to_string(),
                spark: vec![0; 24],
            })
        })?;
        for g in rows {
            groups.push(g?);
        }
        if !groups.is_empty() {
            let mut pat = db.prepare_cached("SELECT pattern FROM patterns WHERE conn = ?1 AND fp = ?2")?;
            let mut sample = db.prepare_cached("SELECT text FROM entries WHERE id = ?1")?;
            for g in &mut groups {
                g.pattern = pat.query_row(params![conn, g.fp], |r| r.get(0)).optional()?.unwrap_or_default();
                let id: i64 = g.sample.parse().unwrap_or(0);
                g.sample = sample.query_row(params![id], |r| r.get(0)).optional()?.unwrap_or_default();
            }
            let index: HashMap<String, usize> = groups.iter().enumerate().map(|(i, g)| (g.fp.clone(), i)).collect();
            let marks: Vec<String> = groups.iter().map(|_| "?".to_string()).collect();
            let mut st = db.prepare(&format!(
                "SELECT fp, (ts - ?) * 24 / ? AS b, COUNT(*) FROM entries WHERE {w} AND fp IN ({}) GROUP BY fp, b",
                marks.join(",")
            ))?;
            let mut sp = vec![Sql::Integer(f.from), Sql::Integer(span)];
            sp.extend(p.iter().cloned());
            sp.extend(groups.iter().map(|g| Sql::Text(g.fp.clone())));
            let rows =
                st.query_map(params_from_iter(sp.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, u64>(2)?)))?;
            for row in rows {
                let (fp, b, n) = row?;
                if let Some(&i) = index.get(&fp) {
                    groups[i].spark[b.clamp(0, 23) as usize] += n;
                }
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

    /// Deletes entries older than `before` (µs) and the oldest entries beyond `max_bytes`.
    pub fn retain(&self, before: i64, max_bytes: u64) -> Result<u64> {
        let db = self.db.lock().unwrap();
        let mut deleted = 0u64;
        loop {
            let n = db.execute("DELETE FROM entries WHERE id IN (SELECT id FROM entries WHERE ts < ?1 LIMIT 20000)", params![before])?;
            deleted += n as u64;
            if n < 20000 {
                break;
            }
        }
        for _ in 0..50 {
            if used_bytes(&db)? <= max_bytes {
                break;
            }
            let n = db.execute(
                "DELETE FROM entries WHERE id IN (SELECT id FROM entries ORDER BY ts ASC LIMIT (SELECT MAX(COUNT(*) / 20, 1000) FROM entries))",
                [],
            )?;
            deleted += n as u64;
            if n == 0 {
                break;
            }
        }
        if deleted > 0 {
            db.execute_batch("PRAGMA incremental_vacuum; PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(deleted)
    }

    pub fn stats(&self) -> Result<StoreStats> {
        let db = self.db.lock().unwrap();
        let (entries, oldest) = db.query_row("SELECT COUNT(*), MIN(ts) FROM entries", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(StoreStats { bytes: used_bytes(&db)?, entries, oldest })
    }

    pub fn clear(&self, conn: Option<&str>) -> Result<()> {
        let db = self.db.lock().unwrap();
        match conn {
            Some(c) => {
                db.execute("DELETE FROM entries WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM patterns WHERE conn = ?1", params![c])?;
                db.execute("DELETE FROM cursors WHERE conn = ?1", params![c])?;
            }
            None => db.execute_batch("DELETE FROM entries; DELETE FROM patterns; DELETE FROM cursors;")?,
        }
        db.execute_batch("PRAGMA incremental_vacuum;")?;
        Ok(())
    }
}

fn used_bytes(db: &Connection) -> Result<u64> {
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

fn split_list(s: Option<String>) -> Vec<String> {
    s.map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default()
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
}
