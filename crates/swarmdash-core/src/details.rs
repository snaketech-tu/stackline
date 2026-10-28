//! The full configuration of one service, read on demand when its details
//! open: `GET /services/{id}` for swarm services and `GET /containers/{id}/json`
//! for containers. Values of secret-looking environment variables never leave
//! this module.

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use crate::docker::DockerClient;
use crate::model::{Service, StackKind};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceDetails {
    /// The image with its tag and digest, as Docker reports it.
    pub image: String,
    /// Unix ms.
    pub created: Option<i64>,
    pub updated: Option<i64>,
    /// When the container (or the inspected replica) last started.
    pub started: Option<i64>,
    pub command: Option<String>,
    pub entrypoint: Option<String>,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub hostname: Option<String>,
    pub env: Vec<EnvVar>,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    pub networks: Vec<Network>,
    pub ports: Vec<String>,
    pub restart: Option<String>,
    pub restart_count: Option<u64>,
    pub oom_killed: bool,
    pub limits: Option<String>,
    pub reservations: Option<String>,
    pub healthcheck: Option<String>,
    /// The newest health check runs of the inspected container, newest first.
    pub health_log: Vec<HealthRun>,
    pub update_config: Option<String>,
    pub rollback_config: Option<String>,
    pub placement: Vec<String>,
    pub secrets: Vec<String>,
    pub configs: Vec<String>,
    pub log_driver: Option<String>,
    pub stop_grace: Option<String>,
    /// The container that the runtime facts (health, restarts) come from.
    pub container: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub name: String,
    /// `None` when the variable looks like a secret.
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub name: String,
    pub ip: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthRun {
    /// Unix ms.
    pub at: Option<i64>,
    pub exit_code: i64,
    pub output: String,
}

/// Reads the configuration of a service. For a swarm service it also reads the
/// newest running replica on this node, for its health checks and restarts.
pub async fn fetch(client: &DockerClient, svc: &Service, kind: StackKind) -> Result<ServiceDetails> {
    let running = svc.tasks.iter().find(|t| t.state == "running" && t.container_id.is_some());
    let container = running.or(svc.tasks.first()).and_then(|t| t.container_id.clone());
    if kind == StackKind::Swarm {
        let spec: Value = client.get_json(&format!("/services/{}", svc.id)).await?;
        let mut d = from_service(&spec);
        // Only containers on the node swarmdash is connected to can be inspected.
        if let Some(c) = container
            && let Ok(v) = client.get_json::<Value>(&format!("/containers/{c}/json")).await
        {
            add_runtime(&mut d, &v);
        }
        return Ok(d);
    }
    let Some(c) = container else { return Ok(ServiceDetails { image: svc.image.clone(), ..Default::default() }) };
    let v: Value = client.get_json(&format!("/containers/{c}/json")).await?;
    Ok(from_container(&v))
}

fn from_service(v: &Value) -> ServiceDetails {
    let spec = &v["Spec"];
    let task = &spec["TaskTemplate"];
    let cs = &task["ContainerSpec"];
    let mut d = ServiceDetails {
        image: text(&cs["Image"]).unwrap_or_default(),
        created: time(&v["CreatedAt"]),
        updated: time(&v["UpdatedAt"]),
        command: join(&cs["Args"]),
        entrypoint: join(&cs["Command"]),
        workdir: text(&cs["Dir"]),
        user: text(&cs["User"]),
        hostname: text(&cs["Hostname"]),
        env: env(&cs["Env"]),
        labels: labels(&spec["Labels"]),
        healthcheck: healthcheck(&cs["Healthcheck"]),
        stop_grace: cs["StopGracePeriod"].as_i64().map(duration),
        log_driver: text(&task["LogDriver"]["Name"]),
        update_config: update_config(&spec["UpdateConfig"]),
        rollback_config: update_config(&spec["RollbackConfig"]),
        ..Default::default()
    };
    for m in list(&cs["Mounts"]) {
        d.mounts.push(Mount {
            kind: text(&m["Type"]).unwrap_or_else(|| "volume".into()),
            source: text(&m["Source"]).unwrap_or_default(),
            target: text(&m["Target"]).unwrap_or_default(),
            read_only: m["ReadOnly"].as_bool().unwrap_or(false),
        });
    }
    for p in list(&spec["EndpointSpec"]["Ports"]) {
        let target = p["TargetPort"].as_u64().unwrap_or(0);
        let proto = text(&p["Protocol"]).unwrap_or_else(|| "tcp".into());
        let mode = text(&p["PublishMode"]).unwrap_or_else(|| "ingress".into());
        d.ports.push(match p["PublishedPort"].as_u64() {
            Some(pp) => format!("{pp}→{target}/{proto} ({mode})"),
            None => format!("{target}/{proto}"),
        });
    }
    let names: std::collections::HashMap<String, String> =
        list(&v["Endpoint"]["VirtualIPs"]).iter().filter_map(|ip| Some((text(&ip["NetworkID"])?, text(&ip["Addr"])?))).collect();
    for n in list(&task["Networks"]) {
        let id = text(&n["Target"]).unwrap_or_default();
        d.networks.push(Network { ip: names.get(&id).cloned(), name: id.chars().take(12).collect(), aliases: strings(&n["Aliases"]) });
    }
    let rp = &task["RestartPolicy"];
    if rp.is_object() {
        let mut r = text(&rp["Condition"]).unwrap_or_else(|| "any".into());
        if let Some(n) = rp["MaxAttempts"].as_u64().filter(|n| *n > 0) {
            r.push_str(&format!(", at most {n} attempts"));
        }
        if let Some(t) = rp["Delay"].as_i64().filter(|t| *t > 0) {
            r.push_str(&format!(", {} apart", duration(t)));
        }
        if let Some(t) = rp["Window"].as_i64().filter(|t| *t > 0) {
            r.push_str(&format!(", window {}", duration(t)));
        }
        d.restart = Some(r);
    }
    d.limits = resources(&task["Resources"]["Limits"], "NanoCPUs", "MemoryBytes");
    d.reservations = resources(&task["Resources"]["Reservations"], "NanoCPUs", "MemoryBytes");
    d.placement = strings(&task["Placement"]["Constraints"]);
    if let Some(n) = task["Placement"]["MaxReplicas"].as_u64().filter(|n| *n > 0) {
        d.placement.push(format!("at most {n} per node"));
    }
    d.secrets = list(&cs["Secrets"]).iter().filter_map(|s| text(&s["SecretName"])).collect();
    d.configs = list(&cs["Configs"]).iter().filter_map(|s| text(&s["ConfigName"])).collect();
    d
}

fn from_container(v: &Value) -> ServiceDetails {
    let cfg = &v["Config"];
    let host = &v["HostConfig"];
    let mut d = ServiceDetails {
        image: text(&cfg["Image"]).unwrap_or_default(),
        created: time(&v["Created"]),
        command: join(&cfg["Cmd"]),
        entrypoint: join(&cfg["Entrypoint"]),
        workdir: text(&cfg["WorkingDir"]),
        user: text(&cfg["User"]),
        hostname: text(&cfg["Hostname"]),
        env: env(&cfg["Env"]),
        labels: labels(&cfg["Labels"]),
        healthcheck: healthcheck(&cfg["Healthcheck"]),
        log_driver: text(&host["LogConfig"]["Type"]),
        stop_grace: cfg["StopTimeout"].as_i64().map(|s| duration(s * 1_000_000_000)),
        ..Default::default()
    };
    for m in list(&v["Mounts"]) {
        d.mounts.push(Mount {
            kind: text(&m["Type"]).unwrap_or_else(|| "volume".into()),
            source: text(&m["Name"]).or_else(|| text(&m["Source"])).unwrap_or_default(),
            target: text(&m["Destination"]).unwrap_or_default(),
            read_only: !m["RW"].as_bool().unwrap_or(true),
        });
    }
    if let Some(ports) = v["NetworkSettings"]["Ports"].as_object() {
        for (target, bindings) in ports {
            let published: Vec<String> = list(bindings)
                .iter()
                .filter_map(|b| {
                    let port = text(&b["HostPort"])?;
                    let ip = text(&b["HostIp"]).filter(|ip| ip != "0.0.0.0" && ip != "::");
                    Some(match ip {
                        Some(ip) => format!("{ip}:{port}"),
                        None => port,
                    })
                })
                .collect();
            let mut published = published;
            published.dedup();
            d.ports.push(if published.is_empty() { target.clone() } else { format!("{}→{target}", published.join(", ")) });
        }
    }
    if let Some(nets) = v["NetworkSettings"]["Networks"].as_object() {
        for (name, n) in nets {
            let mut aliases = strings(&n["Aliases"]);
            aliases.retain(|a| a.len() != 12 || !a.chars().all(|c| c.is_ascii_hexdigit()));
            d.networks.push(Network { name: name.clone(), ip: text(&n["IPAddress"]), aliases });
        }
    }
    let rp = &host["RestartPolicy"];
    if let Some(name) = text(&rp["Name"]) {
        let max = rp["MaximumRetryCount"].as_u64().filter(|n| *n > 0);
        d.restart = Some(match max {
            Some(n) => format!("{name}, at most {n} attempts"),
            None => name,
        });
    }
    d.limits = resources(host, "NanoCpus", "Memory");
    d.reservations = resources(host, "", "MemoryReservation");
    add_runtime(&mut d, v);
    d
}

/// Facts only a container has: when it started, restarts, OOM kills and health checks.
fn add_runtime(d: &mut ServiceDetails, v: &Value) {
    let st = &v["State"];
    d.container = text(&v["Name"]).map(|n| n.trim_start_matches('/').to_string());
    d.started = time(&st["StartedAt"]);
    d.restart_count = v["RestartCount"].as_u64();
    d.oom_killed = st["OOMKilled"].as_bool().unwrap_or(false);
    d.health_log = list(&st["Health"]["Log"])
        .iter()
        .rev()
        .take(5)
        .map(|h| HealthRun {
            at: time(&h["End"]).or_else(|| time(&h["Start"])),
            exit_code: h["ExitCode"].as_i64().unwrap_or(0),
            output: text(&h["Output"]).unwrap_or_default().trim().chars().take(2000).collect(),
        })
        .collect();
    if d.healthcheck.is_none() {
        d.healthcheck = healthcheck(&v["Config"]["Healthcheck"]);
    }
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn strings(v: &Value) -> Vec<String> {
    list(v).iter().filter_map(text).collect()
}

fn join(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(a) if !a.is_empty() => Some(a.iter().filter_map(|x| x.as_str()).map(quote).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

fn quote(s: &str) -> String {
    if s.is_empty() || s.contains(char::is_whitespace) || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn time(v: &Value) -> Option<i64> {
    let s = v.as_str()?;
    if s.starts_with("0001-") {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.timestamp_millis())
}

fn labels(v: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> =
        v.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
    out.sort();
    out
}

/// Names that usually hold credentials. Their values are not sent to the page.
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["PASS", "PWD", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "AUTH", "PRIVATE", "CERT", "SALT", "COOKIE", "SESSION", "SIGNATURE", "DSN"]
        .iter()
        .any(|w| n.contains(w))
}

fn env(v: &Value) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = strings(v)
        .into_iter()
        .map(|kv| {
            let (name, value) = kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or((kv, String::new()));
            let value = if secret_name(&name) { None } else { Some(hide_url_password(&value)) };
            EnvVar { name, value }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `postgres://app:hunter2@db/app` becomes `postgres://app:•••@db/app`.
fn hide_url_password(v: &str) -> String {
    regex!(r"(://[^/:@\s]+:)[^@\s/]+@").replace_all(v, "$1•••@").into_owned()
}

fn duration(ns: i64) -> String {
    let s = ns / 1_000_000_000;
    if ns > 0 && s == 0 {
        format!("{} ms", ns / 1_000_000)
    } else if s < 120 {
        format!("{s} s")
    } else if s < 7200 {
        format!("{} min", s / 60)
    } else {
        format!("{} h", s / 3600)
    }
}

fn healthcheck(v: &Value) -> Option<String> {
    let test = strings(&v["Test"]);
    let (kind, rest) = test.split_first()?;
    let cmd = match kind.as_str() {
        "NONE" => return Some("disabled".into()),
        "CMD-SHELL" => rest.join(" "),
        _ => rest.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "),
    };
    let mut parts = vec![cmd];
    if let Some(t) = v["Interval"].as_i64().filter(|t| *t > 0) {
        parts.push(format!("every {}", duration(t)));
    }
    if let Some(t) = v["Timeout"].as_i64().filter(|t| *t > 0) {
        parts.push(format!("timeout {}", duration(t)));
    }
    if let Some(n) = v["Retries"].as_u64().filter(|n| *n > 0) {
        parts.push(format!("{n} retries"));
    }
    if let Some(t) = v["StartPeriod"].as_i64().filter(|t| *t > 0) {
        parts.push(format!("start period {}", duration(t)));
    }
    Some(parts.join(" · "))
}

fn update_config(v: &Value) -> Option<String> {
    if !v.is_object() {
        return None;
    }
    let mut parts = vec![match v["Parallelism"].as_u64() {
        Some(0) => "all at once".to_string(),
        Some(n) => format!("{n} at a time"),
        None => "1 at a time".to_string(),
    }];
    if let Some(t) = v["Delay"].as_i64().filter(|t| *t > 0) {
        parts.push(format!("{} apart", duration(t)));
    }
    if let Some(a) = text(&v["FailureAction"]) {
        parts.push(format!("on failure {a}"));
    }
    if let Some(o) = text(&v["Order"]) {
        parts.push(o);
    }
    Some(parts.join(" · "))
}

fn resources(v: &Value, cpu_key: &str, mem_key: &str) -> Option<String> {
    let mut parts = vec![];
    if !cpu_key.is_empty()
        && let Some(n) = v[cpu_key].as_u64().filter(|n| *n > 0)
    {
        let cpus = n as f64 / 1e9;
        parts.push(format!("{} CPU", if cpus.fract() == 0.0 { format!("{cpus:.0}") } else { format!("{cpus:.2}") }));
    }
    if let Some(m) = v[mem_key].as_u64().filter(|n| *n > 0) {
        parts.push(format!("{} MiB memory", m / (1024 * 1024)));
    }
    if parts.is_empty() { None } else { Some(parts.join(" · ")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secrets_stay_hidden() {
        let e = env(&json!([
            "DB_PASSWORD=hunter2",
            "API_KEY=abc",
            "JAVA_OPTS=-Xmx512m",
            "DATABASE_URL=postgres://app:hunter2@db:5432/app",
            "EMPTY"
        ]));
        let get = |n: &str| e.iter().find(|v| v.name == n).unwrap().value.clone();
        assert_eq!(get("DB_PASSWORD"), None);
        assert_eq!(get("API_KEY"), None);
        assert_eq!(get("JAVA_OPTS").as_deref(), Some("-Xmx512m"));
        assert_eq!(get("DATABASE_URL").as_deref(), Some("postgres://app:•••@db:5432/app"));
        assert_eq!(get("EMPTY").as_deref(), Some(""));
        assert!(!format!("{e:?}").contains("hunter2"));
    }

    #[test]
    fn reads_a_swarm_service_spec() {
        let v = json!({
            "CreatedAt": "2026-10-20T10:00:00.000000000Z",
            "UpdatedAt": "2026-10-27T08:30:00.5Z",
            "Spec": {
                "Labels": { "com.docker.stack.namespace": "shop" },
                "TaskTemplate": {
                    "ContainerSpec": {
                        "Image": "ghcr.io/acme/api:1.4@sha256:abc",
                        "Command": ["java"], "Args": ["-jar", "app.jar"],
                        "Env": ["SPRING_PROFILES_ACTIVE=prod", "JWT_SECRET=x"],
                        "Mounts": [{ "Type": "volume", "Source": "shop_data", "Target": "/data", "ReadOnly": true }],
                        "Healthcheck": { "Test": ["CMD-SHELL", "curl -f localhost:8080/health"], "Interval": 30000000000u64, "Retries": 3 },
                        "Secrets": [{ "SecretName": "shop_db" }]
                    },
                    "Resources": { "Limits": { "NanoCPUs": 1500000000u64, "MemoryBytes": 536870912u64 } },
                    "RestartPolicy": { "Condition": "on-failure", "Delay": 5000000000u64, "MaxAttempts": 3 },
                    "Placement": { "Constraints": ["node.role == manager"] },
                    "Networks": [{ "Target": "n1abcdefghijklmnop", "Aliases": ["api"] }]
                },
                "UpdateConfig": { "Parallelism": 1, "Delay": 10000000000u64, "FailureAction": "rollback", "Order": "start-first" },
                "EndpointSpec": { "Ports": [{ "Protocol": "tcp", "TargetPort": 8080, "PublishedPort": 80, "PublishMode": "ingress" }] }
            },
            "Endpoint": { "VirtualIPs": [{ "NetworkID": "n1abcdefghijklmnop", "Addr": "10.0.1.5/24" }] }
        });
        let d = from_service(&v);
        assert_eq!(d.entrypoint.as_deref(), Some("java"));
        assert_eq!(d.command.as_deref(), Some("-jar app.jar"));
        assert_eq!(d.ports, vec!["80→8080/tcp (ingress)"]);
        assert_eq!(d.restart.as_deref(), Some("on-failure, at most 3 attempts, 5 s apart"));
        assert_eq!(d.limits.as_deref(), Some("1.50 CPU · 512 MiB memory"));
        assert_eq!(d.healthcheck.as_deref(), Some("curl -f localhost:8080/health · every 30 s · 3 retries"));
        assert_eq!(d.update_config.as_deref(), Some("1 at a time · 10 s apart · on failure rollback · start-first"));
        assert_eq!(d.placement, vec!["node.role == manager"]);
        assert_eq!(d.secrets, vec!["shop_db"]);
        assert_eq!(d.networks[0].ip.as_deref(), Some("10.0.1.5/24"));
        assert_eq!(d.mounts[0].target, "/data");
        assert!(d.mounts[0].read_only);
        assert!(d.env.iter().any(|e| e.name == "JWT_SECRET" && e.value.is_none()));
        assert!(d.updated.unwrap() > d.created.unwrap());
    }

    #[test]
    fn reads_a_compose_container() {
        let v = json!({
            "Name": "/chronos-app-1",
            "Created": "2026-10-27T08:00:00Z",
            "RestartCount": 2,
            "Config": { "Image": "chronos-app", "Cmd": null, "Entrypoint": ["/start.sh"], "Env": ["TZ=UTC"], "Labels": {} },
            "HostConfig": { "RestartPolicy": { "Name": "unless-stopped", "MaximumRetryCount": 0 }, "Memory": 1073741824u64, "LogConfig": { "Type": "json-file" } },
            "State": {
                "StartedAt": "2026-10-27T08:00:01Z", "OOMKilled": false,
                "Health": { "Log": [
                    { "Start": "2026-10-27T09:00:00Z", "End": "2026-10-27T09:00:01Z", "ExitCode": 0, "Output": "ok" },
                    { "Start": "2026-10-27T09:00:30Z", "End": "2026-10-27T09:00:31Z", "ExitCode": 1, "Output": "connection refused\n" }
                ] }
            },
            "Mounts": [{ "Type": "bind", "Source": "/opt/chronos/data", "Destination": "/data", "RW": true }],
            "NetworkSettings": {
                "Ports": { "8080/tcp": [{ "HostIp": "0.0.0.0", "HostPort": "8080" }, { "HostIp": "::", "HostPort": "8080" }], "9000/tcp": null },
                "Networks": { "chronos_default": { "IPAddress": "172.18.0.3", "Aliases": ["app", "3f2a9c8e1b7d"] } }
            }
        });
        let d = from_container(&v);
        assert_eq!(d.container.as_deref(), Some("chronos-app-1"));
        assert_eq!(d.restart.as_deref(), Some("unless-stopped"));
        assert_eq!(d.restart_count, Some(2));
        assert_eq!(d.limits.as_deref(), Some("1024 MiB memory"));
        assert!(d.ports.contains(&"8080→8080/tcp".to_string()), "{:?}", d.ports);
        assert!(d.ports.contains(&"9000/tcp".to_string()));
        assert_eq!(d.networks[0].aliases, vec!["app"]);
        assert_eq!(d.health_log.len(), 2);
        assert_eq!((d.health_log[0].exit_code, d.health_log[0].output.as_str()), (1, "connection refused"));
        assert!(!d.mounts[0].read_only);
    }
}
