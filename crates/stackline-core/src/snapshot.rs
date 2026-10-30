//! Builds a [`Snapshot`] from the Docker Engine API (GET requests only).

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::Result;
use futures_util::StreamExt;
use serde_json::Value;

use crate::docker::DockerClient;
use crate::model::*;

const COMPOSE_PROJECT: &str = "com.docker.compose.project";
const COMPOSE_SERVICE: &str = "com.docker.compose.service";
const COMPOSE_NUMBER: &str = "com.docker.compose.container-number";
const COMPOSE_ONEOFF: &str = "com.docker.compose.oneoff";
const STACK_NAMESPACE: &str = "com.docker.stack.namespace";
const SWARM_TASK: &str = "com.docker.swarm.task.id";

pub struct Options<'a> {
    /// Also read CPU and memory usage per running container.
    pub stats: bool,
    /// The CPU counters of each container at the last refresh, to compute
    /// usage from. Updated by every fetch.
    pub cpu: &'a std::sync::Mutex<CpuSamples>,
    /// Whether a container that exited with 137 was killed for running out
    /// of memory, by container id, so each one is inspected once.
    pub oom: &'a std::sync::Mutex<OomKills>,
}

pub type OomKills = HashMap<String, bool>;

/// Total CPU time of a container and of the whole host.
pub type CpuSample = (u64, u64);
pub type CpuSamples = HashMap<String, CpuSample>;
/// CPU percent (unknown until there are two samples) and memory bytes.
type Usage = (Option<f64>, u64);

/// Podman answers the Docker API too; its version lists a "Podman Engine" component.
pub fn engine_name(version: &Value) -> String {
    let podman = version["Components"].as_array().is_some_and(|c| c.iter().any(|c| s(&c["Name"]).starts_with("Podman")))
        || s(&version["Platform"]["Name"]).contains("Podman");
    if podman { "Podman" } else { "Docker" }.into()
}

pub async fn fetch(client: &DockerClient, opts: &Options<'_>) -> Result<Snapshot> {
    let info: Value = client.get_json("/info").await?;
    let version: Value = client.get_json("/version").await.unwrap_or(Value::Null);
    let mut containers: Vec<Value> = client.get_json("/containers/json?all=1").await?;
    mark_oom_kills(client, &mut containers, opts.oom).await;

    let mut warnings = Vec::new();
    let swarm_state = s(&info["Swarm"]["LocalNodeState"]);
    let is_manager = info["Swarm"]["ControlAvailable"].as_bool().unwrap_or(false);
    let local_node_id = s(&info["Swarm"]["NodeID"]);

    let cluster_id = Some(s(&info["Swarm"]["Cluster"]["ID"])).filter(|c| !c.is_empty() && is_manager);
    let host = Host {
        id: s(&info["ID"]),
        cluster_id,
        name: s(&info["Name"]),
        os: s(&info["OperatingSystem"]),
        kernel: s(&info["KernelVersion"]),
        engine: engine_name(&version),
        docker_version: s(&info["ServerVersion"]),
        api_version: s(&version["ApiVersion"]),
        ncpu: u(&info["NCPU"]) as u32,
        mem_total: u(&info["MemTotal"]),
        containers_running: u(&info["ContainersRunning"]) as u32,
        containers_total: u(&info["Containers"]) as u32,
        images: u(&info["Images"]) as u32,
        swarm_state: if swarm_state.is_empty() { "inactive".into() } else { swarm_state.clone() },
        is_manager,
        cpu_percent: None,
        mem_used: None,
    };

    let mut stacks: Vec<Stack> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut task_failures = Vec::new();
    let mode = if swarm_state == "active" && is_manager { Mode::Swarm } else { Mode::Standalone };
    if swarm_state == "active" && !is_manager {
        warnings
            .push("This node is a swarm worker. Connect to a manager node to see swarm services; showing local containers only.".into());
    }

    if mode == Mode::Swarm {
        let raw_nodes: Vec<Value> = client.get_json("/nodes").await?;
        let services: Vec<Value> = client.get_json("/services?status=true").await?;
        let tasks: Vec<Value> = client.get_json("/tasks").await?;
        nodes = raw_nodes.iter().map(|n| map_node(n, &local_node_id)).collect();
        let hostnames: HashMap<String, String> = nodes.iter().map(|n| (n.id.clone(), n.hostname.clone())).collect();
        stacks = map_swarm(&services, &tasks, &hostnames);
        task_failures = failed_tasks(&services, &tasks, &hostnames);
        for n in &mut nodes {
            n.tasks_running = stacks
                .iter()
                .flat_map(|s| &s.services)
                .flat_map(|s| &s.tasks)
                .filter(|t| t.node == n.hostname && t.state == "running")
                .count() as u32;
        }
    } else {
        nodes.push(Node {
            id: local_node_id.clone(),
            hostname: host.name.clone(),
            role: "standalone".into(),
            leader: false,
            reachability: None,
            availability: "active".into(),
            state: "ready".into(),
            addr: String::new(),
            engine: host.docker_version.clone(),
            os: host.os.clone(),
            ncpu: host.ncpu,
            mem_total: host.mem_total,
            labels: BTreeMap::new(),
            is_local: true,
            cpu_percent: None,
            mem_used: None,
            tasks_running: host.containers_running,
        });
    }
    let local_host = nodes.iter().find(|n| n.is_local).map(|n| n.hostname.clone()).unwrap_or(host.name.clone());
    stacks.extend(map_containers(&containers, &local_host));
    stacks.sort_by(|a, b| (a.kind == StackKind::Standalone, &a.name).cmp(&(b.kind == StackKind::Standalone, &b.name)));

    let mut snap = Snapshot { mode, host, nodes, stacks, fetched_at: chrono::Utc::now().timestamp_millis(), warnings, task_failures };
    if opts.stats {
        add_stats(client, &mut snap, &containers, opts.cpu).await;
    }
    Ok(snap)
}

