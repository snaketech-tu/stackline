//! When containers started, stopped, crashed or failed a health check.
//!
//! Read from Docker's event stream (`GET /events`) a few seconds at a time and
//! stored next to the logs, so the time graphs can mark a restart and the
//! Overview can say that a service crashed three times in the last hour.
//! Docker keeps only its most recent events in memory, so they are read often.
//! Tasks that failed on other swarm nodes come from the swarm's task history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use http_body_util::BodyExt;
use serde::Serialize;
use serde_json::Value;

use crate::docker::DockerClient;
use crate::docker::frames::LineDecoder;
use crate::model::{Snapshot, TaskFailure};
use crate::store::Store;

/// The cursor of the event stream, next to the cursors of the log sources.
pub const CURSOR: &str = "~events";
/// Events are read again from this long before the last read, so a stop
/// that came just before a read still explains the exit right after it.
const OVERLAP_US: i64 = 60_000_000;
/// A container that exits within this long after `docker stop` (a kill
/// event) or running out of memory exited because of that.
const CAUSE_US: i64 = 30_000_000;
const EVERY: Duration = Duration::from_secs(5);

/// What happened to a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Start,
    /// Exited with code 0 on its own.
    Exit,
    /// Exited after it was stopped or killed (`docker stop`, an update, a restart).
    Stop,
    /// Exited with an error code on its own.
    Crash,
    /// Stopped by the kernel at its memory limit.
    Oom,
    /// The health check started failing.
    Unhealthy,
    /// A swarm task failed or was rejected (from the task history).
    Failed,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Start => "start",
            Kind::Exit => "exit",
            Kind::Stop => "stop",
            Kind::Crash => "crash",
            Kind::Oom => "oom",
            Kind::Unhealthy => "unhealthy",
            Kind::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        [Kind::Start, Kind::Exit, Kind::Stop, Kind::Crash, Kind::Oom, Kind::Unhealthy, Kind::Failed].into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Unique per connection, so events read twice are stored once.
    #[serde(skip)]
    pub key: String,
    /// Unix microseconds.
    pub ts: i64,
    pub stack: String,
    pub service: String,
    pub replica: String,
    pub node: String,
    /// The container ID.
    pub source: String,
    pub kind: Kind,
    pub exit_code: Option<i64>,
    pub detail: Option<String>,
}

/// Reads the events of one connection.
pub struct EventPoller {
    conn: String,
    store: Arc<Store>,
    last: Option<Instant>,
    parser: Parser,
}

impl EventPoller {
    pub fn new(conn: &str, store: Arc<Store>) -> Self {
        Self { conn: conn.to_string(), store, last: None, parser: Parser::default() }
    }

    /// Reads what happened since the last read, at most every few seconds.
    /// Returns how many events were new.
    pub async fn poll(&mut self, client: &DockerClient, snap: &Snapshot, retention_us: i64) -> Result<usize> {
        if self.last.is_some_and(|t| t.elapsed() < EVERY) {
            return Ok(0);
        }
        self.last = Some(Instant::now());
        let epoch = self.store.epoch();
        let now = chrono::Utc::now().timestamp_micros();
        let (store, conn) = (self.store.clone(), self.conn.clone());
        let cursor = tokio::task::spawn_blocking(move || store.cursor(&conn, CURSOR)).await??;
        let since = cursor.map_or(now - retention_us, |c| c - OVERLAP_US).max(now - retention_us);
        let filters = "%7B%22type%22%3A%5B%22container%22%5D%7D"; // {"type":["container"]}
        let path = format!("/events?since={}&until={}&filters={filters}", secs(since), secs(now));
        let lines = read_lines(client, &path).await?;

        let names = Names::of(snap);
        let mut events: Vec<Event> =
            lines.iter().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| self.parser.feed(&v, &names)).collect();
        self.parser.forget_before(now - 2 * CAUSE_US);
        events.extend(snap.task_failures.iter().filter(|t| t.node != names.local || t.container.is_empty()).map(from_task));

        let (store, conn) = (self.store.clone(), self.conn.clone());
        tokio::task::spawn_blocking(move || {
            // A clear while reading: what was read belongs to the lines just deleted.
            if store.epoch() != epoch {
                return Ok(0);
            }
            let n = store.insert_events(&conn, &events)?;
            store.set_cursor(&conn, CURSOR, now)?;
            Ok(n)
        })
        .await?
    }
}

/// Docker takes event times in seconds, with a fraction.
fn secs(us: i64) -> String {
    format!("{}.{:06}", us.div_euclid(1_000_000), us.rem_euclid(1_000_000))
}

