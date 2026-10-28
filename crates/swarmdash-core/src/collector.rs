//! Reads logs per replica and stores them.
//!
//! Every poll asks Docker only for lines newer than the last one stored for
//! that replica (`since=`), and only among its newest lines (`tail=`): with
//! `since` alone Docker reads the whole log file on every poll. So after the
//! first load the server does very little work.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::StreamExt;
use http_body_util::BodyExt;

use crate::docker::DockerClient;
use crate::docker::frames::LineDecoder;
use crate::logs::assemble::Assembler;
use crate::logs::{LogEntry, split_docker_timestamp};
use crate::model::LogSource;
use crate::store::Store;

const BATCH: usize = 2000;
/// How many of the newest lines a poll asks for at first, and at most. A
/// replica that wrote more since the last poll is asked again for more.
const TAIL_MIN: usize = 1000;
const TAIL_MAX: usize = 100_000;

struct SourceState {
    src: LogSource,
    assembler: Assembler,
    cursor: Option<i64>,
    /// When a stopped container was last read; stopped containers are read once a minute.
    last_read: Option<Instant>,
    error: Option<String>,
    /// The `tail` of the next poll; grows for replicas that write a lot.
    tail: usize,
    /// The store's epoch when this read started; see [`Store::epoch`].
    epoch: u64,
}

pub struct Collector {
    conn: String,
    store: Arc<Store>,
    states: HashMap<String, SourceState>,
    epoch: u64,
}

#[derive(Debug, Default, Clone)]
pub struct PollReport {
    pub sources: usize,
    pub entries: usize,
    pub errors: Vec<String>,
}

impl Collector {
    pub fn new(conn: &str, store: Arc<Store>) -> Self {
        let epoch = store.epoch();
        Self { conn: conn.to_string(), store, states: HashMap::new(), epoch }
    }

    /// Reads new lines from every source. `retention_us` bounds the first read of a new source.
    pub async fn poll(&mut self, client: &DockerClient, sources: Vec<LogSource>, retention_us: i64) -> PollReport {
        let now = chrono::Utc::now().timestamp_micros();
        // After "Clear stored logs" every source is read again from the
        // start of the retention, so the cleared range fills up again.
        let epoch = self.store.epoch();
        if epoch != self.epoch {
            self.states.clear();
            self.epoch = epoch;
        }
        let keys: std::collections::HashSet<String> = sources.iter().map(|s| s.key.clone()).collect();
        self.states.retain(|k, _| keys.contains(k));
        for src in sources {
            let st = self.states.entry(src.key.clone()).or_insert_with(|| SourceState {
                assembler: Assembler::new(&src.service),
                cursor: None,
                last_read: None,
                error: None,
                tail: TAIL_MIN,
                epoch,
                src: src.clone(),
            });
            st.src = src;
            st.epoch = epoch;
            if st.cursor.is_none() {
                st.cursor = self.store.cursor(&self.conn, &st.src.key).ok().flatten();
            }
        }

        let due: Vec<String> = self
            .states
            .iter()
            .filter(|(_, s)| s.src.running || s.last_read.is_none_or(|t| t.elapsed() > Duration::from_secs(60)))
            .map(|(k, _)| k.clone())
            .collect();
        let mut taken: Vec<SourceState> = due.iter().filter_map(|k| self.states.remove(k)).collect();
        let mut report = PollReport { sources: taken.len(), ..Default::default() };

        let conn = self.conn.clone();
        let store = self.store.clone();
        let results: Vec<(SourceState, usize)> = futures_util::stream::iter(taken.drain(..))
            .map(|mut st| {
                let (conn, store, client) = (conn.clone(), store.clone(), client.clone());
                async move {
                    let n = match read_source(&client, &store, &conn, &mut st, now, retention_us).await {
                        Ok(n) => {
                            st.error = None;
                            n
                        }
                        Err(e) => {
                            st.error = Some(format!("{}: {e:#}", st.src.service));
                            0
                        }
                    };
                    st.last_read = Some(Instant::now());
                    (st, n)
                }
            })
            .buffer_unordered(6)
            .collect()
            .await;
        for (st, n) in results {
            report.entries += n;
            if let Some(e) = &st.error {
                report.errors.push(e.clone());
            }
            self.states.insert(st.src.key.clone(), st);
        }
        report
    }
}

