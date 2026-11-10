//! The tables, indexes and triggers of the store.

/// Keeps the search index in step with new lines. Their per-minute counts
/// are added by [`Store::insert`](super::Store::insert), once per minute instead of once per line.
pub(super) const INSERT_TRIGGER: &str = "CREATE TRIGGER entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts (rowid, text) VALUES (new.id, new.text);
END;";

/// Adds to the counts of a minute: for the service, and for the whole connection (service '').
pub(super) const ADD_MINUTE: &str =
    "INSERT INTO minutes (conn, minute, service, stack, debug, info, warn, error, debug_exc, info_exc, warn_exc, error_exc)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
    ON CONFLICT (conn, minute, service) DO UPDATE SET
        debug = debug + excluded.debug, info = info + excluded.info, warn = warn + excluded.warn, error = error + excluded.error,
        debug_exc = debug_exc + excluded.debug_exc, info_exc = info_exc + excluded.info_exc,
        warn_exc = warn_exc + excluded.warn_exc, error_exc = error_exc + excluded.error_exc";

/// Keeps the search index and the per-minute counts in step with deleted
/// lines. Deleting every line drops it for a moment, so the table can be
/// emptied at once.
pub(super) const DELETE_TRIGGER: &str = "CREATE TRIGGER entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts (entries_fts, rowid, text) VALUES ('delete', old.id, old.text);
    UPDATE minutes SET
        debug = debug - (old.level = 1), info = info - (old.level NOT IN (1, 3, 4)),
        warn = warn - (old.level = 3), error = error - (old.level = 4),
        debug_exc = debug_exc - (old.exc != 0 AND old.level = 1), info_exc = info_exc - (old.exc != 0 AND old.level NOT IN (1, 3, 4)),
        warn_exc = warn_exc - (old.exc != 0 AND old.level = 3), error_exc = error_exc - (old.exc != 0 AND old.level = 4)
    WHERE conn = old.conn AND minute = old.ts / 60000000 AND service IN (old.service, '');
END;";

pub(super) const MINUTE: i64 = 60_000_000;

pub(super) const SCHEMA: &str = r#"
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