fn map_node(n: &Value, local_id: &str) -> Node {
    let mgr = &n["ManagerStatus"];
    let labels = n["Spec"]["Labels"]
        .as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect())
        .unwrap_or_default();
    Node {
        id: s(&n["ID"]),
        hostname: s(&n["Description"]["Hostname"]),
        role: s(&n["Spec"]["Role"]),
        leader: mgr["Leader"].as_bool().unwrap_or(false),
        reachability: mgr["Reachability"].as_str().map(str::to_string),
        availability: s(&n["Spec"]["Availability"]),
        state: s(&n["Status"]["State"]),
        addr: s(&n["Status"]["Addr"]),
        engine: s(&n["Description"]["Engine"]["EngineVersion"]),
        os: format!("{} {}", s(&n["Description"]["Platform"]["OS"]), s(&n["Description"]["Platform"]["Architecture"])).trim().to_string(),
        ncpu: (u(&n["Description"]["Resources"]["NanoCPUs"]) / 1_000_000_000) as u32,
        mem_total: u(&n["Description"]["Resources"]["MemoryBytes"]),
        labels,
        is_local: s(&n["ID"]) == local_id,
        cpu_percent: None,
        mem_used: None,
        tasks_running: 0,
    }
}

fn map_swarm(services: &[Value], tasks: &[Value], hostnames: &HashMap<String, String>) -> Vec<Stack> {
    // Latest task per (service, slot); for global services the slot is the node.
    let mut latest: HashMap<(String, String), &Value> = HashMap::new();
    for t in tasks {
        let svc = s(&t["ServiceID"]);
        let slot = match t["Slot"].as_u64() {
            Some(n) if n > 0 => n.to_string(),
            _ => s(&t["NodeID"]),
        };
        let key = (svc, slot);
        let newer = match latest.get(&key) {
            Some(prev) => ts(&t["CreatedAt"]) > ts(&prev["CreatedAt"]),
            None => true,
        };
        // Prefer a task that should be running over a newer one that was shut down.
        let prefer = match latest.get(&key) {
            Some(prev) => {
                let want_new = s(&t["DesiredState"]) == "running";
                let want_old = s(&prev["DesiredState"]) == "running";
                (want_new && !want_old) || (want_new == want_old && newer)
            }
            None => true,
        };
        if prefer {
            latest.insert(key, t);
        }
    }

    // The most recent failure per slot, shown while a replacement task is starting,
    // and how many tasks of that slot failed.
    let mut last_failure: HashMap<(String, String), (i64, String)> = HashMap::new();
    let mut failures: HashMap<(String, String), u32> = HashMap::new();
    for t in tasks {
        let slot = match t["Slot"].as_u64() {
            Some(n) if n > 0 => n.to_string(),
            _ => s(&t["NodeID"]),
        };
        let key = (s(&t["ServiceID"]), slot);
        if matches!(s(&t["Status"]["State"]).as_str(), "failed" | "rejected") {
            *failures.entry(key.clone()).or_default() += 1;
        }
        let err = t["Status"]["Err"].as_str().unwrap_or("");
        if err.is_empty() {
            continue;
        }
        let when = ts(&t["Status"]["Timestamp"]);
        if last_failure.get(&key).is_none_or(|(w, _)| when > *w) {
            last_failure.insert(key, (when, err.to_string()));
        }
    }

    let mut by_stack: BTreeMap<String, Vec<Service>> = BTreeMap::new();
    for sv in services {
        let spec = &sv["Spec"];
        let id = s(&sv["ID"]);
        let full = s(&spec["Name"]);
        let stack = spec["Labels"][STACK_NAMESPACE].as_str().unwrap_or("").to_string();
        let name = if !stack.is_empty() { full.strip_prefix(&format!("{stack}_")).unwrap_or(&full).to_string() } else { full.clone() };
        let global = spec["Mode"].get("Global").is_some();
        let cs = &spec["TaskTemplate"]["ContainerSpec"];
        let mut svc_tasks: Vec<Task> = latest
            .iter()
            .filter(|((sid, _), _)| *sid == id)
            .filter(|(_, t)| s(&t["DesiredState"]) == "running" || ts(&t["Status"]["Timestamp"]) > now_ms() - 24 * 3600 * 1000)
            .map(|((_, slot), t)| {
                let node_id = s(&t["NodeID"]);
                let node = hostnames.get(&node_id).cloned().unwrap_or(node_id.clone());
                let st = &t["Status"];
                Task {
                    id: s(&t["ID"]),
                    slot: if global { node.clone() } else { slot.clone() },
                    node,
                    state: s(&st["State"]),
                    desired_state: s(&t["DesiredState"]),
                    message: st["Message"].as_str().map(str::to_string),
                    error: st["Err"].as_str().filter(|e| !e.is_empty()).map(str::to_string).or_else(|| {
                        (s(&st["State"]) != "running")
                            .then(|| last_failure.get(&(id.clone(), slot.clone())).map(|(_, e)| e.clone()))
                            .flatten()
                    }),
                    health: None,
                    container_id: st["ContainerStatus"]["ContainerID"].as_str().filter(|c| !c.is_empty()).map(str::to_string),
                    container_name: None,
                    since: ts_opt(&st["Timestamp"]),
                    exit_code: st["ContainerStatus"]["ExitCode"].as_i64().filter(|c| *c != 0),
                    failures: {
                        let n = failures.get(&(id.clone(), slot.clone())).copied().unwrap_or(0);
                        // The current task is counted when it failed itself.
                        if matches!(s(&st["State"]).as_str(), "failed" | "rejected") { n.saturating_sub(1) } else { n }
                    },
                    cpu_percent: None,
                    mem_used: None,
                }
            })
            .collect();
        svc_tasks.sort_by_key(|a| natural(&a.slot));
        let ports = sv["Endpoint"]["Ports"]
            .as_array()
            .map(|ps| ps.iter().map(|p| format!("{}→{}/{}", u(&p["PublishedPort"]), u(&p["TargetPort"]), s(&p["Protocol"]))).collect())
            .unwrap_or_default();
        let st = &sv["ServiceStatus"];
        let desired = st["DesiredTasks"].as_u64().unwrap_or_else(|| u(&spec["Mode"]["Replicated"]["Replicas"])) as u32;
        let running =
            st["RunningTasks"].as_u64().unwrap_or_else(|| svc_tasks.iter().filter(|t| t.state == "running").count() as u64) as u32;
        by_stack.entry(stack.clone()).or_default().push(Service {
            id,
            name,
            full_name: full,
            stack: if stack.is_empty() { "services".into() } else { stack },
            image: strip_digest(&s(&cs["Image"])),
            mode: if global { "global".into() } else { "replicated".into() },
            desired,
            running,
            ports,
            updated_at: ts_opt(&sv["UpdatedAt"]),
            tty: cs["TTY"].as_bool().unwrap_or(false),
            tasks: svc_tasks,
            cpu_percent: None,
            mem_used: None,
            config: swarm_config(spec),
        });
    }
    by_stack
        .into_iter()
        .map(|(name, mut services)| {
            services.sort_by(|a, b| a.name.cmp(&b.name));
            Stack { name: if name.is_empty() { "services".into() } else { name }, kind: StackKind::Swarm, services }
        })
        .collect()
}

