//! Searches, counts, the time graph and error groups: everything the logs
//! view asks of the store.

use super::*;

/// Error groups whose first entry is in the last 15 minutes. Needs at least an
/// hour of stored history, or every group would look new after a fresh start.
pub(super) fn new_groups(db: &Connection, conn: &str, now: i64, oldest: Option<i64>) -> Result<Vec<NewGroup>> {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn query(
    db: &Connection,
    conn: &str,
    f: &LogFilter,
    buckets: usize,
    limit: usize,
    minutes_since: i64,
    indexed: bool,
    page: Option<Page>,
) -> Result<QueryResult> {
    let buckets = buckets.clamp(1, 500) as i64;
    // A page of older lines is only lines: the counts, the graph and the
    // groups are those of the first page, which the caller has already.
    let lines_only = matches!(page, Some(Page::Before(..)));
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
    let countable =
        !lines_only && f.text.is_empty() && f.fps.is_empty() && f.nodes.is_empty() && f.replicas.is_empty() && f.sources.is_empty();
    let lo = (f.from.div_euclid(MINUTE) + (f.from.rem_euclid(MINUTE) != 0) as i64).max(minutes_since.div_euclid(MINUTE));
    let hi = f.to.div_euclid(MINUTE);
    let mut raw = if lines_only { vec![] } else { vec![(f.from, f.to)] };
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
        "SELECT fp, level, ts, exc, service, node, source, id FROM entries WHERE {w} AND fp IS NOT NULL AND level >= 3{}",
        if lines_only { " AND 0" } else { "" }
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
    // One line more than asked for says whether there are more.
    let limit = limit.clamp(1, 5000);
    let per_service = indexed && f.pick_stacks.is_empty() && (1..=50).contains(&f.exact_services.len());
    let parts: Vec<(String, Vec<Sql>)> = if per_service {
        f.exact_services.iter().map(|s| LogFilter { exact_services: vec![s.clone()], ..f.clone() }.where_clause(conn, true)).collect()
    } else {
        vec![(w, p)]
    };
    let mut entries = Vec::new();
    for (mut w, mut p) in parts {
        // The (ts, id) order of the stream; `ts` alone first, so the index finds the start.
        match page {
            Some(Page::Before(ts, id)) => {
                w.push_str(" AND ts <= ? AND (ts < ? OR id < ?)");
                p.extend([Sql::Integer(ts), Sql::Integer(ts), Sql::Integer(id)]);
            }
            Some(Page::After(ts, id)) => {
                w.push_str(" AND ts >= ? AND (ts > ? OR id > ?)");
                p.extend([Sql::Integer(ts), Sql::Integer(ts), Sql::Integer(id)]);
            }
            None => {}
        }
        let mut st = db.prepare_cached(&format!(
            "SELECT id, ts, stack, service, replica, node, source, stream, level, exc, lines, fp, text, raw
             FROM entries WHERE {w} ORDER BY ts DESC, id DESC LIMIT {}",
            limit + 1
        ))?;
        entries.extend(st.query_map(params_from_iter(p.iter()), row_to_entry)?.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    if per_service && f.exact_services.len() > 1 {
        entries.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.id.cmp(&a.id)));
    }
    let more = entries.len() > limit;
    entries.truncate(limit);

    Ok(QueryResult { total, levels, histogram, groups, entries, more })
}

/// The services with stored lines in these stacks, found by skipping through
/// the service index.
pub(super) fn services_of_stacks(db: &Connection, conn: &str, stacks: &[String]) -> Result<Vec<String>> {
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

pub(super) fn error_summary(db: &Connection, conn: &str, now: i64) -> Result<ErrorSummary> {
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
pub(super) fn quiet(db: &Connection, conn: &str, now: i64, minutes_since: i64) -> Result<Vec<Quiet>> {
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

pub(super) fn error_rate(db: &Connection, conn: &str, from: i64, to: i64) -> Result<ErrorRate> {
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
pub(super) fn usage(db: &Connection, conn: &str) -> Result<Vec<ServiceUsage>> {
    let mut st = db.prepare(
        "SELECT service, COUNT(*), SUM(LENGTH(text) + COALESCE(LENGTH(raw), 0)) AS b FROM entries WHERE conn = ?1
         GROUP BY service ORDER BY b DESC",
    )?;
    let rows = st.query_map(params![conn], |r| Ok(ServiceUsage { service: r.get(0)?, entries: r.get(1)?, text_bytes: r.get(2)? }))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryOut> {
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
