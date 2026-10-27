//! Reads logs per replica and stores them.
//!
//! Every poll asks Docker only for lines newer than the last one stored for
//! that replica (`since=`), so after the first load the server does very little work.

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

struct SourceState {
    src: LogSource,
    assembler: Assembler,
    cursor: Option<i64>,
    /// When a stopped container was last read; stopped containers are read once a minute.
    last_read: Option<Instant>,
    error: Option<String>,
}

pub struct Collector {
    conn: String,
    store: Arc<Store>,
    states: HashMap<String, SourceState>,
}

#[derive(Debug, Default, Clone)]
pub struct PollReport {
    pub sources: usize,
    pub entries: usize,
    pub errors: Vec<String>,
}

impl Collector {
    pub fn new(conn: &str, store: Arc<Store>) -> Self {
        Self { conn: conn.to_string(), store, states: HashMap::new() }
    }

    /// Reads new lines from every source. `retention_us` bounds the first read of a new source.
    pub async fn poll(&mut self, client: &DockerClient, sources: Vec<LogSource>, retention_us: i64) -> PollReport {
        let now = chrono::Utc::now().timestamp_micros();
        let keys: Vec<String> = sources.iter().map(|s| s.key.clone()).collect();
        self.states.retain(|k, _| keys.contains(k));
        for src in sources {
            let st = self.states.entry(src.key.clone()).or_insert_with(|| SourceState {
                assembler: Assembler::new(&src.service),
                cursor: None,
                last_read: None,
                error: None,
                src: src.clone(),
            });
            st.src = src;
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
    let timeout = if st.cursor.is_none() { Duration::from_secs(600) } else { Duration::from_secs(60) };

    let mut body = client.get_stream(&path).await?;
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
            written += flush(store, conn, &st.src, &mut entries, last_ts).await?;
        }
    }
    decoder.finish(&mut lines);
    consume(st, &mut lines, &mut entries, &mut last_ts);
    if st.src.running {
        st.assembler.flush_if_idle(now, &mut entries);
    } else {
        st.assembler.flush(&mut entries);
    }
    written += flush(store, conn, &st.src, &mut entries, last_ts).await?;
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
        if last_ts.is_some_and(|l| ts <= l) {
            continue;
        }
        *last_ts = Some(ts);
        st.assembler.push(ts, kind, text, entries);
    }
}

async fn flush(store: &Arc<Store>, conn: &str, src: &LogSource, entries: &mut Vec<LogEntry>, last_ts: Option<i64>) -> Result<usize> {
    let batch = std::mem::take(entries);
    let n = batch.len();
    let (store, conn, src) = (store.clone(), conn.to_string(), src.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        store.insert(&conn, &src, &batch)?;
        if let Some(ts) = last_ts {
            store.set_cursor(&conn, &src.key, ts)?;
        }
        Ok(())
    })
    .await??;
    Ok(n)
}