async fn read_source(
    client: &DockerClient,
    store: &Arc<Store>,
    conn: &str,
    st: &mut SourceState,
    now: i64,
    retention_us: i64,
) -> Result<usize> {
    let since_us = st.cursor.map(|c| c + 1).unwrap_or(now - retention_us);
    let since = format!("{}.{:06}000", since_us.div_euclid(1_000_000), since_us.rem_euclid(1_000_000));
    let kind = if st.src.via_task { "tasks" } else { "containers" };
    let path = format!("/{kind}/{}/logs?stdout=1&stderr=1&timestamps=1&since={since}", st.src.key);
    if st.cursor.is_none() {
        // The first read of a replica: everything within the retention.
        return read_stream(client, store, conn, st, &path, Duration::from_secs(600), now).await;
    }
    // Docker reads the newest `tail` lines from the end of the file and keeps
    // those after `since`. When they are (nearly) all new, more may have been
    // written since the last poll, so ask again for more. Half, because one
    // very long line can arrive as several parts that each count.
    let mut tail = st.tail;
    loop {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut body = client.get_stream(&format!("{path}&tail={tail}")).await?;
        let mut decoder = LineDecoder::new(st.src.tty);
        let mut lines = Vec::new();
        loop {
            let frame = match tokio::time::timeout_at(deadline.into(), body.frame()).await {
                Ok(Some(f)) => f?,
                Ok(None) | Err(_) => break,
            };
            if let Ok(data) = frame.into_data() {
                decoder.push(&data, &mut lines);
            }
        }
        decoder.finish(&mut lines);
        if lines.len() < tail / 2 {
            if lines.len() < tail / 8 {
                st.tail = (tail / 2).max(TAIL_MIN);
            }
            let (mut entries, mut last_ts) = (Vec::new(), st.cursor);
            consume(st, &mut lines, &mut entries, &mut last_ts);
            if st.src.running {
                st.assembler.flush_if_idle(now, &mut entries);
            } else {
                st.assembler.flush(&mut entries);
            }
            let n = flush(store, conn, st, &mut entries, last_ts).await?;
            st.cursor = last_ts;
            return Ok(n);
        }
        if tail >= TAIL_MAX {
            // A burst too big to ask for by count: read everything after the cursor.
            return read_stream(client, store, conn, st, &path, Duration::from_secs(600), now).await;
        }
        tail = (tail * 8).min(TAIL_MAX);
        st.tail = tail;
    }
}

/// Reads everything a log request returns, storing it in batches as it arrives.
async fn read_stream(
    client: &DockerClient,
    store: &Arc<Store>,
    conn: &str,
    st: &mut SourceState,
    path: &str,
    timeout: Duration,
    now: i64,
) -> Result<usize> {
    let mut body = client.get_stream(path).await?;
    let mut decoder = LineDecoder::new(st.src.tty);
    let mut lines = Vec::new();
    let mut entries: Vec<LogEntry> = Vec::new();
    let mut written = 0usize;
    let mut last_ts = st.cursor;
    let deadline = Instant::now() + timeout;

    loop {
        let frame = match tokio::time::timeout_at(deadline.into(), body.frame()).await {
            Ok(Some(f)) => f?,
            Ok(None) => break,
            Err(_) => break,
        };
        let Ok(data) = frame.into_data() else { continue };
        decoder.push(&data, &mut lines);
        consume(st, &mut lines, &mut entries, &mut last_ts);
        if entries.len() >= BATCH {
            written += flush(store, conn, st, &mut entries, last_ts).await?;
        }
    }
    decoder.finish(&mut lines);
    consume(st, &mut lines, &mut entries, &mut last_ts);
    if st.src.running {
        st.assembler.flush_if_idle(now, &mut entries);
    } else {
        st.assembler.flush(&mut entries);
    }
    written += flush(store, conn, st, &mut entries, last_ts).await?;
    st.cursor = last_ts;
    Ok(written)
}

fn consume(
    st: &mut SourceState,
    lines: &mut Vec<(crate::docker::frames::StreamKind, String)>,
    entries: &mut Vec<LogEntry>,
    last_ts: &mut Option<i64>,
) {
    for (kind, line) in lines.drain(..) {
        let Some((ts, text)) = split_docker_timestamp(&line) else { continue };
        // Only lines already stored by an earlier read are skipped: lines of
        // one read can share a microsecond (a stack trace written at once).
        if st.cursor.is_some_and(|c| ts <= c) {
            continue;
        }
        *last_ts = (*last_ts).max(Some(ts));
        st.assembler.push(ts, kind, text, entries);
    }
}

async fn flush(store: &Arc<Store>, conn: &str, st: &SourceState, entries: &mut Vec<LogEntry>, last_ts: Option<i64>) -> Result<usize> {
    let batch = std::mem::take(entries);
    let n = batch.len();
    let (store, conn, src, epoch) = (store.clone(), conn.to_string(), st.src.clone(), st.epoch);
    tokio::task::spawn_blocking(move || -> Result<()> {
        // The store was cleared during this read: the next poll starts over.
        if store.epoch() != epoch {
            return Ok(());
        }
        store.insert(&conn, &src, &batch)?;
        if let Some(ts) = last_ts {
            store.set_cursor(&conn, &src.key, ts)?;
        }
        Ok(())
    })
    .await??;
    Ok(n)
}