/// Every task in the swarm's history that failed or was rejected, on any node.
fn failed_tasks(services: &[Value], tasks: &[Value], hostnames: &HashMap<String, String>) -> Vec<TaskFailure> {
    let by_id: HashMap<String, &Value> = services.iter().map(|sv| (s(&sv["ID"]), sv)).collect();
    tasks
        .iter()
        .filter(|t| matches!(s(&t["Status"]["State"]).as_str(), "failed" | "rejected"))
        .filter_map(|t| {
            let sv = by_id.get(&s(&t["ServiceID"]))?;
            let spec = &sv["Spec"];
            let stack = spec["Labels"][STACK_NAMESPACE].as_str().filter(|x| !x.is_empty()).unwrap_or("services").to_string();
            let node_id = s(&t["NodeID"]);
            let node = hostnames.get(&node_id).cloned().unwrap_or(node_id);
            let replica = match t["Slot"].as_u64() {
                Some(n) if n > 0 => n.to_string(),
                _ => node.clone(),
            };
            let st = &t["Status"];
            Some(TaskFailure {
                task_id: s(&t["ID"]),
                ts: chrono::DateTime::parse_from_rfc3339(st["Timestamp"].as_str()?).ok()?.timestamp_micros(),
                stack,
                service: s(&spec["Name"]),
                replica,
                node,
                container: s(&st["ContainerStatus"]["ContainerID"]),
                exit_code: st["ContainerStatus"]["ExitCode"].as_i64().filter(|c| *c != 0),
                error: st["Err"].as_str().filter(|e| !e.is_empty()).map(str::to_string),
            })
        })
        .collect()
}

