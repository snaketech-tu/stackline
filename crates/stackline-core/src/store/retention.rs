//! Keeping the store within its age and size limits, and clearing it.

use super::*;

/// How many lines each connection gives up so that `keep` lines are left:
/// the lines kept are shared out equally, and a connection with fewer lines
/// than its share keeps all of them, which leaves more for the others.
pub(super) fn over_share(counts: &[(String, u64)], keep: u64) -> Vec<(String, u64)> {
    let mut sorted: Vec<&(String, u64)> = counts.iter().collect();
    sorted.sort_by_key(|(_, n)| *n);
    let mut left = keep;
    let mut out = Vec::new();
    for (i, (conn, n)) in sorted.iter().enumerate() {
        let share = left / (sorted.len() - i) as u64;
        let kept = (*n).min(share);
        left -= kept;
        if *n > kept {
            out.push((conn.clone(), n - kept));
        }
    }
    out
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
        // Over the size limit: drop the oldest lines, down to 90% of the
        // limit so this does not run again a minute later. Every connection
        // gets an equal share of what is kept, and only connections above
        // their share give up lines: a chatty server must not push out the
        // whole history of a quiet one. Deleted lines stay in the search index
        // until it is merged, so the index is merged before the size is
        // measured again.
        for _ in 0..4 {
            let live = live_bytes(&self.db.lock().unwrap())?;
            if live <= max_bytes {
                break;
            }
            let counts: Vec<(String, u64)> = {
                let db = self.db.lock().unwrap();
                let mut st = db.prepare("SELECT conn, COUNT(*) FROM entries GROUP BY conn")?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
            };
            let total: u64 = counts.iter().map(|(_, n)| n).sum();
            if total == 0 {
                break;
            }
            let target = max_bytes / 10 * 9;
            let left = ((total as u128 * (live - target) as u128).div_ceil(live as u128) as u64).clamp(1, total);
            for (conn, mut drop) in over_share(&counts, total - left) {
                while drop > 0 {
                    let n = self.db.lock().unwrap().execute(
                        "DELETE FROM entries WHERE id IN (SELECT id FROM entries WHERE conn = ?1 ORDER BY ts LIMIT ?2)",
                        params![conn, drop.min(BATCH as u64)],
                    )? as u64;
                    deleted += n;
                    drop = drop.saturating_sub(n);
                    if n == 0 {
                        break;
                    }
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
    pub(super) fn shrink(&self) -> Result<()> {
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
    pub(super) fn merge_index(&self) -> Result<()> {
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
    pub(super) fn used_bytes(&self, db: &Connection) -> Result<u64> {
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
    pub(super) fn delete_entries(&self, conn: Option<&str>, service: Option<&str>) -> Result<u64> {
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

    /// Checks which host or swarm a connection's logs come from against the
    /// one remembered. The first one seen is remembered. A different one is
    /// only reported: its lines must not mix with the old ones, and whether
    /// the old ones go is the user's call ([`Store::accept_host`]).
    pub fn check_host(&self, conn: &str, identity: &str) -> Result<HostCheck> {
        let db = self.db.lock().unwrap();
        let known: Option<String> = db.query_row("SELECT identity FROM hosts WHERE conn = ?1", params![conn], |r| r.get(0)).optional()?;
        match known {
            Some(k) if k == identity => Ok(HostCheck::Same),
            Some(_) => Ok(HostCheck::Changed),
            None => {
                db.execute("INSERT INTO hosts (conn, identity) VALUES (?1, ?2)", params![conn, identity])?;
                Ok(HostCheck::First)
            }
        }
    }

    /// Makes `identity` the host of a connection. With `keep_old` false the
    /// lines from the host before are deleted first.
    pub fn accept_host(&self, conn: &str, identity: &str, keep_old: bool) -> Result<()> {
        if !keep_old {
            self.clear(Some(conn))?;
        }
        self.db.lock().unwrap().execute(
            "INSERT INTO hosts (conn, identity) VALUES (?1, ?2) ON CONFLICT (conn) DO UPDATE SET identity = excluded.identity",
            params![conn, identity],
        )?;
        Ok(())
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
        // Under the write lock: a batch being stored right now either lands
        // before this and is deleted below, or sees the new epoch and is dropped.
        {
            let _db = self.db.lock().unwrap();
            self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
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

/// The connections that have stored lines, found by skipping through the
/// index instead of reading every row.
pub(super) fn conns(db: &Connection) -> Result<Vec<String>> {
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
pub(super) fn checkpoint(db: &Connection) -> Result<()> {
    db.busy_timeout(std::time::Duration::ZERO)?;
    let r = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(r?)
}

/// Pages that hold data; free pages are given back by `incremental_vacuum`.
pub(super) fn live_bytes(db: &Connection) -> Result<u64> {
    let pages: u64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let free: u64 = db.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
    let size: u64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(pages.saturating_sub(free) * size)
}
