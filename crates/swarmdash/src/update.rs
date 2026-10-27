//! Update check and install.
//!
//! On startup swarmdash asks GitHub for newer releases and the UI offers them
//! with their changelog. Nothing is downloaded until the user clicks update.
//! The desktop app installs through Tauri's updater; `swarmdash serve`
//! downloads the binary for its platform and replaces itself. Both check the
//! download against the release signing key before installing, then restart.

use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use swarmdash_core::runtime::Manager;
use tokio::sync::Notify;

pub const REPO: &str = "snaketech-tu/swarmdash";
const PUBKEY: &str = include_str!("../updater.pub");
const TARGET: &str = env!("SWARMDASH_TARGET");
const MANUAL: &str = "Download the new version from GitHub and install it over this one.";
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);

pub type Progress = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// Installs an update for the desktop app. Implemented by the Tauri shell.
pub trait DesktopInstaller: Send + Sync {
    /// Downloads, verifies and installs the newest release, reporting bytes
    /// received, then restarts the app. Only returns on failure.
    fn install(&self, progress: Progress) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>;
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Restarting,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub name: String,
    /// Markdown from the GitHub release.
    pub notes: String,
    pub published_at: Option<String>,
    pub url: String,
    #[serde(skip)]
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub current: String,
    pub phase: Phase,
    /// Every release newer than this one, newest first.
    pub releases: Vec<Release>,
    pub checked_at: Option<i64>,
    pub error: Option<String>,
    pub received: u64,
    pub total: Option<u64>,
    /// Whether this copy can install updates itself, and if not, why.
    pub can_install: bool,
    pub install_hint: Option<String>,
    pub releases_url: String,
}

enum Installer {
    SelfReplace { exe: PathBuf },
    Desktop(Arc<dyn DesktopInstaller>),
}

pub struct Updater {
    state: Mutex<UpdateState>,
    installer: Installer,
    /// Signalled after `swarmdash serve` replaced itself; `main` then restarts.
    pub restart: Arc<Notify>,
}

impl Updater {
    pub fn new(desktop: Option<Arc<dyn DesktopInstaller>>) -> Arc<Updater> {
        let (installer, hint) = match desktop {
            Some(d) => {
                (Installer::Desktop(d), pubkey().is_none().then(|| format!("This build was made without an update signing key. {MANUAL}")))
            }
            None => {
                let exe = std::env::current_exe().unwrap_or_default();
                let hint = self_install_blocker(&exe);
                (Installer::SelfReplace { exe }, hint)
            }
        };
        Arc::new(Updater {
            state: Mutex::new(UpdateState {
                current: env!("CARGO_PKG_VERSION").into(),
                phase: Phase::Idle,
                releases: vec![],
                checked_at: None,
                error: None,
                received: 0,
                total: None,
                can_install: hint.is_none(),
                install_hint: hint,
                releases_url: format!("https://github.com/{REPO}/releases"),
            }),
            installer,
            restart: Arc::new(Notify::new()),
        })
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().unwrap().clone()
    }

    fn set(&self, f: impl FnOnce(&mut UpdateState)) {
        f(&mut self.state.lock().unwrap());
    }

