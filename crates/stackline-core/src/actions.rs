//! The changes Stackline can make, once a connection's read-only mode is
//! switched off: restart, stop and start a service's containers, and restart
//! or scale a swarm service. Nothing is ever created or removed.
//!
//! Each action is first turned into a `Plan`: the Docker API calls it needs and
//! the matching `docker` command, so the interface can show what will happen
//! before it does. Planning is pure and tested; `run` only sends the calls.

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::docker::DockerClient;
use crate::model::{Service, StackKind};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Restart,
    Stop,
    Start,
    Scale,
}

/// What someone asked for, as the API receives it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub action: Kind,
    /// Full service name, e.g. `shop_api`.
    pub service: String,
    /// The stack it belongs to, for when two stacks have a service of the same name.
    #[serde(default)]
    pub stack: Option<String>,
    /// Only work out the plan, so it can be shown before anything happens.
    #[serde(default)]
    pub dry: bool,
    /// Only this replica (its slot), instead of every one of the service.
    #[serde(default)]
    pub replica: Option<String>,
    /// For `scale`: the new number of replicas.
    #[serde(default)]
    pub replicas: Option<u32>,
    /// The fingerprint of the plan that was shown (from a dry run). When the
    /// plan made now differs, nothing runs: see [`Changed`].
    #[serde(default)]
    pub expect: Option<String>,
}

/// The plan differs from the one that was shown and confirmed.
#[derive(Debug)]
pub struct Changed;

impl std::fmt::Display for Changed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The service changed since you confirmed, so nothing was done. Look at it again and retry.")
    }
}

impl std::error::Error for Changed {}

