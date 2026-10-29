//! When a service was deployed and what changed.
//!
//! Every refresh compares what a service runs (image, command, environment,
//! replicas, ports, mounts, limits) with what it ran before. A change, with
//! the time Docker gives for it (a swarm service's update, the newest
//! container of a Compose service), becomes a `deploy` event next to the
//! crashes, so the time graphs mark it and errors that only started after it
//! stand out. Environment values are compared by a hash and never stored.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::events::{Event, Kind};
use crate::model::Snapshot;
use crate::store::Store;

/// What a service runs, as `key → value`. Keys starting with `env:` hold a
/// hash of the value.
pub type Config = BTreeMap<String, String>;

/// A short hash of an environment value, so a change shows without the value.
pub fn hash(v: &str) -> String {
    format!("{:08x}", xxhash_rust::xxh3::xxh3_64(v.as_bytes()) as u32)
}

/// Compares the services of a snapshot with what was stored before and
/// stores a deploy event for each one that changed. A service seen for the
/// first time gets one at its last update when that is within `from`.
/// Returns how many deploys were stored.
pub fn record(store: &Store, conn: &str, snap: &Snapshot, from: i64) -> Result<usize> {
    let known = store.deploy_state(conn)?;
    let local = snap.nodes.iter().find(|n| n.is_local).map(|n| n.hostname.clone()).unwrap_or_else(|| snap.host.name.clone());
    let mut events = Vec::new();
    for st in &snap.stacks {
        for svc in &st.services {
            let Some(at) = svc.updated_at.map(|ms| ms * 1000) else { continue };
            let detail = match known.get(&svc.full_name) {
                // Nothing new, or an update that only came back to an older time.
                Some((ts, _)) if at <= *ts => continue,
                Some((_, before)) => {
                    let before: Config = serde_json::from_str(before).unwrap_or_default();
                    describe(&changes(&before, &svc.config), svc.config.get("image"))
                }
                None if at >= from => svc.config.get("image").map(|i| format!("image {i}")).unwrap_or_default(),
                None => {
                    store.set_deploy_state(conn, &svc.full_name, at, &serde_json::to_string(&svc.config)?)?;
                    continue;
                }
            };
            store.set_deploy_state(conn, &svc.full_name, at, &serde_json::to_string(&svc.config)?)?;
            events.push(Event {
                key: format!("deploy:{}:{at}", svc.full_name),
                ts: at,
                stack: st.name.clone(),
                service: svc.full_name.clone(),
                replica: String::new(),
                node: local.clone(),
                source: String::new(),
                kind: Kind::Deploy,
                exit_code: None,
                detail: Some(detail).filter(|d| !d.is_empty()),
            });
        }
    }
    store.insert_events(conn, &events)
}

/// One line per kind of change, in a stable order.
pub fn changes(before: &Config, after: &Config) -> Vec<String> {
    let mut out = Vec::new();
    let (mut env_added, mut env_removed, mut env_changed) = (vec![], vec![], vec![]);
    for (k, v) in after {
        match (k.strip_prefix("env:"), before.get(k)) {
            (Some(name), None) => env_added.push(name),
            (Some(name), Some(b)) if b != v => env_changed.push(name),
            (Some(_), _) => {}
            (None, Some(b)) if b != v => out.push(match k.as_str() {
                "image" => format!("image {b} → {v}"),
                "image id" => "the image was rebuilt or pulled again".to_string(),
                "replicas" => format!("replicas {b} → {v}"),
                "forced" => "forced restart".to_string(),
                "compose config" => "compose file changed".to_string(),
                _ => format!("{k} changed"),
            }),
            (None, None) if !before.is_empty() => out.push(format!("{k} added")),
            _ => {}
        }
    }
    for k in before.keys() {
        match k.strip_prefix("env:") {
            Some(name) if !after.contains_key(k) => env_removed.push(name),
            None if !after.contains_key(k) => out.push(format!("{k} removed")),
            _ => {}
        }
    }
    let mut env = Vec::new();
    let list = |names: &[&str], what: &str| (!names.is_empty()).then(|| format!("{} {what}", names.join(", ")));
    env.extend(list(&env_changed, "changed"));
    env.extend(list(&env_added, "added"));
    env.extend(list(&env_removed, "removed"));
    if !env.is_empty() {
        out.push(format!("env {}", env.join(", ")));
    }
    out
}

