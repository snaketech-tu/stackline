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
/// How far back a replica seen for the first time is read. Reading the whole
/// retention of every replica of a big stack at once is slow and fills the
/// store with lines nobody asked for; newer lines are kept from then on.
const FIRST_READ_US: i64 = 6 * 3_600_000_000;

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
    /// The first read goes back the whole retention, not [`FIRST_READ_US`].
    refill: bool,
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

    /// Reads new lines from every source. The first read of a new source goes
    /// back [`FIRST_READ_US`], at most `retention_us`.
    pub async fn poll(&mut self, client: &DockerClient, sources: Vec<LogSource>, retention_us: i64) -> PollReport {
        let now = chrono::Utc::now().timestamp_micros();
        // After "Clear stored logs" every source is read again from the
        // start of the retention, so the cleared range fills up again.
        let epoch = self.store.epoch();
        let refill = epoch != self.epoch;
        if refill {
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
                refill,
                src: src.clone(),
            });
            // A container that just stopped is read right away: its last
            // lines usually say why, and should not wait for the next minute.
            if st.src.running && !src.running {
                st.last_read = None;
            }
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
    let first = if st.refill { retention_us } else { retention_us.min(FIRST_READ_US) };
    let since_us = st.cursor.map(|c| c + 1).unwrap_or(now - first);
    let since = format!("{}.{:06}000", since_us.div_euclid(1_000_000), since_us.rem_euclid(1_000_000));
    let kind = if st.src.via_task { "tasks" } else { "containers" };
    let path = format!("/{kind}/{}/logs?stdout=1&stderr=1&timestamps=1&since={since}", st.src.key);
    if st.cursor.is_none() {
        // The first read of a replica: everything since `first`.
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::docker::Endpoint;
    use crate::store::LogFilter;

    /// Stands in for Docker's log endpoint: answers `GET …/logs` with the
    /// lines newer than `since`, and only the last `tail` of them, the way the
    /// engine does. Records every path it was asked for.
    struct FakeDocker {
        lines: Arc<Mutex<Vec<(i64, String)>>>,
        paths: Arc<Mutex<Vec<String>>>,
        addr: String,
    }

    impl FakeDocker {
        async fn start() -> FakeDocker {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            let lines: Arc<Mutex<Vec<(i64, String)>>> = Arc::default();
            let paths: Arc<Mutex<Vec<String>>> = Arc::default();
            let (l, p) = (lines.clone(), paths.clone());
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else { return };
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf);
                    let path = head.lines().next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
                    p.lock().unwrap().push(path.clone());
                    let body = frames(&l.lock().unwrap(), &path);
                    let res = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.raw-stream\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(res.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.shutdown().await;
                }
            });
            FakeDocker { lines, paths, addr }
        }

        fn client(&self) -> DockerClient {
            DockerClient::new(Endpoint::Tcp(self.addr.clone()))
        }

        fn write(&self, ts: i64, text: &str) {
            self.lines.lock().unwrap().push((ts, text.to_string()));
        }

        fn paths(&self) -> Vec<String> {
            std::mem::take(&mut *self.paths.lock().unwrap())
        }
    }

    fn param(path: &str, key: &str) -> Option<String> {
        path.split_once('?')?.1.split('&').find_map(|kv| kv.strip_prefix(&format!("{key}=")).map(str::to_string))
    }

    /// Multiplexed stdout frames of the lines a request asks for.
    fn frames(lines: &[(i64, String)], path: &str) -> Vec<u8> {
        let since = param(path, "since").map(|s| {
            let (secs, frac) = s.split_once('.').unwrap_or((&s, "0"));
            secs.parse::<i64>().unwrap() * 1_000_000 + format!("{frac:0<9}")[..6].parse::<i64>().unwrap()
        });
        let tail = param(path, "tail").map(|t| t.parse::<usize>().unwrap());
        let mut chosen: Vec<&(i64, String)> = lines.iter().collect();
        if let Some(n) = tail {
            chosen = chosen[chosen.len().saturating_sub(n)..].to_vec();
        }
        let mut out = Vec::new();
        for (ts, text) in chosen {
            if since.is_some_and(|s| *ts < s) {
                continue;
            }
            let stamp = chrono::DateTime::from_timestamp_micros(*ts).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            let line = format!("{stamp} {text}\n");
            out.extend_from_slice(&[1, 0, 0, 0]);
            out.extend_from_slice(&(line.len() as u32).to_be_bytes());
            out.extend_from_slice(line.as_bytes());
        }
        out
    }

    fn source() -> LogSource {
        LogSource {
            key: "c1".into(),
            via_task: false,
            stack: "shop".into(),
            service: "shop_api".into(),
            replica: "1".into(),
            node: "shop".into(),
            tty: false,
            running: true,
        }
    }

    fn stored(store: &Store) -> u64 {
        store.query("conn", &LogFilter { from: 0, to: i64::MAX, ..Default::default() }, 1, 10).unwrap().total
    }

    const RETENTION: i64 = 7 * 86_400_000_000;

    #[tokio::test]
    async fn reads_the_retention_once_then_only_the_newest_lines() {
        let docker = FakeDocker::start().await;
        let store = Arc::new(Store::open_in_memory().unwrap());
        let now = chrono::Utc::now().timestamp_micros();
        for i in 0..3 {
            docker.write(now - 60_000_000 + i * 1_000_000, &format!("INFO line {i}"));
        }
        let mut c = Collector::new("conn", store.clone());

        // The first read asks for everything within the retention, without a tail.
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!((r.entries, r.errors.len()), (3, 0), "{r:?}");
        let paths = docker.paths();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].contains("since=") && !paths[0].contains("tail="), "{}", paths[0]);
        assert_eq!(store.cursor("conn", "c1").unwrap(), Some(now - 60_000_000 + 2_000_000));

        // Later polls ask for the newest lines after the cursor.
        docker.write(now - 30_000_000, "INFO line 3");
        docker.write(now - 29_000_000, "ERROR line 4");
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!(r.entries, 2, "{r:?}");
        let paths = docker.paths();
        assert!(paths[0].contains(&format!("tail={TAIL_MIN}")), "{}", paths[0]);
        assert_eq!(stored(&store), 5);

        // Nothing new: nothing stored twice.
        assert_eq!(c.poll(&docker.client(), vec![source()], RETENTION).await.entries, 0);
        assert_eq!(stored(&store), 5);
        docker.paths();

        // After the store was cleared the source is read again from the start.
        store.clear(None).unwrap();
        assert_eq!(stored(&store), 0);
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!(r.entries, 5, "{r:?}");
        assert!(!docker.paths()[0].contains("tail="));
        assert_eq!(stored(&store), 5);
    }

    #[tokio::test]
    async fn a_new_replica_is_read_6_hours_back_and_a_clear_refills_the_retention() {
        let docker = FakeDocker::start().await;
        let store = Arc::new(Store::open_in_memory().unwrap());
        let now = chrono::Utc::now().timestamp_micros();
        docker.write(now - 2 * 86_400_000_000, "INFO two days ago");
        docker.write(now - 7 * 3_600_000_000, "INFO seven hours ago");
        docker.write(now - 60_000_000, "INFO a minute ago");
        let mut c = Collector::new("conn", store.clone());

        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!(r.entries, 1, "{r:?}");
        let since = param(&docker.paths()[0], "since").unwrap();
        let since_s: i64 = since.split('.').next().unwrap().parse().unwrap();
        assert!((now - FIRST_READ_US) / 1_000_000 - since_s <= 1, "{since}");

        // "Clear stored logs" reads the whole retention again.
        store.clear(None).unwrap();
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!(r.entries, 3, "{r:?}");

        // A replica that starts after that is read 6 hours back again.
        let mut other = source();
        other.key = "c2".into();
        let r = c.poll(&docker.client(), vec![source(), other], RETENTION).await;
        assert_eq!(r.entries, 1, "{r:?}");
        assert_eq!(stored(&store), 4);
    }

    #[tokio::test]
    async fn a_burst_bigger_than_the_tail_is_asked_for_again() {
        let docker = FakeDocker::start().await;
        let store = Arc::new(Store::open_in_memory().unwrap());
        let now = chrono::Utc::now().timestamp_micros();
        docker.write(now - 60_000_000, "INFO start");
        let mut c = Collector::new("conn", store.clone());
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!(r.entries, 1, "{r:?}");
        docker.paths();

        // More lines than one tail: the tail grows until the newest lines fit.
        for i in 0..1500 {
            docker.write(now - 30_000_000 + i * 1000, &format!("INFO burst {i}"));
        }
        let r = c.poll(&docker.client(), vec![source()], RETENTION).await;
        assert_eq!((r.entries, r.errors.len()), (1500, 0), "{r:?}");
        let tails: Vec<String> = docker.paths().iter().filter_map(|p| param(p, "tail")).collect();
        assert_eq!(tails, vec![TAIL_MIN.to_string(), (TAIL_MIN * 8).to_string()]);
        assert_eq!(stored(&store), 1501);

        // A container that stopped is read once more at once, for its last
        // lines, and then left alone for a minute.
        let mut stopped = source();
        stopped.running = false;
        docker.write(now - 1_000_000, "ERROR fatal: out of memory");
        let r = c.poll(&docker.client(), vec![stopped.clone()], RETENTION).await;
        assert_eq!((r.sources, r.entries), (1, 1), "{r:?}");
        assert_eq!(c.poll(&docker.client(), vec![stopped], RETENTION).await.sources, 0);
    }
}