fn map_containers(containers: &[Value], local_host: &str) -> Vec<Stack> {
    // project -> service -> containers
    let mut projects: BTreeMap<String, BTreeMap<String, Vec<&Value>>> = BTreeMap::new();
    let mut loose: Vec<&Value> = Vec::new();
    for c in containers {
        let labels = &c["Labels"];
        if labels.get(SWARM_TASK).is_some() {
            continue;
        }
        match (labels[COMPOSE_PROJECT].as_str(), labels[COMPOSE_SERVICE].as_str()) {
            (Some(p), Some(svc)) => projects.entry(p.to_string()).or_default().entry(svc.to_string()).or_default().push(c),
            _ => loose.push(c),
        }
    }
    let mut out = Vec::new();
    for (project, services) in projects {
        let services = services
            .into_iter()
            .map(|(name, cs)| container_service(&project, &name, &format!("{project}_{name}"), "compose", &cs, local_host))
            .collect();
        out.push(Stack { name: project, kind: StackKind::Compose, services });
    }
    if !loose.is_empty() {
        let mut services: Vec<Service> = loose
            .iter()
            .map(|c| {
                let name = container_name(c);
                container_service("standalone", &name, &name, "container", &[*c], local_host)
            })
            .collect();
        services.sort_by(|a, b| a.name.cmp(&b.name));
        out.push(Stack { name: "standalone".into(), kind: StackKind::Standalone, services });
    }
    out
}