    /// Checks once shortly after startup and then twice a day, unless the user turned it off.
    pub fn spawn_checks(self: &Arc<Self>, manager: Arc<Manager>) {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            loop {
                if manager.config().settings.check_updates {
                    me.check().await;
                }
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    pub async fn check(&self) -> UpdateState {
        {
            let mut st = self.state.lock().unwrap();
            if matches!(st.phase, Phase::Checking | Phase::Downloading | Phase::Installing | Phase::Restarting) {
                return st.clone();
            }
            st.phase = Phase::Checking;
            st.error = None;
        }
        let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver");
        let result = tokio::task::spawn_blocking(move || fetch_releases(&current)).await.map_err(|e| anyhow!(e));
        self.set(|st| {
            st.checked_at = Some(chrono::Utc::now().timestamp_millis());
            match result.and_then(|r| r) {
                Ok(releases) => {
                    st.phase = if releases.is_empty() { Phase::UpToDate } else { Phase::Available };
                    st.releases = releases;
                }
                Err(e) => {
                    tracing::info!("update check failed: {e:#}");
                    st.phase = Phase::Failed;
                    st.error = Some(format!("Could not check for updates: {e:#}"));
                }
            }
        });
        self.state()
    }

    /// Starts installing the newest release in the background.
    pub fn install(self: &Arc<Self>) -> Result<()> {
        let latest = {
            let mut st = self.state.lock().unwrap();
            if !st.can_install {
                bail!("{}", st.install_hint.clone().unwrap_or_else(|| "This copy cannot update itself.".into()));
            }
            if matches!(st.phase, Phase::Downloading | Phase::Installing | Phase::Restarting) {
                return Ok(());
            }
            let latest = st.releases.first().cloned().ok_or_else(|| anyhow!("No update is available."))?;
            st.phase = Phase::Downloading;
            st.received = 0;
            st.total = None;
            st.error = None;
            latest
        };
        let me = self.clone();
        let progress: Progress = {
            let me = self.clone();
            Arc::new(move |received, total| {
                me.set(|st| {
                    st.received = received;
                    st.total = total;
                    if total.is_some_and(|t| received >= t) {
                        st.phase = Phase::Installing;
                    }
                })
            })
        };
        tokio::spawn(async move {
            let result = match &me.installer {
                Installer::Desktop(d) => d.install(progress).await,
                Installer::SelfReplace { exe } => {
                    let exe = exe.clone();
                    let res = tokio::task::spawn_blocking(move || install_self(&latest, &exe, progress))
                        .await
                        .map_err(|e| anyhow!(e))
                        .and_then(|r| r);
                    if res.is_ok() {
                        me.set(|st| st.phase = Phase::Restarting);
                        // Give the UI a moment to see the new phase before the server goes away.
                        tokio::time::sleep(Duration::from_millis(600)).await;
                        me.restart.notify_one();
                    }
                    res
                }
            };
            if let Err(e) = result {
                tracing::warn!("update failed: {e:#}");
                me.set(|st| {
                    st.phase = Phase::Failed;
                    st.error = Some(format!("Update failed: {e:#}"));
                });
            }
        });
        Ok(())
    }
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(concat!("swarmdash/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(15)))
        .build()
        .into()
}

fn releases_api() -> String {
    std::env::var("SWARMDASH_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases?per_page=30"))
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

/// Published releases newer than `current`, newest first.
fn fetch_releases(current: &semver::Version) -> Result<Vec<Release>> {
    let list: Vec<GhRelease> =
        agent().get(&releases_api()).header("Accept", "application/vnd.github+json").call()?.body_mut().read_json()?;
    Ok(newer_releases(list, current))
}

fn newer_releases(list: Vec<GhRelease>, current: &semver::Version) -> Vec<Release> {
    let mut out: Vec<(semver::Version, Release)> = list
        .into_iter()
        .filter(|r| !r.draft && (!r.prerelease || !current.pre.is_empty()))
        .filter_map(|r| {
            let v = semver::Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            (v > *current).then(|| {
                let release = Release {
                    version: v.to_string(),
                    name: r.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.tag_name.clone()),
                    notes: r.body.unwrap_or_default(),
                    published_at: r.published_at,
                    url: r.html_url,
                    assets: r.assets,
                };
                (v, release)
            })
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, r)| r).collect()
}

/// Why `swarmdash serve` cannot replace itself here, if it cannot.
fn self_install_blocker(exe: &Path) -> Option<String> {
    if pubkey().is_none() {
        return Some(format!("This build was made without an update signing key. {MANUAL}"));
    }
    let s = exe.to_string_lossy();
    if s.contains("/Cellar/") || s.contains("/homebrew/") {
        return Some("swarmdash was installed with Homebrew. Run `brew upgrade swarmdash` to update.".into());
    }
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        return Some("This is a development build. Pull and rebuild to update.".into());
    }
    let dir = exe.parent()?;
    let probe = dir.join(".swarmdash-write-test");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(probe);
            None
        }
        Err(_) => Some(format!("swarmdash cannot write to {}. Update it the way you installed it. {MANUAL}", dir.display())),
    }
}

