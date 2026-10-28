//! Keeps every opened connection alive: one task refreshes the cluster state,
//! another collects logs. The HTTP API only reads what these tasks produce.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use serde::Serialize;
use tokio::task::JoinHandle;

use crate::alerts::Alerts;
use crate::collector::Collector;
use crate::config::{AppConfig, ConnectionConfig, ConnectionKind, Paths, Settings};
use crate::docker::ssh::SshTunnel;
use crate::docker::{DockerClient, Endpoint};
use crate::model::Snapshot;
use crate::snapshot::{self, Options};
use crate::store::Store;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Connecting,
    Connected,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnState {
    pub status: Status,
    pub error: Option<String>,
    pub latency_ms: Option<u64>,
    pub endpoint: Option<String>,
    pub updated_at: Option<i64>,
    pub logs_synced_at: Option<i64>,
    pub log_errors: Vec<String>,
}

/// One opened connection.
pub struct Runtime {
    pub cfg: ConnectionConfig,
    snapshot: RwLock<Option<Arc<Snapshot>>>,
    state: RwLock<ConnState>,
    client: tokio::sync::Mutex<Option<(DockerClient, Option<SshTunnel>)>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// CPU counters per container from the last refresh.
    cpu: Mutex<snapshot::CpuSamples>,
}

impl Runtime {
    fn new(cfg: ConnectionConfig) -> Self {
        Self {
            cfg,
            snapshot: RwLock::new(None),
            state: RwLock::new(ConnState {
                status: Status::Connecting,
                error: None,
                latency_ms: None,
                endpoint: None,
                updated_at: None,
                logs_synced_at: None,
                log_errors: vec![],
            }),
            client: tokio::sync::Mutex::new(None),
            tasks: Mutex::new(vec![]),
            cpu: Mutex::default(),
        }
    }

    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.snapshot.read().unwrap().clone()
    }

    pub fn state(&self) -> ConnState {
        self.state.read().unwrap().clone()
    }

    /// Reads the full configuration of a service by its full name.
    pub async fn details(&self, name: &str) -> Result<Option<crate::details::ServiceDetails>> {
        let Some(snap) = self.snapshot() else { return Ok(None) };
        let Some((svc, kind)) = snap.stacks.iter().find_map(|st| st.services.iter().find(|s| s.full_name == name).map(|s| (s, st.kind)))
        else {
            return Ok(None);
        };
        let client = self.client().await?;
        Ok(Some(crate::details::fetch(&client, svc, kind).await?))
    }

    /// Returns a working client, (re)opening the SSH tunnel when needed.
    async fn client(&self) -> Result<DockerClient> {
        let mut guard = self.client.lock().await;
        if let Some((client, tunnel)) = guard.as_mut()
            && tunnel.as_mut().is_none_or(|t| t.is_alive())
        {
            return Ok(client.clone());
        }
        *guard = None;
        let (client, tunnel) = open(&self.cfg).await?;
        self.state.write().unwrap().endpoint = Some(describe(&self.cfg, client.endpoint()));
        *guard = Some((client.clone(), tunnel));
        Ok(client)
    }

    async fn drop_client(&self) -> Option<String> {
        let mut guard = self.client.lock().await;
        let msg = guard.as_ref().and_then(|(_, t)| t.as_ref().and_then(|t| t.last_message()));
        *guard = None;
        msg
    }

    async fn refresh(&self, settings: &Settings) {
        let result = async {
            let client = self.client().await?;
            let started = Instant::now();
            client.ping().await?;
            let ms = started.elapsed().as_millis() as u64;
            Ok::<_, anyhow::Error>((snapshot::fetch(&client, &Options { stats: settings.collect_stats, cpu: &self.cpu }).await?, ms))
        }
        .await;
        match result {
            Ok((snap, ms)) => {
                *self.snapshot.write().unwrap() = Some(Arc::new(snap));
                let mut st = self.state.write().unwrap();
                st.status = Status::Connected;
                st.error = None;
                st.latency_ms = Some(ms);
                st.updated_at = Some(chrono::Utc::now().timestamp_millis());
            }
            Err(e) => {
                let hint = self.drop_client().await;
                let mut msg = format!("{e:#}");
                if let Some(h) = hint.filter(|h| !msg.contains(h.as_str())) {
                    msg = format!("{msg} (ssh: {h})");
                }
                let mut st = self.state.write().unwrap();
                st.status = Status::Error;
                st.error = Some(explain(&msg));
            }
        }
    }
}

