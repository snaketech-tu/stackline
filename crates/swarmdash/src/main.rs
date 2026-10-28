use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use swarmdash::{ServeOptions, start, update};
use swarmdash_core::config::Paths;

/// A read-only dashboard for Docker Swarm and Compose stacks.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the dashboard on this computer and open it in your browser (default).
    Serve {
        /// Port to listen on.
        #[arg(long, default_value_t = 7070)]
        port: u16,
        /// Address to listen on. Keep the default unless you know you need another.
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Do not open a browser.
        #[arg(long)]
        no_open: bool,
        /// Keep config and logs in this folder instead of the default locations.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cli = Cli::parse();
    // Remember where we were started from: after a self-update the running
    // file has been swapped and the new one must be started from this path.
    let exe = std::env::current_exe()?;
    let restarted = std::env::var_os("SWARMDASH_RESTARTED").is_some();
    let (port, host, no_open, data_dir) = match cli.command {
        Some(Command::Serve { port, host, no_open, data_dir }) => (port, host, no_open, data_dir),
        None => (7070, IpAddr::from([127, 0, 0, 1]), false, None),
    };
    let paths = data_dir.as_deref().map(Paths::in_dir).unwrap_or_else(Paths::default_paths);
    let server = start(ServeOptions { addr: SocketAddr::new(host, port), paths, desktop: false, installer: None, notifier: None }).await?;
    let url = server.url();
    if restarted {
        println!("swarmdash {} is running at {url}", env!("CARGO_PKG_VERSION"));
    } else {
        println!("swarmdash is running. Open this link to use it:\n\n  {url}\n\nPress Ctrl+C to stop.");
        if !no_open {
            let _ = webbrowser::open(&url);
        }
    }
    let restart = tokio::select! {
        r = tokio::signal::ctrl_c() => { r?; false }
        _ = server.updater.restart.notified() => true,
    };
    server.manager.shutdown();
    if restart {
        println!("Updated. Restarting…");
        update::restart(&exe, &server.token)?;
    }
    Ok(())
}
