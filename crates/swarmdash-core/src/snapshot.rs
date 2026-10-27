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

pub struct Options {
    /// Also read CPU and memory usage per running container (about one second per call).
    pub stats: bool,
}

pub async fn fetch(client: &DockerClient, opts: &Options) -> Result<Snapshot> {
    let info: Value = client.get_json("/info").await?;
    let version: Value = client.get_json("/version").await.unwrap_or(Value::Null);
    let containers: Vec<Value> = client.get_json("/containers/json?all=1").await?;

    let mut warnings = Vec::new();
    let swarm_state = s(&info["Swarm"]["LocalNodeState"]);
    let is_manager = info["Swarm"]["ControlAvailable"].as_bool().unwrap_or(false);
    let local_node_id = s(&info["Swarm"]["NodeID"]);

    let host = Host {
        name: s(&info["Name"]),
        os: s(&info["OperatingSystem"]),
        kernel: s(&info["KernelVersion"]),
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

    let mut snap = Snapshot { mode, host, nodes, stacks, fetched_at: chrono::Utc::now().timestamp_millis(), warnings };
    if opts.stats {
        add_stats(client, &mut snap, &containers).await;
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

    // The most recent failure per slot, shown while a replacement task is starting.
    let mut last_failure: HashMap<(String, String), (i64, String)> = HashMap::new();
    for t in tasks {
        let err = t["Status"]["Err"].as_str().unwrap_or("");
        if err.is_empty() {
            continue;
        }
        let slot = match t["Slot"].as_u64() {
            Some(n) if n > 0 => n.to_string(),
            _ => s(&t["NodeID"]),
        };
        let when = ts(&t["Status"]["Timestamp"]);
        let key = (s(&t["ServiceID"]), slot);
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
            Task {
                id: s(&c["Id"]),
                slot: c["Labels"][COMPOSE_NUMBER].as_str().map(str::to_string).unwrap_or_else(|| "1".into()),
                node: host.to_string(),
                state: state.clone(),
                desired_state: if state == "running" { "running".into() } else { state.clone() },
                health: health_of(&status),
                exit_code: exit_code_of(&status),
                message: Some(status),
                error: None,
                container_id: Some(s(&c["Id"])),
                container_name: Some(container_name(c)),
                since: c["Created"].as_i64().map(|t| t * 1000),
                cpu_percent: None,
                mem_used: None,
            }
        })
        .collect();
    tasks.sort_by_key(|a| natural(&a.slot));
    let running = tasks.iter().filter(|t| t.state == "running").count() as u32;
    // A container that exited cleanly (a one-shot job) is not expected to run.
    let desired = tasks.iter().filter(|t| !(t.state == "exited" && t.exit_code.is_none())).count() as u32;
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
    }
}

async fn add_stats(client: &DockerClient, snap: &mut Snapshot, containers: &[Value]) {
    let running: Vec<String> = containers.iter().filter(|c| s(&c["State"]) == "running").map(|c| s(&c["Id"])).collect();
    let results: Vec<(String, Option<(f64, u64)>)> = futures_util::stream::iter(running)
        .map(|id| async move {
            let path = format!("/containers/{id}/stats?stream=false");
            let r = tokio::time::timeout(Duration::from_secs(6), client.get_json::<Value>(&path)).await;
            let v = match r {
                Ok(Ok(v)) => parse_stats(&v),
                _ => None,
            };
            (id, v)
        })
        .buffer_unordered(12)
        .collect()
        .await;
    let stats: HashMap<String, (f64, u64)> = results.into_iter().filter_map(|(id, v)| v.map(|v| (id, v))).collect();
    if stats.is_empty() {
        return;
    }
    let ncpu = snap.host.ncpu.max(1) as f64;
    let (mut cpu_sum, mut mem_sum) = (0.0, 0u64);
    for st in &mut snap.stacks {
        for svc in &mut st.services {
            let (mut c, mut m, mut any) = (0.0, 0u64, false);
            for t in &mut svc.tasks {
                if let Some((cpu, mem)) = t.container_id.as_ref().and_then(|id| stats.get(id)) {
                    t.cpu_percent = Some(*cpu);
                    t.mem_used = Some(*mem);
                    c += cpu;
                    m += mem;
                    any = true;
                }
            }
            if any {
                svc.cpu_percent = Some(c);
                svc.mem_used = Some(m);
            }
        }
    }
    for (cpu, mem) in stats.values() {
        cpu_sum += cpu;
        mem_sum += mem;
    }
    snap.host.cpu_percent = Some((cpu_sum / ncpu).min(100.0));
    snap.host.mem_used = Some(mem_sum);
    if let Some(n) = snap.nodes.iter_mut().find(|n| n.is_local) {
        n.cpu_percent = snap.host.cpu_percent;
        n.mem_used = snap.host.mem_used;
    }
}

/// CPU percent of one core-equivalent (100 = one full core) and memory bytes.
fn parse_stats(v: &Value) -> Option<(f64, u64)> {
    let cpu = &v["cpu_stats"];
    let pre = &v["precpu_stats"];
    let cpu_delta = u(&cpu["cpu_usage"]["total_usage"]) as f64 - u(&pre["cpu_usage"]["total_usage"]) as f64;
    let sys_delta = u(&cpu["system_cpu_usage"]) as f64 - u(&pre["system_cpu_usage"]) as f64;
    let online = cpu["online_cpus"].as_u64().unwrap_or(1).max(1) as f64;
    let pct = if cpu_delta > 0.0 && sys_delta > 0.0 { cpu_delta / sys_delta * online * 100.0 } else { 0.0 };
    let mem = &v["memory_stats"];
    let usage = u(&mem["usage"]);
    let cache = mem["stats"]["inactive_file"].as_u64().or_else(|| mem["stats"]["cache"].as_u64()).unwrap_or(0);
    if usage == 0 && cpu.is_null() {
        return None;
    }
    Some((pct, usage.saturating_sub(cache)))
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
    fn groups_compose_containers_into_stacks() {
        let cs = vec![
            json!({"Id":"a1","Names":["/shop-app-1"],"Image":"shop:1","State":"running","Status":"Up 2 hours (healthy)","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"app","com.docker.compose.container-number":"1"},
                   "Ports":[{"PrivatePort":8080,"PublicPort":80,"Type":"tcp"},{"PrivatePort":8080,"PublicPort":80,"Type":"tcp"}]}),
            json!({"Id":"m1","Names":["/shop-migrate-1"],"Image":"shop:1","State":"exited","Status":"Exited (0) 2 hours ago","Created":1700000000,
                   "Labels":{"com.docker.compose.project":"shop","com.docker.compose.service":"migrate"}}),
            json!({"Id":"x1","Names":["/scratch"],"Image":"alpine@sha256:abc","State":"exited","Status":"Exited (137) 1 day ago","Created":1700000000,"Labels":{}}),
        ];
        let stacks = map_containers(&cs, "shop");
        assert_eq!(stacks.len(), 2);
        let app = &stacks[0].services[0];
        assert_eq!((app.full_name.as_str(), app.desired, app.running), ("shop_app", 1, 1));
        assert_eq!(app.ports, vec!["80→8080/tcp"]);
        assert_eq!(app.tasks[0].health.as_deref(), Some("healthy"));
        let migrate = &stacks[0].services[1];
        assert_eq!((migrate.desired, migrate.running), (0, 0));
        let scratch = &stacks[1].services[0];
        assert_eq!((scratch.image.as_str(), scratch.desired, scratch.tasks[0].exit_code), ("alpine", 1, Some(137)));
    }
}
