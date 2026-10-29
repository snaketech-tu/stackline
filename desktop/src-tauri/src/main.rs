// The desktop app: runs the Stackline server on a random local port and shows
// it in a native window. Updates come through Tauri's updater, which verifies
// the download with the same key as the `stackline serve` binaries.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::anyhow;
use stackline::update::{DesktopInstaller, Progress};
use stackline::{ServeOptions, Server};
use stackline_core::alerts::Notifier;
use stackline_core::config::Paths;
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

const PUBKEY: &str = include_str!("../../../crates/stackline/updater.pub");

struct TauriInstaller(AppHandle);

impl DesktopInstaller for TauriInstaller {
    fn install(&self, progress: Progress) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let app = self.0.clone();
        Box::pin(async move {
            let update =
                app.updater()?.check().await?.ok_or_else(|| anyhow!("No installer for this platform was found in the latest release."))?;
            let mut received = 0u64;
            let bytes = update
                .download(
                    |chunk, total| {
                        received += chunk as u64;
                        progress(received, total);
                    },
                    || {},
                )
                .await?;
            // Close SSH tunnels first: on Windows the installer ends this process.
            if let Some(server) = app.try_state::<Server>() {
                server.manager.shutdown();
            }
            update.install(bytes)?;
            app.restart()
        })
    }
}

/// Error-rate alerts as native notifications.
struct TauriNotifier(AppHandle);

impl Notifier for TauriNotifier {
    fn notify(&self, title: &str, body: &str) -> anyhow::Result<()> {
        self.0.notification().builder().title(title).body(body).show()?;
        Ok(())
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().pubkey(PUBKEY.trim()).build())
        .setup(|app| {
            let installer: Arc<dyn DesktopInstaller> = Arc::new(TauriInstaller(app.handle().clone()));
            let opts = ServeOptions {
                addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                paths: Paths::default_paths(),
                desktop: true,
                installer: Some(installer),
                notifier: Some(Arc::new(TauriNotifier(app.handle().clone()))),
            };
            let server = tauri::async_runtime::block_on(stackline::start(opts))?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(server.url().parse()?))
                .title("Stackline")
                .inner_size(1360.0, 860.0)
                .min_inner_size(380.0, 520.0)
                .build()?;
            app.manage(server);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Stackline could not start")
        .run(|app, event| {
            if let RunEvent::Exit = event
                && let Some(server) = app.try_state::<Server>()
            {
                server.manager.shutdown();
            }
        });
}