fn describe(changes: &[String], image: Option<&String>) -> String {
    if changes.is_empty() {
        return match image {
            Some(i) => format!("same configuration, image {i}"),
            None => "same configuration".into(),
        };
    }
    changes.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn names_what_changed_without_values() {
        let before = cfg(&[("image", "shop/api:1.2"), ("replicas", "2"), ("env:API_URL", "aaaa"), ("env:OLD", "bbbb"), ("ports", "80")]);
        let after = cfg(&[("image", "shop/api:1.3"), ("replicas", "3"), ("env:API_URL", "cccc"), ("env:DEBUG", "dddd"), ("ports", "80")]);
        assert_eq!(
            changes(&before, &after),
            ["image shop/api:1.2 → shop/api:1.3", "replicas 2 → 3", "env API_URL changed, DEBUG added, OLD removed"]
        );
        assert!(changes(&after, &after).is_empty());
        assert_eq!(describe(&[], Some(&"shop/api:1.3".to_string())), "same configuration, image shop/api:1.3");
    }

    #[test]
    fn a_new_update_time_is_a_deploy_once() {
        use crate::model::*;
        let store = Store::open_in_memory().unwrap();
        let svc = |updated: i64, image: &str| Service {
            id: "s".into(),
            name: "api".into(),
            full_name: "shop_api".into(),
            stack: "shop".into(),
            image: image.into(),
            mode: "replicated".into(),
            desired: 1,
            running: 1,
            ports: vec![],
            updated_at: Some(updated),
            tty: false,
            tasks: vec![],
            cpu_percent: None,
            mem_used: None,
            config: cfg(&[("image", image), ("env:TOKEN", &hash("secret"))]),
        };
        let snap = |s: Service| Snapshot {
            mode: Mode::Swarm,
            host: Host {
                id: "h".into(),
                cluster_id: None,
                name: "box".into(),
                os: String::new(),
                kernel: String::new(),
                engine: "Docker".into(),
                docker_version: String::new(),
                api_version: String::new(),
                ncpu: 1,
                mem_total: 0,
                containers_running: 0,
                containers_total: 0,
                images: 0,
                swarm_state: "active".into(),
                is_manager: true,
                cpu_percent: None,
                mem_used: None,
            },
            nodes: vec![],
            stacks: vec![Stack { name: "shop".into(), kind: StackKind::Swarm, services: vec![s] }],
            fetched_at: 0,
            warnings: vec![],
            task_failures: vec![],
        };
        // Updated long before the retention: remembered, no marker.
        assert_eq!(record(&store, "c1", &snap(svc(1_000, "shop/api:1.1")), 5_000_000).unwrap(), 0);
        // A new update: a marker with what changed.
        assert_eq!(record(&store, "c1", &snap(svc(10_000, "shop/api:1.2")), 5_000_000).unwrap(), 1);
        // The same again: nothing.
        assert_eq!(record(&store, "c1", &snap(svc(10_000, "shop/api:1.2")), 5_000_000).unwrap(), 0);
        let all = crate::store::LogFilter { from: 0, to: i64::MAX, ..Default::default() };
        let ev = store.events("c1", &all, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].kind, ev[0].ts), (Kind::Deploy, 10_000_000));
        assert_eq!(ev[0].detail.as_deref(), Some("image shop/api:1.1 → shop/api:1.2"));
        assert!(!format!("{ev:?}").contains("secret"));
        // A service first seen within the retention is marked at its update.
        assert_eq!(record(&store, "c2", &snap(svc(10_000, "shop/api:1.2")), 5_000_000).unwrap(), 1);
        assert_eq!(store.events("c2", &all, 10).unwrap()[0].detail.as_deref(), Some("image shop/api:1.2"));
    }
}
