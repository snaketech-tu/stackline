//! Saved connections and preferences, stored as JSON in the user's config folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::docker::ssh::SshTarget;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
    #[serde(default)]
    pub settings: Settings,
    /// Pinned services per connection id, as `stack/service`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, Vec<String>>,
    /// Which services' logs are collected, per connection id. Missing means all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub collect: BTreeMap<String, Collect>,
}

/// The services of one connection whose logs Stackline reads and stores.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Collect {
    /// Collect only `services`. Otherwise collect everything except `services`.
    pub only: bool,
    /// Full service names, e.g. `shop_api`.
    pub services: Vec<String>,
}

impl Collect {
    pub fn wants(&self, service: &str) -> bool {
        self.services.iter().any(|s| s == service) == self.only
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: ConnectionKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConnectionKind {
    /// This computer. `endpoint` overrides the default socket, e.g. `unix:///run/user/1000/docker.sock`.
    Local {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
    },
    Ssh(SshTarget),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `system`, `light` or `dark`.
    pub theme: String,
    pub retention_days: u32,
    pub max_store_mb: u64,
    /// How often cluster state is refreshed.
    pub refresh_secs: u32,
    /// How often new log lines are fetched.
    pub log_poll_secs: u32,
    /// Read CPU and memory per container.
    pub collect_stats: bool,
    /// Look for a newer release on startup.
    pub check_updates: bool,
    /// Connect to every saved connection when the app starts and collect its
    /// logs, instead of only the ones opened in the UI.
    pub collect_at_start: bool,
    /// Send a notification when a connection logs too many errors.
    pub alert_enabled: bool,
    /// The window the errors are counted in, 5 to 15 minutes.
    pub alert_window_mins: u32,
    /// Notify when the window holds at least this many errors.
    pub alert_threshold: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            // Debugging is about the last hours; a week of every service of a
            // big stack costs a lot of disk and a long first read.
            retention_days: 2,
            max_store_mb: 1024,
            refresh_secs: 10,
            log_poll_secs: 3,
            collect_stats: true,
            check_updates: true,
            collect_at_start: true,
            alert_enabled: false,
            alert_window_mins: 10,
            alert_threshold: 20,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            connections: vec![ConnectionConfig {
                id: "local".into(),
                name: "This computer".into(),
                kind: ConnectionKind::Local { endpoint: None },
            }],
            settings: Settings::default(),
            pins: BTreeMap::new(),
            collect: BTreeMap::new(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<AppConfig> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

/// Where Stackline keeps its files.
pub struct Paths {
    pub config: PathBuf,
    pub store: PathBuf,
}

impl Paths {
    pub fn default_paths() -> Paths {
        let cfg = app_dir(dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")));
        let data = app_dir(dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")));
        Paths { config: cfg.join("config.json"), store: data.join("logs.db") }
    }

    pub fn in_dir(dir: &Path) -> Paths {
        Paths { config: dir.join("config.json"), store: dir.join("logs.db") }
    }
}

/// The app's folder under `base`. Stackline was called swarmdash before: its
/// folder is moved over once, so settings, pins and stored logs are kept.
/// While the move is not possible (another copy still has the store open),
/// the old folder is used and the move is tried again on the next start.
fn app_dir(base: PathBuf) -> PathBuf {
    let dir = base.join("stackline");
    let old = base.join("swarmdash");
    if dir.exists() || !old.is_dir() {
        return dir;
    }
    match std::fs::rename(&old, &dir) {
        Ok(()) => {
            tracing::info!("moved {} to {}", old.display(), dir.display());
            dir
        }
        Err(e) => {
            tracing::warn!("could not move {} to {}: {e}", old.display(), dir.display());
            old
        }
    }
}

/// The environment variable `STACKLINE_<name>`, or `SWARMDASH_<name>` from
/// before the rename. An update started by a swarmdash version hands over its
/// sign-in token under the old name.
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(format!("STACKLINE_{name}")).or_else(|_| std::env::var(format!("SWARMDASH_{name}"))).ok()
}

/// Where exported logs are saved when the browser cannot download them (desktop app).
pub fn downloads_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
}

/// A short id for a new connection, derived from its name.
pub fn new_id(name: &str, taken: &[String]) -> String {
    let mut base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if base.is_empty() {
        base = "connection".into();
    }
    base.truncate(32);
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_connections() {
        let json = r#"{"connections":[{"id":"prod","name":"prod","kind":"ssh","target":"deploy@203.0.113.10"},{"id":"local","name":"This computer","kind":"local"}]}"#;
        let c: AppConfig = serde_json::from_str(json).unwrap();
        match &c.connections[0].kind {
            ConnectionKind::Ssh(t) => {
                assert_eq!(t.target, "deploy@203.0.113.10");
                assert_eq!(t.socket, "/var/run/docker.sock");
            }
            _ => panic!(),
        }
        assert_eq!(c.settings.retention_days, 2);
        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains(r#""kind":"ssh""#));
        assert_eq!(new_id("Prod EU!", &["prod-eu".into()]), "prod-eu-2");
    }

    #[test]
    fn moves_the_swarmdash_folder_once() {
        let base = std::env::temp_dir().join(format!("stackline-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("swarmdash")).unwrap();
        std::fs::write(base.join("swarmdash/config.json"), "{}").unwrap();
        assert_eq!(app_dir(base.clone()), base.join("stackline"));
        assert!(base.join("stackline/config.json").exists() && !base.join("swarmdash").exists());
        // A folder left behind by an old copy later on is not merged in.
        std::fs::create_dir_all(base.join("swarmdash")).unwrap();
        assert_eq!(app_dir(base.clone()), base.join("stackline"));
        assert!(base.join("swarmdash").exists());
        // A new install has neither.
        std::fs::remove_dir_all(&base).unwrap();
        assert_eq!(app_dir(base.clone()), base.join("stackline"));
        assert!(!base.exists());
    }

    #[test]
    fn collect_lists_exceptions_or_choices() {
        let all = Collect::default();
        assert!(all.wants("shop_api"));
        let except = Collect { only: false, services: vec!["shop_debug".into()] };
        assert!(except.wants("shop_api") && !except.wants("shop_debug"));
        let only = Collect { only: true, services: vec!["shop_api".into()] };
        assert!(only.wants("shop_api") && !only.wants("shop_worker"));
    }
}