fn container_service(stack: &str, name: &str, full: &str, mode: &str, cs: &[&Value], host: &str) -> Service {
    let mut tasks: Vec<Task> = cs
        .iter()
        .filter(|c| c["Labels"][COMPOSE_ONEOFF].as_str() != Some("True"))
        .map(|c| {
            let status = s(&c["Status"]);
            let state = s(&c["State"]);
            let exit_code = exit_code_of(&status);
            let oom = c["OOMKilled"].as_bool() == Some(true);
            Task {
                id: s(&c["Id"]),
                slot: c["Labels"][COMPOSE_NUMBER].as_str().map(str::to_string).unwrap_or_else(|| "1".into()),
                node: host.to_string(),
                state: state.clone(),
                desired_state: if oom || expected_to_run(&state, exit_code) { "running".into() } else { state.clone() },
                health: health_of(&status),
                exit_code,
                message: Some(status),
                error: oom.then(|| "Out of memory: the kernel stopped it at its memory limit".to_string()),
                container_id: Some(s(&c["Id"])),
                container_name: Some(container_name(c)),
                since: c["Created"].as_i64().map(|t| t * 1000),
                failures: 0,
                cpu_percent: None,
                mem_used: None,
            }
        })
        .collect();
    tasks.sort_by_key(|a| natural(&a.slot));
    let running = tasks.iter().filter(|t| t.state == "running").count() as u32;
    let desired = tasks.iter().filter(|t| t.desired_state == "running").count() as u32;
    let mut ports: Vec<String> = Vec::new();
    for c in cs {
        for p in c["Ports"].as_array().into_iter().flatten() {
            if let Some(public) = p["PublicPort"].as_u64() {
                let s = format!("{}→{}/{}", public, u(&p["PrivatePort"]), s(&p["Type"]));
                if !ports.contains(&s) {
                    ports.push(s);
                }
            }
        }
    }
    let first = cs.first().copied().unwrap_or(&Value::Null);
    Service {
        id: full.to_string(),
        name: name.to_string(),
        full_name: full.to_string(),
        stack: stack.to_string(),
        image: strip_digest(&s(&first["Image"])),
        mode: mode.to_string(),
        desired,
        running,
        ports,
        updated_at: cs.iter().filter_map(|c| c["Created"].as_i64()).max().map(|t| t * 1000),
        tty: false,
        tasks,
        cpu_percent: None,
        mem_used: None,
        config: container_config(cs),
    }
}

/// What a swarm service runs, from its spec. See `deploys`.
fn swarm_config(spec: &Value) -> crate::deploys::Config {
    let mut c = crate::deploys::Config::new();
    let t = &spec["TaskTemplate"];
    let cs = &t["ContainerSpec"];
    let mut put = |k: &str, v: String| {
        if !v.is_empty() && v != "null" {
            c.insert(k.to_string(), v);
        }
    };
    put("image", strip_digest(&s(&cs["Image"])));
    put("image id", s(&cs["Image"]).split_once('@').map(|(_, d)| d.to_string()).unwrap_or_default());
    put("command", cs["Command"].to_string());
    put("args", cs["Args"].to_string());
    put("replicas", spec["Mode"]["Replicated"]["Replicas"].as_u64().map(|n| n.to_string()).unwrap_or_default());
    put("ports", spec["EndpointSpec"]["Ports"].to_string());
    put("mounts", cs["Mounts"].to_string());
    put("limits", t["Resources"].to_string());
    put("placement", t["Placement"]["Constraints"].to_string());
    put("secrets", names(&cs["Secrets"], "SecretName"));
    put("configs", names(&cs["Configs"], "ConfigName"));
    put("networks", t["Networks"].to_string());
    put("healthcheck", cs["Healthcheck"].to_string());
    put("forced", t["ForceUpdate"].as_u64().filter(|n| *n > 0).map(|n| n.to_string()).unwrap_or_default());
    for e in cs["Env"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let (k, v) = e.split_once('=').unwrap_or((e, ""));
        put(&format!("env:{k}"), crate::deploys::hash(v));
    }
    c
}

/// What the containers of a Compose service or a single container run. The
/// container list has no environment, but Compose's config hash covers it.
fn container_config(cs: &[&Value]) -> crate::deploys::Config {
    let mut c = crate::deploys::Config::new();
    let Some(first) = cs.iter().max_by_key(|c| c["Created"].as_i64()) else { return c };
    let mut put = |k: &str, v: String| {
        if !v.is_empty() {
            c.insert(k.to_string(), v);
        }
    };
    put("image", strip_digest(&s(&first["Image"])));
    put("image id", s(&first["ImageID"]));
    put("command", s(&first["Command"]));
    put("compose config", s(&first["Labels"]["com.docker.compose.config-hash"]));
    // By number: while Compose recreates a container, the old and the new one both exist.
    let numbers: std::collections::BTreeSet<&str> = cs
        .iter()
        .filter(|c| c["Labels"][COMPOSE_ONEOFF].as_str() != Some("True"))
        .map(|c| c["Labels"][COMPOSE_NUMBER].as_str().unwrap_or("1"))
        .collect();
    put("replicas", numbers.len().to_string());
    c
}