/// Reads the event stream up to `until`, where Docker ends it.
async fn read_lines(client: &DockerClient, path: &str) -> Result<Vec<String>> {
    let mut body = client.get_stream(path).await?;
    let mut dec = LineDecoder::new(true);
    let mut out = Vec::new();
    let mut lines = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match tokio::time::timeout_at(deadline.into(), body.frame()).await {
            Ok(Some(frame)) => {
                if let Some(data) = frame?.data_ref() {
                    dec.push(data, &mut lines);
                    out.extend(lines.drain(..).map(|(_, l)| l));
                }
            }
            Ok(None) => break,
            // A daemon that never ends the stream: keep what came.
            Err(_) => break,
        }
    }
    Ok(out)
}

/// What the snapshot knows to name events by.
struct Names {
    /// This host, or the swarm node Stackline is connected to.
    local: String,
    /// Swarm node IDs to host names, for global services.
    nodes: HashMap<String, String>,
}

impl Names {
    fn of(snap: &Snapshot) -> Names {
        let local = snap.nodes.iter().find(|n| n.is_local).map(|n| n.hostname.clone()).unwrap_or_else(|| snap.host.name.clone());
        Names { local, nodes: snap.nodes.iter().map(|n| (n.id.clone(), n.hostname.clone())).collect() }
    }
}

/// Turns Docker events into stored events. Remembers recent stops and
/// out-of-memory kills, to tell why a container exited.
#[derive(Default)]
struct Parser {
    killed: HashMap<String, i64>,
    oom: HashMap<String, i64>,
}

impl Parser {
    fn feed(&mut self, v: &Value, names: &Names) -> Option<Event> {
        if v["Type"].as_str() != Some("container") {
            return None;
        }
        let id = v["Actor"]["ID"].as_str()?;
        let nanos = v["timeNano"].as_i64()?;
        let ts = nanos / 1000;
        let action = v["Action"].as_str()?;
        let attrs = &v["Actor"]["Attributes"];
        let kind = match action {
            "kill" => {
                self.killed.insert(id.to_string(), ts);
                return None;
            }
            "oom" => {
                self.oom.insert(id.to_string(), ts);
                return None;
            }
            "start" => Kind::Start,
            "health_status: unhealthy" => Kind::Unhealthy,
            "die" => {
                let code = attrs["exitCode"].as_str().and_then(|c| c.parse::<i64>().ok());
                let recent = |m: &HashMap<String, i64>| m.get(id).is_some_and(|t| ts - t <= CAUSE_US && ts >= *t);
                if recent(&self.oom) {
                    Kind::Oom
                } else if recent(&self.killed) {
                    Kind::Stop
                } else if code.unwrap_or(0) == 0 {
                    Kind::Exit
                } else {
                    Kind::Crash
                }
            }
            _ => return None,
        };
        let (stack, service, replica) = attribute(attrs, names)?;
        let exit_code = match kind {
            Kind::Exit | Kind::Stop | Kind::Crash | Kind::Oom => attrs["exitCode"].as_str().and_then(|c| c.parse().ok()),
            _ => None,
        };
        Some(Event {
            key: format!("{id}:{nanos}:{action}"),
            ts,
            stack,
            service,
            replica,
            node: names.local.clone(),
            source: id.to_string(),
            kind,
            exit_code,
            detail: None,
        })
    }

    fn forget_before(&mut self, ts: i64) {
        self.killed.retain(|_, t| *t >= ts);
        self.oom.retain(|_, t| *t >= ts);
    }
}

/// The stack, full service name and replica of a container, named as the
/// snapshot names them, so events match the logs of the same service.
fn attribute(a: &Value, names: &Names) -> Option<(String, String, String)> {
    let get = |k: &str| a[k].as_str().filter(|s| !s.is_empty());
    if let Some(service) = get("com.docker.swarm.service.name") {
        let stack = get("com.docker.stack.namespace").unwrap_or("services");
        // `<service>.<slot>.<task id>`, or `<service>.<node id>.<task id>` for a global service.
        let slot =
            get("com.docker.swarm.task.name").and_then(|t| t.strip_prefix(service)?.strip_prefix('.')?.split('.').next()).unwrap_or("1");
        let replica = names.nodes.get(slot).cloned().unwrap_or_else(|| slot.to_string());
        return Some((stack.to_string(), service.to_string(), replica));
    }
    if let (Some(project), Some(service)) = (get("com.docker.compose.project"), get("com.docker.compose.service")) {
        if get("com.docker.compose.oneoff") == Some("True") {
            return None;
        }
        let number = get("com.docker.compose.container-number").unwrap_or("1");
        return Some((project.to_string(), format!("{project}_{service}"), number.to_string()));
    }
    let name = get("name")?;
    Some(("standalone".to_string(), name.to_string(), "1".to_string()))
}