/// Opens a client for a connection without starting any background work.
pub async fn open(cfg: &ConnectionConfig) -> Result<(DockerClient, Option<SshTunnel>)> {
    match &cfg.kind {
        ConnectionKind::Local { endpoint } => {
            let ep = match endpoint.as_deref().filter(|s| !s.is_empty()) {
                Some(s) => Endpoint::parse(s).ok_or_else(|| anyhow!("unsupported Docker endpoint {s}"))?,
                None => Endpoint::local_default(),
            };
            Ok((DockerClient::new(ep), None))
        }
        ConnectionKind::Ssh(t) => {
            let tunnel = SshTunnel::open(t).await?;
            let client = DockerClient::new(Endpoint::Tcp(tunnel.local_addr().to_string()));
            Ok((client, Some(tunnel)))
        }
    }
}

fn describe(cfg: &ConnectionConfig, ep: &Endpoint) -> String {
    match &cfg.kind {
        ConnectionKind::Ssh(t) => format!("ssh {}{}", t.target, t.port.map(|p| format!(":{p}")).unwrap_or_default()),
        ConnectionKind::Local { .. } => ep.describe(),
    }
}

/// Adds a plain-language fix to common errors.
fn explain(msg: &str) -> String {
    let lower = msg.to_lowercase();
    let hint = if lower.contains("permission denied") && lower.contains("sock") {
        Some(
            "The user cannot read the Docker socket. Add it to the `docker` group on the host (sudo usermod -aG docker <user>), then reconnect.",
        )
    } else if lower.contains("permission denied (publickey") {
        Some("SSH rejected the key. Check that `ssh <target>` works in a terminal without a password prompt.")
    } else if lower.contains("could not resolve hostname") {
        Some("The host name could not be resolved. Check the address or your ~/.ssh/config alias.")
    } else if lower.contains("host key verification failed") {
        Some("The server's host key is not trusted yet. Connect once with `ssh <target>` in a terminal to accept it.")
    } else if lower.contains("no such file or directory") && lower.contains("docker") {
        Some("No Docker socket was found. Is Docker running?")
    } else {
        None
    };
    match hint {
        Some(h) => format!("{msg}\n{h}"),
        None => msg.to_string(),
    }
}

/// Owns configuration, the log store and all opened connections.
pub struct Manager {
    paths: Paths,
    config: RwLock<AppConfig>,
    pub store: Arc<Store>,
    pub alerts: Alerts,
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
    /// Services whose stored lines are being deleted, as (connection, service).
    deleting: Arc<Mutex<std::collections::HashSet<(String, String)>>>,
}