fn names(v: &Value, key: &str) -> String {
    let mut n: Vec<String> = v.as_array().into_iter().flatten().map(|x| s(&x[key])).collect();
    n.sort();
    n.join(", ")
}

/// Exit code 137 is SIGKILL: `docker stop` after its timeout, or the kernel
/// when the container ran out of memory. Only the container's details tell
/// them apart, so those containers are inspected, each one once.
async fn mark_oom_kills(client: &DockerClient, containers: &mut [Value], known: &std::sync::Mutex<OomKills>) {
    let killed: Vec<String> = containers
        .iter()
        .filter(|c| s(&c["State"]) == "exited" && exit_code_of(&s(&c["Status"])) == Some(137))
        .map(|c| s(&c["Id"]))
        .collect();
    let unknown: Vec<String> = {
        let mut k = known.lock().unwrap();
        k.retain(|id, _| killed.contains(id));
        killed.iter().filter(|id| !k.contains_key(*id)).cloned().collect()
    };
    let found: Vec<(String, bool)> = futures_util::stream::iter(unknown)
        .map(|id| async move {
            let v = client.get_json::<Value>(&format!("/containers/{id}/json")).await;
            (id, v.ok().map(|v| v["State"]["OOMKilled"].as_bool().unwrap_or(false)))
        })
        .buffer_unordered(6)
        .filter_map(|(id, oom)| async move { oom.map(|o| (id, o)) })
        .collect()
        .await;
    let mut k = known.lock().unwrap();
    k.extend(found);
    for c in containers.iter_mut() {
        if k.get(&s(&c["Id"])) == Some(&true) {
            c["OOMKilled"] = Value::Bool(true);
        }
    }
}

async fn add_stats(client: &DockerClient, snap: &mut Snapshot, containers: &[Value], samples: &std::sync::Mutex<CpuSamples>) {
    let running: Vec<String> = containers.iter().filter(|c| s(&c["State"]) == "running").map(|c| s(&c["Id"])).collect();
    // `one-shot` answers at once with a single sample; without it Docker
    // waits a second per container for a second sample. CPU usage then comes
    // from the samples of the previous refresh. Engines before API 1.41
    // ignore it and still send both samples.
    let read = |ids: Vec<String>| {
        futures_util::stream::iter(ids)
            .map(|id| async move {
                let path = format!("/containers/{id}/stats?stream=false&one-shot=true");
                let r = tokio::time::timeout(Duration::from_secs(6), client.get_json::<Value>(&path)).await;
                (id, r.ok().and_then(Result::ok))
            })
            .buffer_unordered(12)
            .collect::<Vec<(String, Option<Value>)>>()
    };
    let parse = |results: Vec<(String, Option<Value>)>, prev: &CpuSamples, next: &mut CpuSamples, stats: &mut HashMap<String, Usage>| {
        for (id, v) in results {
            if let Some((usage, sample)) = v.and_then(|v| parse_stats(&v, prev.get(&id).copied())) {
                next.insert(id.clone(), sample);
                stats.insert(id, usage);
            }
        }
    };
    let prev = samples.lock().unwrap().clone();
    let (mut next, mut stats) = (CpuSamples::new(), HashMap::new());
    parse(read(running).await, &prev, &mut next, &mut stats);
    // Containers seen for the first time (all of them on the first refresh)
    // have no earlier sample: read them once more a moment later, so CPU
    // shows at once instead of after the next refresh.
    let new: Vec<String> = stats.iter().filter(|(_, (cpu, _))| cpu.is_none()).map(|(id, _)| id.clone()).collect();
    if !new.is_empty() {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let first = next.clone();
        parse(read(new).await, &first, &mut next, &mut stats);
    }
    *samples.lock().unwrap() = next;
    if stats.is_empty() {
        return;
    }
    let ncpu = snap.host.ncpu.max(1) as f64;
    let (mut cpu_sum, mut mem_sum) = (0.0, 0u64);
    for st in &mut snap.stacks {
        for svc in &mut st.services {
            let (mut c, mut m, mut any) = (0.0, 0u64, false);
            let mut known = true;
            for t in &mut svc.tasks {
                if let Some((cpu, mem)) = t.container_id.as_ref().and_then(|id| stats.get(id)) {
                    t.cpu_percent = *cpu;
                    t.mem_used = Some(*mem);
                    c += cpu.unwrap_or(0.0);
                    known &= cpu.is_some();
                    m += mem;
                    any = true;
                }
            }
            if any {
                svc.cpu_percent = known.then_some(c);
                svc.mem_used = Some(m);
            }
        }
    }
    let mut known = true;
    for (cpu, mem) in stats.values() {
        cpu_sum += cpu.unwrap_or(0.0);
        known &= cpu.is_some();
        mem_sum += mem;
    }
    snap.host.cpu_percent = known.then_some((cpu_sum / ncpu).min(100.0));
    snap.host.mem_used = Some(mem_sum);
    if let Some(n) = snap.nodes.iter_mut().find(|n| n.is_local) {
        n.cpu_percent = snap.host.cpu_percent;
        n.mem_used = snap.host.mem_used;
    }
}