fn from_task(t: &TaskFailure) -> Event {
    Event {
        key: format!("task:{}", t.task_id),
        ts: t.ts,
        stack: t.stack.clone(),
        service: t.service.clone(),
        replica: t.replica.clone(),
        node: t.node.clone(),
        source: t.container.clone(),
        kind: Kind::Failed,
        exit_code: t.exit_code,
        detail: t.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names() -> Names {
        Names { local: "box".into(), nodes: HashMap::from([("n1".to_string(), "worker-1".to_string())]) }
    }

    fn ev(action: &str, id: &str, secs: i64, attrs: Value) -> Value {
        json!({ "Type": "container", "Action": action, "Actor": { "ID": id, "Attributes": attrs }, "timeNano": secs * 1_000_000_000 })
    }

    fn compose(svc: &str, extra: Value) -> Value {
        let mut a = json!({ "com.docker.compose.project": "demo", "com.docker.compose.service": svc, "com.docker.compose.container-number": "2", "com.docker.compose.oneoff": "False", "name": "demo-x-2" });
        a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        a
    }

    #[test]
    fn tells_a_crash_from_a_stop_and_an_out_of_memory_kill() {
        let mut p = Parser::default();
        let n = names();
        let kinds: Vec<(Kind, Option<i64>)> = [
            ev("start", "a", 100, compose("app", json!({}))),
            ev("die", "a", 110, compose("app", json!({ "exitCode": "1" }))),
            ev("kill", "b", 200, compose("app", json!({ "signal": "15" }))),
            ev("die", "b", 205, compose("app", json!({ "exitCode": "143" }))),
            ev("oom", "c", 300, compose("app", json!({}))),
            ev("die", "c", 300, compose("app", json!({ "exitCode": "137" }))),
            ev("die", "d", 400, compose("migrate", json!({ "exitCode": "0" }))),
            ev("health_status: unhealthy", "e", 500, compose("app", json!({}))),
            ev("health_status: healthy", "e", 600, compose("app", json!({}))),
            ev("exec_die", "e", 700, compose("app", json!({ "exitCode": "1" }))),
            // A kill long ago does not explain this exit.
            ev("die", "b", 900, compose("app", json!({ "exitCode": "1" }))),
        ]
        .iter()
        .filter_map(|v| p.feed(v, &n))
        .map(|e| (e.kind, e.exit_code))
        .collect();
        assert_eq!(
            kinds,
            vec![
                (Kind::Start, None),
                (Kind::Crash, Some(1)),
                (Kind::Stop, Some(143)),
                (Kind::Oom, Some(137)),
                (Kind::Exit, Some(0)),
                (Kind::Unhealthy, None),
                (Kind::Crash, Some(1)),
            ]
        );
    }

    #[test]
    fn names_events_like_the_snapshot_names_services() {
        let mut p = Parser::default();
        let n = names();
        let e = p.feed(&ev("start", "a", 1, compose("app", json!({}))), &n).unwrap();
        assert_eq!((e.stack.as_str(), e.service.as_str(), e.replica.as_str(), e.node.as_str()), ("demo", "demo_app", "2", "box"));
        assert_eq!(e.key, "a:1000000000:start");

        let swarm = json!({ "com.docker.swarm.service.name": "shop_api", "com.docker.stack.namespace": "shop", "com.docker.swarm.task.name": "shop_api.3.k2j4" });
        let e = p.feed(&ev("start", "b", 1, swarm), &n).unwrap();
        assert_eq!((e.stack.as_str(), e.service.as_str(), e.replica.as_str()), ("shop", "shop_api", "3"));

        let global = json!({ "com.docker.swarm.service.name": "agent", "com.docker.swarm.task.name": "agent.n1.x9" });
        let e = p.feed(&ev("start", "c", 1, global), &n).unwrap();
        assert_eq!((e.stack.as_str(), e.service.as_str(), e.replica.as_str()), ("services", "agent", "worker-1"));

        let e = p.feed(&ev("start", "d", 1, json!({ "name": "lonely" })), &n).unwrap();
        assert_eq!((e.stack.as_str(), e.service.as_str(), e.replica.as_str()), ("standalone", "lonely", "1"));

        // `docker compose run` containers are not part of the service.
        assert!(p.feed(&ev("start", "e", 1, compose("app", json!({ "com.docker.compose.oneoff": "True" }))), &n).is_none());
    }

    #[test]
    fn times_are_sent_in_seconds() {
        assert_eq!(secs(1_700_000_000_123_456), "1700000000.123456");
        assert_eq!(secs(5), "0.000005");
    }
}
