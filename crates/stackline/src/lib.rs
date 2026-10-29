//! The Stackline HTTP server: a JSON API over the core plus the embedded web UI.
//! Used by `stackline serve` and by the desktop app.

mod api;
pub mod update;
mod web;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use stackline_core::alerts::Notifier;
use stackline_core::config::Paths;
use stackline_core::runtime::Manager;
use update::{DesktopInstaller, Updater};

pub struct ServeOptions {
    pub addr: SocketAddr,
    pub paths: Paths,
    /// Running inside the desktop app (changes how downloads are saved).
    pub desktop: bool,
    /// The desktop app's updater; without one, Stackline updates its own executable.
    pub installer: Option<Arc<dyn DesktopInstaller>>,
    /// The desktop app's native notifications; without them the page shows alerts.
    pub notifier: Option<Arc<dyn Notifier>>,
}

pub struct Server {
    pub addr: SocketAddr,
    pub token: String,
    pub manager: Arc<Manager>,
    pub updater: Arc<Updater>,
}

impl Server {
    /// The URL that opens the UI and signs the browser in.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.token)
    }
}

pub struct AppState {
    pub manager: Arc<Manager>,
    pub token: String,
    pub desktop: bool,
    pub allowed_hosts: Vec<String>,
    pub updater: Arc<Updater>,
}

/// The whole HTTP service: sign-in guard, JSON API and embedded UI. Exposed
/// for tests that drive it without a listening socket.
pub fn router(state: Arc<AppState>) -> axum::Router {
    web::router(state)
}

/// Binds the server and starts it in the background.
pub async fn start(opts: ServeOptions) -> Result<Server> {
    let manager = Manager::new(opts.paths)?;
    if let Some(n) = opts.notifier {
        manager.alerts.set_notifier(n);
    }
    // After an update the new process keeps the old token, so open tabs stay signed in.
    let restarted = stackline_core::config::env_var("RESTARTED").is_some();
    let token = stackline_core::config::env_var("TOKEN")
        .filter(|t| t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(random_token);
    let listener = bind(opts.addr, restarted).await?;
    let addr = listener.local_addr()?;
    let mut allowed_hosts =
        vec![format!("127.0.0.1:{}", addr.port()), format!("localhost:{}", addr.port()), format!("[::1]:{}", addr.port())];
    if !addr.ip().is_loopback() {
        allowed_hosts.push(addr.to_string());
    }
    let updater = Updater::new(opts.installer);
    updater.spawn_checks(manager.clone());
    let state = Arc::new(AppState {
        manager: manager.clone(),
        token: token.clone(),
        desktop: opts.desktop,
        allowed_hosts,
        updater: updater.clone(),
    });
    let app = web::router(state);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("server stopped: {e}");
        }
    });
    Ok(Server { addr, token, manager, updater })
}

/// Binds the address. Right after an update the old process may still hold
/// the port for a moment, so keep trying for a few seconds.
async fn bind(addr: SocketAddr, retry: bool) -> Result<tokio::net::TcpListener> {
    let mut tries = 0;
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e) if retry && tries < 50 && e.kind() == std::io::ErrorKind::AddrInUse => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            Err(e) => return Err(anyhow::anyhow!("could not listen on {addr}: {e}")),
        }
    }
}

fn random_token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("no random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}
