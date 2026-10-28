//! What swarmdash shows: one host (or swarm), its nodes, stacks, services and replicas.
//!
//! The same model covers swarm mode (stacks of swarm services, tasks as replicas)
//! and plain Docker (Compose projects as stacks, containers as replicas).

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Swarm,
    Standalone,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum StackKind {
    Swarm,
    Compose,
    Standalone,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub mode: Mode,
    pub host: Host,
    pub nodes: Vec<Node>,
    pub stacks: Vec<Stack>,
    /// Unix milliseconds.
    pub fetched_at: i64,
    pub warnings: Vec<String>,
    /// Tasks of swarm services that failed, from the swarm's task history on
    /// every node. Stored as events; the page reads them from there.
    #[serde(skip)]
    pub task_failures: Vec<TaskFailure>,
}

/// A swarm task that failed or was rejected.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskFailure {
    pub task_id: String,
    /// Unix microseconds.
    pub ts: i64,
    pub stack: String,
    pub service: String,
    pub replica: String,
    pub node: String,
    pub container: String,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    /// The Docker engine's ID; stays the same as long as the host does.
    pub id: String,
    /// The swarm's cluster ID when connected to a manager.
    pub cluster_id: Option<String>,
    pub name: String,
    pub os: String,
    pub kernel: String,
    pub docker_version: String,
    pub api_version: String,
    pub ncpu: u32,
    pub mem_total: u64,
    pub containers_running: u32,
    pub containers_total: u32,
    pub images: u32,
    pub swarm_state: String,
    pub is_manager: bool,
    /// Sum of container CPU usage as a share of all host CPUs (0–100).
    pub cpu_percent: Option<f64>,
    /// Sum of container memory usage in bytes.
    pub mem_used: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub hostname: String,
    pub role: String,
    pub leader: bool,
    pub reachability: Option<String>,
    pub availability: String,
    pub state: String,
    pub addr: String,
    pub engine: String,
    pub os: String,
    pub ncpu: u32,
    pub mem_total: u64,
    pub labels: BTreeMap<String, String>,
    /// True for the node swarmdash is connected to.
    pub is_local: bool,
    pub cpu_percent: Option<f64>,
    pub mem_used: Option<u64>,
    pub tasks_running: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub name: String,
    pub kind: StackKind,
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub id: String,
    /// Short name inside the stack, e.g. `api`.
    pub name: String,
    /// Unique name, e.g. `shop_api`.
    pub full_name: String,
    pub stack: String,
    pub image: String,
    /// `replicated`, `global`, `compose` or `container`.
    pub mode: String,
    pub desired: u32,
    pub running: u32,
    pub ports: Vec<String>,
    pub updated_at: Option<i64>,
    pub tty: bool,
    pub tasks: Vec<Task>,
    pub cpu_percent: Option<f64>,
    pub mem_used: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    /// Replica number (or node name for global services).
    pub slot: String,
    pub node: String,
    pub state: String,
    pub desired_state: String,
    /// Human status, e.g. `Up 3 hours (healthy)`.
    pub message: Option<String>,
    pub error: Option<String>,
    pub health: Option<String>,
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub since: Option<i64>,
    pub exit_code: Option<i64>,
    /// Earlier tasks of this slot that failed (swarm keeps a few in its history).
    pub failures: u32,
    pub cpu_percent: Option<f64>,
    pub mem_used: Option<u64>,
}

/// One stream of log lines: a container, or a swarm task on another node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSource {
    /// Stable key: the container or task id.
    pub key: String,
    pub via_task: bool,
    pub stack: String,
    pub service: String,
    pub replica: String,
    pub node: String,
    pub tty: bool,
    pub running: bool,
}

impl Snapshot {
    /// What the logs of this connection belong to: the swarm, or the single host.
    /// When it changes, the connection now points somewhere else.
    pub fn identity(&self) -> Option<String> {
        match (&self.host.cluster_id, self.host.id.is_empty()) {
            (Some(c), _) if !c.is_empty() => Some(format!("swarm:{c}")),
            (_, false) => Some(format!("engine:{}", self.host.id)),
            _ => None,
        }
    }

    pub fn log_sources(&self) -> Vec<LogSource> {
        let local_node = self.nodes.iter().find(|n| n.is_local).map(|n| n.hostname.clone());
        let mut out = Vec::new();
        for st in &self.stacks {
            for svc in &st.services {
                for t in &svc.tasks {
                    let on_local = local_node.as_deref() == Some(t.node.as_str()) || st.kind != StackKind::Swarm;
                    let (key, via_task) = match (&t.container_id, on_local) {
                        (Some(c), true) => (c.clone(), false),
                        _ if st.kind == StackKind::Swarm => (t.id.clone(), true),
                        _ => continue,
                    };
                    out.push(LogSource {
                        key,
                        via_task,
                        stack: st.name.clone(),
                        service: svc.full_name.clone(),
                        replica: t.slot.clone(),
                        node: t.node.clone(),
                        tty: svc.tty,
                        running: t.state == "running",
                    });
                }
            }
        }
        out
    }
}