/// CPU percent of one core-equivalent (100 = one full core) and memory bytes,
/// plus this sample's CPU counters. CPU is unknown until there are two samples:
/// the one Docker sent before this one, or `prev` from the last refresh.
fn parse_stats(v: &Value, prev: Option<CpuSample>) -> Option<(Usage, CpuSample)> {
    let cpu = &v["cpu_stats"];
    let pre = &v["precpu_stats"];
    let sample = (u(&cpu["cpu_usage"]["total_usage"]), u(&cpu["system_cpu_usage"]));
    let before = match u(&pre["system_cpu_usage"]) {
        0 => prev,
        sys => Some((u(&pre["cpu_usage"]["total_usage"]), sys)),
    };
    let online = cpu["online_cpus"].as_u64().unwrap_or(1).max(1) as f64;
    let pct = before.map(|(total, sys)| {
        let (cpu_delta, sys_delta) = (sample.0 as f64 - total as f64, sample.1 as f64 - sys as f64);
        if cpu_delta > 0.0 && sys_delta > 0.0 { cpu_delta / sys_delta * online * 100.0 } else { 0.0 }
    });
    let mem = &v["memory_stats"];
    let usage = u(&mem["usage"]);
    let cache = mem["stats"]["inactive_file"].as_u64().or_else(|| mem["stats"]["cache"].as_u64()).unwrap_or(0);
    if usage == 0 && cpu.is_null() {
        return None;
    }
    Some(((pct, usage.saturating_sub(cache)), sample))
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn u(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_i64().map(|i| i.max(0) as u64)).unwrap_or(0)
}

fn ts(v: &Value) -> i64 {
    ts_opt(v).unwrap_or(0)
}

fn ts_opt(v: &Value) -> Option<i64> {
    v.as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|d| d.timestamp_millis()).filter(|t| *t > 0)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn strip_digest(image: &str) -> String {
    image.split('@').next().unwrap_or(image).to_string()
}

fn container_name(c: &Value) -> String {
    c["Names"][0].as_str().unwrap_or("").trim_start_matches('/').to_string()
}

fn health_of(status: &str) -> Option<String> {
    for h in ["unhealthy", "healthy", "health: starting"] {
        if status.contains(&format!("({h})")) {
            return Some(h.trim_start_matches("health: ").to_string());
        }
    }
    None
}

/// Whether a container should be running. A container that was never started,
/// is paused, finished cleanly (a one-shot job) or was stopped (`docker stop`
/// or `docker compose stop` end it with SIGTERM, 143, or SIGKILL, 137) is
/// stopped on purpose. One that crashed with any other code, or keeps
/// restarting, is not.
fn expected_to_run(state: &str, exit_code: Option<i64>) -> bool {
    match state {
        "created" | "paused" => false,
        "exited" => !matches!(exit_code, None | Some(137) | Some(143)),
        _ => true,
    }
}

fn exit_code_of(status: &str) -> Option<i64> {
    let rest = status.strip_prefix("Exited (")?;
    let code: i64 = rest.split(')').next()?.parse().ok()?;
    (code != 0).then_some(code)
}