fn pubkey() -> Option<String> {
    #[cfg(debug_assertions)]
    if let Ok(k) = std::env::var("SWARMDASH_UPDATE_PUBKEY") {
        return Some(k);
    }
    Some(PUBKEY.trim().to_string()).filter(|k| !k.is_empty())
}

fn asset_name() -> String {
    format!("swarmdash-{TARGET}{}", std::env::consts::EXE_SUFFIX)
}

/// Downloads the release binary for this platform, verifies its signature and
/// swaps it in for the running executable.
fn install_self(release: &Release, exe: &Path, progress: Progress) -> Result<()> {
    let name = asset_name();
    let find = |n: &str| release.assets.iter().find(|a| a.name == n);
    let bin = find(&name).ok_or_else(|| anyhow!("Release {} has no download for this platform ({name}).", release.version))?;
    let sig = find(&format!("{name}.sig")).ok_or_else(|| anyhow!("Release {} has no signature for {name}.", release.version))?;
    let agent = agent();

    let sig_text = agent.get(&sig.browser_download_url).call()?.body_mut().read_to_string()?;
    let total = Some(bin.size).filter(|s| *s > 0);
    let mut reader = agent.get(&bin.browser_download_url).call()?.into_body().into_with_config().limit(512 * 1024 * 1024).reader();
    let mut data = Vec::with_capacity(bin.size as usize);
    let mut buf = vec![0u8; 64 * 1024];
    let mut last = 0;
    loop {
        let n = reader.read(&mut buf).context("download interrupted")?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() - last >= 256 * 1024 {
            last = data.len();
            progress(data.len() as u64, total);
        }
    }
    progress(data.len() as u64, Some(data.len() as u64));

    verify(&data, &sig_text)?;

    let tmp = exe.with_file_name(format!(".swarmdash-update-{}{}", release.version, std::env::consts::EXE_SUFFIX));
    std::fs::write(&tmp, &data).with_context(|| format!("could not write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    let replaced = self_replace::self_replace(&tmp).context("could not replace the swarmdash executable");
    let _ = std::fs::remove_file(&tmp);
    replaced
}

/// Checks a minisign signature in the format Tauri's signer writes
/// (base64 of the key and signature files), so one key signs everything.
fn verify(data: &[u8], sig_b64: &str) -> Result<()> {
    let key = pubkey().ok_or_else(|| anyhow!("no update signing key"))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let key_text = String::from_utf8(b64.decode(key.trim())?)?;
    let sig_text = String::from_utf8(b64.decode(sig_b64.trim())?)?;
    let pk = minisign_verify::PublicKey::decode(&key_text).map_err(|e| anyhow!("bad update key: {e}"))?;
    let sig = minisign_verify::Signature::decode(&sig_text).map_err(|e| anyhow!("bad signature file: {e}"))?;
    pk.verify(data, &sig, false).map_err(|_| anyhow!("the download's signature does not match. It was not installed."))
}

/// Starts the new executable in place of this process, keeping the address,
/// data folder and sign-in token so open browser tabs reconnect by themselves.
pub fn restart(exe: &Path, token: &str) -> Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1)).env("SWARMDASH_TOKEN", token).env("SWARMDASH_RESTARTED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(cmd.exec().into())
    }
    #[cfg(not(unix))]
    {
        cmd.spawn()?;
        std::process::exit(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(tag: &str, pre: bool) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            name: None,
            body: Some(format!("notes {tag}")),
            draft: false,
            prerelease: pre,
            published_at: None,
            html_url: String::new(),
            assets: vec![],
        }
    }

    #[test]
    fn lists_newer_releases_newest_first() {
        let cur = semver::Version::parse("0.2.0").unwrap();
        let got = newer_releases(
            vec![gh("v0.1.0", false), gh("v0.3.0", false), gh("v0.10.0", false), gh("v0.11.0-rc.1", true), gh("nightly", false)],
            &cur,
        );
        let versions: Vec<_> = got.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["0.10.0", "0.3.0"]);
        assert_eq!(got[0].name, "v0.10.0");
    }

    #[test]
    fn built_in_key_is_a_minisign_key() {
        let key = PUBKEY.trim();
        if key.is_empty() {
            return;
        }
        let text = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(key).unwrap()).unwrap();
        minisign_verify::PublicKey::decode(&text).unwrap();
    }
}