impl Manager {
    pub fn new(paths: Paths) -> Result<Arc<Manager>> {
        let config = AppConfig::load(&paths.config)?;
        let store = Arc::new(Store::open(&paths.store)?);
        let m = Arc::new(Manager {
            paths,
            config: RwLock::new(config),
            store,
            alerts: Alerts::default(),
            runtimes: Mutex::new(HashMap::new()),
            deleting: Arc::default(),
        });
        let store = m.store.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = store.upgrade() {
                tracing::warn!("could not upgrade the log store: {e:#}");
            }
        });
        m.clone().spawn_retention();
        m.clone().spawn_alerts();
        Ok(m)
    }

    pub fn config(&self) -> AppConfig {
        self.config.read().unwrap().clone()
    }

    pub fn store_path(&self) -> &std::path::Path {
        &self.paths.store
    }

    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig> {
        let mut c = self.config.write().unwrap();
        f(&mut c);
        c.save(&self.paths.config)?;
        Ok(c.clone())
    }

    /// Returns the running runtime for a connection, starting it on first use.
    pub fn runtime(self: &Arc<Self>, id: &str) -> Option<Arc<Runtime>> {
        let mut map = self.runtimes.lock().unwrap();
        let cfg = self.config.read().unwrap().connections.iter().find(|c| c.id == id).cloned()?;
        if let Some(rt) = map.get(id) {
            if rt.cfg == cfg {
                return Some(rt.clone());
            }
            for t in rt.tasks.lock().unwrap().drain(..) {
                t.abort();
            }
        }
        let rt = Arc::new(Runtime::new(cfg));
        self.start(rt.clone());
        map.insert(id.to_string(), rt.clone());
        Some(rt)
    }

    pub fn stop(&self, id: &str) {
        if let Some(rt) = self.runtimes.lock().unwrap().remove(id) {
            for t in rt.tasks.lock().unwrap().drain(..) {
                t.abort();
            }
        }
    }

    /// Stops every connection and its ssh process. Call before exiting.
    pub fn shutdown(&self) {
        for (_, rt) in self.runtimes.lock().unwrap().drain() {
            for t in rt.tasks.lock().unwrap().drain(..) {
                t.abort();
            }
            if let Ok(mut guard) = rt.client.try_lock()
                && let Some((_, Some(tunnel))) = guard.as_mut()
            {
                tunnel.kill();
            }
        }
    }

    fn start(self: &Arc<Self>, rt: Arc<Runtime>) {
        let m = self.clone();
        let r = rt.clone();
        let state_task = tokio::spawn(async move {
            loop {
                let settings = m.config().settings;
                r.refresh(&settings).await;
                let wait = if r.state().status == Status::Error { 5 } else { settings.refresh_secs.max(2) };
                tokio::time::sleep(Duration::from_secs(wait as u64)).await;
            }
        });
        let m = self.clone();
        let r = rt.clone();
        let log_task = tokio::spawn(async move {
            let mut collector = Collector::new(&r.cfg.id, m.store.clone());
            let mut bound: Option<String> = None;
            loop {
                let settings = m.config().settings;
                let snap = r.snapshot();
                if let (Some(snap), Ok(client)) = (snap, r.client().await) {
                    // Lines stored under this connection must come from the host it reaches now.
                    // Checked when the identity is first seen or changes, not on every poll.
                    if let Some(identity) = snap.identity().filter(|i| bound.as_ref() != Some(i)) {
                        let (store, conn, id2) = (m.store.clone(), r.cfg.id.clone(), identity.clone());
                        // Clearing a large store takes a while; keep it off the async workers.
                        match tokio::task::spawn_blocking(move || store.bind_host(&conn, &id2)).await {
                            Ok(Ok(cleared)) => {
                                if cleared {
                                    tracing::info!("{} now reaches another host; its stored logs were cleared", r.cfg.name);
                                    collector = Collector::new(&r.cfg.id, m.store.clone());
                                }
                                bound = Some(identity);
                            }
                            Ok(Err(e)) => tracing::warn!("could not check the host of {}: {e:#}", r.cfg.name),
                            Err(_) => {}
                        }
                    }
                    let retention = settings.retention_days.max(1) as i64 * 86_400_000_000;
                    let collect = m.config().collect.remove(&r.cfg.id).unwrap_or_default();
                    let sources = snap.log_sources().into_iter().filter(|s| collect.wants(&s.service)).collect();
                    let report = collector.poll(&client, sources, retention).await;
                    let mut st = r.state.write().unwrap();
                    st.logs_synced_at = Some(chrono::Utc::now().timestamp_millis());
                    st.log_errors = report.errors;
                }
                tokio::time::sleep(Duration::from_secs(settings.log_poll_secs.max(1) as u64)).await;
            }
        });
        rt.tasks.lock().unwrap().extend([state_task, log_task]);
    }

    /// Checks the error rate of every opened connection every 15 seconds.
    fn spawn_alerts(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                let cfg = self.config();
                let s = cfg.settings;
                let open: Vec<String> = if s.alert_enabled { self.runtimes.lock().unwrap().keys().cloned().collect() } else { vec![] };
                self.alerts.retain(&open);
                for id in open {
                    let name = cfg.connections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_else(|| id.clone());
                    let now = chrono::Utc::now().timestamp_micros();
                    let from = now - s.alert_window_mins.clamp(1, 60) as i64 * 60_000_000;
                    let store = self.store.clone();
                    let conn = id.clone();
                    match tokio::task::spawn_blocking(move || store.error_rate(&conn, from, now)).await {
                        Ok(Ok(rate)) => self.alerts.update(&id, &name, rate, &s, now / 1000),
                        Ok(Err(e)) => tracing::warn!("could not count the errors of {name}: {e:#}"),
                        Err(_) => {}
                    }
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        });
    }

    /// Stops or resumes collecting services of a connection. Services that are
    /// collected again start from now, instead of reading everything they
    /// logged in between.
    pub fn set_collect(&self, id: &str, next: crate::config::Collect) -> Result<()> {
        let mut before = None;
        self.update_config(|c| {
            before = Some(c.collect.get(id).cloned().unwrap_or_default());
            if next == crate::config::Collect::default() {
                c.collect.remove(id);
            } else {
                c.collect.insert(id.to_string(), next.clone());
            }
        })?;
        let before = before.unwrap_or_default();
        let rt = self.runtimes.lock().unwrap().get(id).cloned();
        if let Some(snap) = rt.and_then(|r| r.snapshot()) {
            let now = chrono::Utc::now().timestamp_micros();
            for src in snap.log_sources() {
                if !before.wants(&src.service) && next.wants(&src.service) {
                    self.store.set_cursor(id, &src.key, now)?;
                }
            }
        }
        Ok(())
    }

    /// Deletes the stored lines of a service in the background: on a large
    /// store that takes minutes, because every line also leaves the search
    /// index. Returns false when it is already running.
    pub fn clear_service(&self, conn: &str, service: &str) -> bool {
        let key = (conn.to_string(), service.to_string());
        if !self.deleting.lock().unwrap().insert(key.clone()) {
            return false;
        }
        let (store, deleting) = (self.store.clone(), self.deleting.clone());
        tokio::task::spawn_blocking(move || {
            match store.clear_service(&key.0, &key.1) {
                Ok(n) => tracing::info!("deleted {n} stored lines of {}", key.1),
                Err(e) => tracing::warn!("could not delete the stored lines of {}: {e:#}", key.1),
            }
            deleting.lock().unwrap().remove(&key);
        });
        true
    }

    /// The services of a connection whose stored lines are being deleted.
    pub fn deleting(&self, conn: &str) -> Vec<String> {
        self.deleting.lock().unwrap().iter().filter(|(c, _)| c == conn).map(|(_, s)| s.clone()).collect()
    }

    fn spawn_retention(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                let s = self.config().settings;
                let before = chrono::Utc::now().timestamp_micros() - s.retention_days.max(1) as i64 * 86_400_000_000;
                let store = self.store.clone();
                let _ = tokio::task::spawn_blocking(move || store.retain(before, s.max_store_mb * 1024 * 1024)).await;
                // Every minute: a host with chatty services can write a lot in an hour.
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStep {
    pub label: String,
    pub ok: bool,
    pub detail: Option<String>,
    pub ms: u64,
}

/// Checks a connection step by step without saving or starting it.
pub async fn test_connection(cfg: &ConnectionConfig) -> Vec<TestStep> {
    let mut steps = Vec::new();
    let t = Instant::now();
    let label = match &cfg.kind {
        ConnectionKind::Ssh(s) => format!("SSH to {} and forward {}", s.target, s.socket),
        ConnectionKind::Local { .. } => "Open the local Docker socket".to_string(),
    };
    let opened = open(cfg).await;
    let (client, tunnel) = match opened {
        Ok(v) => {
            steps.push(TestStep { label, ok: true, detail: None, ms: t.elapsed().as_millis() as u64 });
            v
        }
        Err(e) => {
            steps.push(TestStep { label, ok: false, detail: Some(explain(&format!("{e:#}"))), ms: t.elapsed().as_millis() as u64 });
            return steps;
        }
    };
    let t = Instant::now();
    match client.get_json::<serde_json::Value>("/version").await {
        Ok(v) => steps.push(TestStep {
            label: format!("Docker {} (API {})", v["Version"].as_str().unwrap_or("?"), v["ApiVersion"].as_str().unwrap_or("?")),
            ok: true,
            detail: None,
            ms: t.elapsed().as_millis() as u64,
        }),
        Err(e) => {
            let hint = tunnel.as_ref().and_then(|t| t.last_message());
            let msg = match hint {
                Some(h) => format!("{e:#} (ssh: {h})"),
                None => format!("{e:#}"),
            };
            steps.push(TestStep {
                label: "Reach the Docker API".into(),
                ok: false,
                detail: Some(explain(&msg)),
                ms: t.elapsed().as_millis() as u64,
            });
            return steps;
        }
    }
    let t = Instant::now();
    match client.get_json::<serde_json::Value>("/info").await {
        Ok(v) => {
            let swarm = v["Swarm"]["LocalNodeState"].as_str().unwrap_or("inactive");
            let manager = v["Swarm"]["ControlAvailable"].as_bool().unwrap_or(false);
            let label = match (swarm, manager) {
                ("active", true) => "Swarm mode, manager node".to_string(),
                ("active", false) => "Swarm mode, worker node (connect to a manager for services)".to_string(),
                _ => format!("Plain Docker, {} containers", v["Containers"].as_u64().unwrap_or(0)),
            };
            steps.push(TestStep { label, ok: true, detail: None, ms: t.elapsed().as_millis() as u64 });
        }
        Err(e) => steps.push(TestStep {
            label: "Read host info".into(),
            ok: false,
            detail: Some(format!("{e:#}")),
            ms: t.elapsed().as_millis() as u64,
        }),
    }
    steps
}