/// Sort key that orders "2" before "10".
fn natural(s: &str) -> (u64, String) {
    (s.parse().unwrap_or(u64::MAX), s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tells_podman_from_docker() {
        let podman =
            json!({ "Platform": { "Name": "linux/amd64/fedora-40" }, "Components": [{ "Name": "Podman Engine", "Version": "5.2.3" }] });
        let docker =
            json!({ "Platform": { "Name": "Docker Engine - Community" }, "Components": [{ "Name": "Engine", "Version": "27.3.1" }] });
        assert_eq!(engine_name(&podman), "Podman");
        assert_eq!(engine_name(&docker), "Docker");
        assert_eq!(engine_name(&Value::Null), "Docker");
    }

    #[test]
    fn groups_compose_containers_into_stacks() {
        let cs = vec![
            json!({"Id":"a1","Names":["/shop-app-1"],"Image":"shop:1","State":"running","Status":"Up 2 hours (healthy)","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"app","com.docker.compose.container-number":"1"},
                   "Ports":[{"PrivatePort":8080,"PublicPort":80,"Type":"tcp"},{"PrivatePort":8080,"PublicPort":80,"Type":"tcp"}]}),
            json!({"Id":"m1","Names":["/shop-migrate-1"],"Image":"shop:1","State":"exited","Status":"Exited (0) 2 hours ago","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"migrate"}}),
            json!({"Id":"x1","Names":["/scratch"],"Image":"alpine@sha256:abc","State":"exited","Status":"Exited (1) 1 day ago","Created":1700000000,"Labels":{}}),
            json!({"Id":"w1","Names":["/shop-worker-1"],"Image":"shop:1","State":"exited","Status":"Exited (143) 5 minutes ago","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"worker"}}),
            json!({"Id":"o1","Names":["/shop-cache-1"],"Image":"shop:1","State":"exited","Status":"Exited (137) 1 minute ago","Created":1700000000,"OOMKilled":true,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"cache"}}),
            json!({"Id":"c1","Names":["/shop-cron-1"],"Image":"shop:1","State":"created","Status":"Created","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"cron"}}),
        ];
        let stacks = map_containers(&cs, "shop");
        assert_eq!(stacks.len(), 2);
        let app = &stacks[0].services[0];
        assert_eq!((app.full_name.as_str(), app.desired, app.running), ("shop_app", 1, 1));
        assert_eq!(app.ports, vec!["80→8080/tcp"]);
        assert_eq!(app.tasks[0].health.as_deref(), Some("healthy"));
        let by = |n: &str| stacks[0].services.iter().find(|s| s.name == n).unwrap();
        // Finished, stopped on purpose or never started: none of them counts as down.
        for n in ["migrate", "worker", "cron"] {
            assert_eq!((by(n).desired, by(n).running), (0, 0), "{n}");
        }
        assert_eq!(by("worker").tasks[0].exit_code, Some(143));
        // Killed for running out of memory: down, and it says why.
        assert_eq!((by("cache").desired, by("cache").running), (1, 0));
        assert!(by("cache").tasks[0].error.as_deref().is_some_and(|e| e.contains("Out of memory")));
        // A crash still does.
        let scratch = &stacks[1].services[0];
        assert_eq!((scratch.image.as_str(), scratch.desired, scratch.tasks[0].exit_code), ("alpine", 1, Some(1)));
    }

    #[test]
    fn cpu_from_one_shot_samples() {
        let sample = |total: u64, sys: u64, pre: Option<(u64, u64)>| {
            json!({
                "cpu_stats": { "cpu_usage": { "total_usage": total }, "system_cpu_usage": sys, "online_cpus": 4 },
                "precpu_stats": pre.map(|(t, s)| json!({ "cpu_usage": { "total_usage": t }, "system_cpu_usage": s })).unwrap_or(json!({})),
                "memory_stats": { "usage": 3000, "stats": { "inactive_file": 1000 } },
            })
        };
        // The first one-shot sample: memory, but no CPU yet.
        let ((cpu, mem), next) = parse_stats(&sample(100, 1000, None), None).unwrap();
        assert_eq!((cpu, mem, next), (None, 2000, (100, 1000)));
        // The next refresh computes CPU from the previous sample.
        let ((cpu, _), _) = parse_stats(&sample(150, 1200, None), Some(next)).unwrap();
        assert_eq!(cpu, Some(50.0 / 200.0 * 4.0 * 100.0));
        // Older engines send both samples themselves.
        let ((cpu, _), _) = parse_stats(&sample(150, 1200, Some((140, 1100))), None).unwrap();
        assert_eq!(cpu, Some(10.0 / 100.0 * 4.0 * 100.0));
    }
}
