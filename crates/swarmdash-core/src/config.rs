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
            retention_days: 7,
            max_store_mb: 5120,
            refresh_secs: 10,
            log_poll_secs: 3,
            collect_stats: true,
            check_updates: true,
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

/// Where swarmdash keeps its files.
pub struct Paths {
    pub config: PathBuf,
    pub store: PathBuf,
}

impl Paths {
    pub fn default_paths() -> Paths {
        let cfg = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("swarmdash");
        let data = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("swarmdash");
        Paths { config: cfg.join("config.json"), store: data.join("logs.db") }
    }

    pub fn in_dir(dir: &Path) -> Paths {
        Paths { config: dir.join("config.json"), store: dir.join("logs.db") }
    }
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
        assert_eq!(c.settings.retention_days, 7);
        let back = serde_json::to_string(&c).unwrap();
        assert!(back.contains(r#""kind":"ssh""#));
        assert_eq!(new_id("Prod EU!", &["prod-eu".into()]), "prod-eu-2");
    }
}