/// One call to the Docker API.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    /// POST a path without a body, such as `/containers/<id>/restart`.
    Post(String),
    /// Change a swarm service: read its spec, change it, write it back.
    UpdateService { id: String, change: SpecChange },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecChange {
    /// Replace every task, like `docker service update --force`.
    Force,
    Replicas(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub calls: Vec<Call>,
    /// The same change as a command someone could type.
    pub command: String,
    /// What was done, said after it worked.
    pub done: String,
}

const MAX_REPLICAS: u32 = 100;

impl Plan {
    /// A short value that changes whenever the calls do, so a confirmed plan
    /// can be checked against the one about to run (FNV-1a of the calls).
    pub fn fingerprint(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in format!("{:?}", self.calls).bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// Works out the calls for a request on a service of a stack of `kind`.
pub fn plan(req: &Request, svc: &Service, kind: StackKind) -> Result<Plan> {
    let name = &svc.full_name;
    if kind == StackKind::Swarm {
        if req.replica.is_some() {
            bail!("Swarm restarts a service's tasks as a whole. Restart {name} instead of one replica.");
        }
        return match req.action {
            Kind::Restart => Ok(Plan {
                calls: vec![Call::UpdateService { id: svc.id.clone(), change: SpecChange::Force }],
                command: format!("docker service update --force {name}"),
                done: format!("Restarting the tasks of {name}. Swarm replaces them one after another."),
            }),
            Kind::Scale => {
                if svc.mode != "replicated" {
                    bail!("{name} runs in {} mode: it has one task per node and cannot be scaled.", svc.mode);
                }
                let n = req.replicas.ok_or_else(|| anyhow!("Say how many replicas {name} should run."))?;
                if n > MAX_REPLICAS {
                    bail!("At most {MAX_REPLICAS} replicas can be set from Stackline.");
                }
                Ok(Plan {
                    calls: vec![Call::UpdateService { id: svc.id.clone(), change: SpecChange::Replicas(n) }],
                    command: format!("docker service scale {name}={n}"),
                    done: format!("Scaling {name} to {n} {}.", if n == 1 { "replica" } else { "replicas" }),
                })
            }
            Kind::Stop | Kind::Start => {
                bail!("Swarm keeps a service's tasks running. Scale {name} to 0 to stop it, and back up to start it.")
            }
        };
    }
    if req.action == Kind::Scale {
        bail!("Compose services are scaled with `docker compose up --scale`, which Stackline does not run.");
    }
    let tasks: Vec<_> = svc
        .tasks
        .iter()
        .filter(|t| req.replica.as_ref().is_none_or(|r| &t.slot == r))
        .filter(|t| match req.action {
            Kind::Stop => stoppable(&t.state),
            Kind::Start => startable(&t.state),
            _ => !matches!(t.state.as_str(), "dead" | "removing"),
        })
        .filter_map(|t| t.container_id.clone().map(|id| (id, t.container_name.clone().unwrap_or_else(|| t.slot.clone()))))
        .collect();
    if tasks.is_empty() {
        bail!(match req.action {
            Kind::Stop => format!("Nothing of {name} is running."),
            Kind::Start => format!("Everything of {name} is running already."),
            _ => format!("{name} has no container to restart."),
        });
    }
    let verb = match req.action {
        Kind::Restart => "restart",
        Kind::Stop => "stop",
        _ => "start",
    };
    let names: Vec<_> = tasks.iter().map(|(_, n)| n.as_str()).collect();
    let n = tasks.len();
    Ok(Plan {
        calls: tasks.iter().map(|(id, _)| Call::Post(format!("/containers/{id}/{verb}"))).collect(),
        command: format!("docker {verb} {}", names.join(" ")),
        done: format!(
            "{} {n} {} of {name}.",
            match req.action {
                Kind::Restart => "Restarted",
                Kind::Stop => "Stopped",
                _ => "Started",
            },
            if n == 1 { "container" } else { "containers" }
        ),
    })
}

/// Container states `docker stop` acts on.
fn stoppable(state: &str) -> bool {
    matches!(state, "running" | "restarting" | "paused")
}

/// Container states `docker start` acts on. A dead or removing container cannot be started.
fn startable(state: &str) -> bool {
    matches!(state, "exited" | "created")
}

/// Changes a service spec as `change` says. The spec is what
/// `GET /services/<id>` returns under `Spec`.
pub fn change_spec(spec: &mut Value, change: SpecChange) -> Result<()> {
    match change {
        SpecChange::Force => {
            let tt =
                spec.get_mut("TaskTemplate").and_then(Value::as_object_mut).ok_or_else(|| anyhow!("the service has no task template"))?;
            let n = tt.get("ForceUpdate").and_then(Value::as_u64).unwrap_or(0);
            tt.insert("ForceUpdate".into(), Value::from(n + 1));
        }
        SpecChange::Replicas(n) => {
            let r = spec
                .pointer_mut("/Mode/Replicated")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| anyhow!("the service is not replicated, so it cannot be scaled"))?;
            r.insert("Replicas".into(), Value::from(n));
        }
    }
    Ok(())
}

/// Sends the calls of a plan, in order. When one fails after others went
/// through, the error says how many did, so nobody assumes nothing changed.
pub async fn run(client: &DockerClient, plan: &Plan) -> Result<()> {
    let n = plan.calls.len();
    for (i, call) in plan.calls.iter().enumerate() {
        let res = match call {
            Call::Post(path) => client.post(path, None).await,
            Call::UpdateService { id, change } => update_service(client, id, *change).await,
        };
        if let Err(e) = res {
            return Err(if i == 0 { e } else { e.context(format!("{i} of {n} went through before this one failed")) });
        }
    }
    Ok(())
}

async fn update_service(client: &DockerClient, id: &str, change: SpecChange) -> Result<()> {
    let svc: Value = client.get_json(&format!("/services/{id}")).await?;
    let version = svc.pointer("/Version/Index").and_then(Value::as_u64).ok_or_else(|| anyhow!("Docker sent no service version"))?;
    let mut spec = svc.get("Spec").cloned().ok_or_else(|| anyhow!("Docker sent no service spec"))?;
    change_spec(&mut spec, change)?;
    client.post(&format!("/services/{id}/update?version={version}"), Some(&spec)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Task;
    use serde_json::json;

    fn task(slot: &str, state: &str, id: Option<&str>) -> Task {
        Task {
            id: format!("t{slot}"),
            slot: slot.into(),
            node: "vm".into(),
            state: state.into(),
            desired_state: "running".into(),
            message: None,
            error: None,
            health: None,
            container_id: id.map(str::to_string),
            container_name: id.map(|i| format!("shop-api-{i}")),
            since: None,
            exit_code: None,
            failures: 0,
            cpu_percent: None,
            mem_used: None,
        }
    }

    fn svc(mode: &str, tasks: Vec<Task>) -> Service {
        Service {
            id: "svc1".into(),
            name: "api".into(),
            full_name: "shop_api".into(),
            stack: "shop".into(),
            image: "alpine".into(),
            mode: mode.into(),
            desired: 2,
            running: 1,
            ports: vec![],
            updated_at: None,
            tty: false,
            tasks,
            cpu_percent: None,
            mem_used: None,
            config: Default::default(),
        }
    }

    fn req(action: Kind) -> Request {
        Request { action, service: "shop_api".into(), stack: None, dry: false, replica: None, replicas: None, expect: None }
    }

    #[test]
    fn compose_restart_hits_every_container_and_says_the_command() {
        let s = svc("compose", vec![task("1", "running", Some("a1")), task("2", "exited", Some("b2"))]);
        let p = plan(&req(Kind::Restart), &s, StackKind::Compose).unwrap();
        assert_eq!(p.calls, vec![Call::Post("/containers/a1/restart".into()), Call::Post("/containers/b2/restart".into())]);
        assert_eq!(p.command, "docker restart shop-api-a1 shop-api-b2");
    }

    #[test]
    fn stop_and_start_only_touch_containers_that_need_it() {
        let s = svc("compose", vec![task("1", "running", Some("a1")), task("2", "exited", Some("b2"))]);
        assert_eq!(plan(&req(Kind::Stop), &s, StackKind::Compose).unwrap().calls, vec![Call::Post("/containers/a1/stop".into())]);
        assert_eq!(plan(&req(Kind::Start), &s, StackKind::Compose).unwrap().calls, vec![Call::Post("/containers/b2/start".into())]);
        let one = Request { replica: Some("2".into()), ..req(Kind::Restart) };
        assert_eq!(plan(&one, &s, StackKind::Compose).unwrap().calls, vec![Call::Post("/containers/b2/restart".into())]);
        // A paused container is stopped, a dead one is not started.
        let odd = svc("compose", vec![task("1", "paused", Some("p1")), task("2", "dead", Some("d2"))]);
        assert_eq!(plan(&req(Kind::Stop), &odd, StackKind::Compose).unwrap().calls, vec![Call::Post("/containers/p1/stop".into())]);
        assert!(plan(&req(Kind::Start), &odd, StackKind::Compose).is_err());
        let all_up = svc("compose", vec![task("1", "running", Some("a1"))]);
        assert!(plan(&req(Kind::Start), &all_up, StackKind::Compose).unwrap_err().to_string().contains("running already"));
    }

    #[test]
    fn swarm_restarts_and_scales_through_the_service() {
        let s = svc("replicated", vec![task("1", "running", Some("a1"))]);
        let p = plan(&req(Kind::Restart), &s, StackKind::Swarm).unwrap();
        assert_eq!(p.calls, vec![Call::UpdateService { id: "svc1".into(), change: SpecChange::Force }]);
        assert_eq!(p.command, "docker service update --force shop_api");
        let p = plan(&Request { replicas: Some(3), ..req(Kind::Scale) }, &s, StackKind::Swarm).unwrap();
        assert_eq!(p.command, "docker service scale shop_api=3");
        assert!(plan(&Request { replicas: Some(500), ..req(Kind::Scale) }, &s, StackKind::Swarm).is_err());
        assert!(plan(&req(Kind::Stop), &s, StackKind::Swarm).is_err());
        assert!(plan(&Request { replicas: Some(2), ..req(Kind::Scale) }, &svc("global", vec![]), StackKind::Swarm).is_err());
        assert!(plan(&Request { replicas: Some(2), ..req(Kind::Scale) }, &s, StackKind::Compose).is_err());
    }

    #[test]
    fn a_plan_made_after_a_container_was_replaced_has_another_fingerprint() {
        let before = svc("compose", vec![task("1", "running", Some("a1"))]);
        let after = svc("compose", vec![task("1", "running", Some("a2"))]);
        let shown = plan(&req(Kind::Restart), &before, StackKind::Compose).unwrap().fingerprint();
        assert_eq!(plan(&req(Kind::Restart), &before, StackKind::Compose).unwrap().fingerprint(), shown);
        assert_ne!(plan(&req(Kind::Restart), &after, StackKind::Compose).unwrap().fingerprint(), shown);
        assert_ne!(plan(&req(Kind::Stop), &before, StackKind::Compose).unwrap().fingerprint(), shown);
    }

    #[test]
    fn spec_changes_keep_the_rest_of_the_spec() {
        let mut spec = json!({ "Name": "shop_api", "TaskTemplate": { "ContainerSpec": { "Image": "alpine" }, "ForceUpdate": 4 }, "Mode": { "Replicated": { "Replicas": 2 } } });
        change_spec(&mut spec, SpecChange::Force).unwrap();
        assert_eq!(spec["TaskTemplate"]["ForceUpdate"], 5);
        assert_eq!(spec["TaskTemplate"]["ContainerSpec"]["Image"], "alpine");
        change_spec(&mut spec, SpecChange::Replicas(0)).unwrap();
        assert_eq!(spec["Mode"]["Replicated"]["Replicas"], 0);
        let mut global = json!({ "TaskTemplate": {}, "Mode": { "Global": {} } });
        assert!(change_spec(&mut global, SpecChange::Replicas(1)).is_err());
    }
}
