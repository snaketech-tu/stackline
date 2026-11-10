use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use stackline::{ServeOptions, start, update};
use stackline_core::config::Paths;

/// A dashboard for Docker Swarm and Compose stacks, read-only unless you switch it off per connection.
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
        /// Never change anything, whatever a connection's read-only switch says.
        /// The same as STACKLINE_READ_ONLY=1.
        #[arg(long)]
        read_only: bool,
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
    let restarted = stackline_core::config::env_var("RESTARTED").is_some();
    let (port, host, no_open, data_dir, read_only) = match cli.command {
        Some(Command::Serve { port, host, no_open, data_dir, read_only }) => (port, host, no_open, data_dir, read_only),
        None => (7070, IpAddr::from([127, 0, 0, 1]), false, None, false),
    };
    // A restart after an update passes the same arguments, so this stays on.
    if read_only {
        stackline_core::runtime::force_read_only();
    }
    let paths = data_dir.as_deref().map(Paths::in_dir).unwrap_or_else(Paths::default_paths);
    let server = start(ServeOptions { addr: SocketAddr::new(host, port), paths, desktop: false, installer: None, notifier: None }).await?;
    let url = server.url();
    if restarted {
        println!("Stackline {} is running at {url}", env!("CARGO_PKG_VERSION"));
    } else {
        println!("Stackline is running. Open this link to use it:\n\n  {url}\n\nPress Ctrl+C to stop.");
        if !host.is_loopback() {
            println!(
                "\nStackline listens beyond this computer ({host}). Other machines open it with this computer's IP address\n\
                 in place of {}. The link carries the sign-in and travels unencrypted: share it only on a network you trust.",
                server.addr.ip()
            );
        }
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
