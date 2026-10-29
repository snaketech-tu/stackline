//! SSH access to a remote Docker host.
//!
//! Stackline runs your system `ssh` so that `~/.ssh/config`, the agent, keys and
//! ProxyJump all work exactly as in a terminal. One SSH connection forwards a
//! local TCP port to the remote Docker socket; every API request then goes
//! through that single connection. Nothing is installed or opened on the server.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshTarget {
    /// `host`, `user@host` or an alias from `~/.ssh/config`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// A specific private key; otherwise ssh picks keys and the agent as usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// ProxyJump host, e.g. `me@bastion:22`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<String>,
    /// Docker socket path on the remote host.
    #[serde(default = "default_socket")]
    pub socket: String,
}

pub fn default_socket() -> String {
    "/var/run/docker.sock".to_string()
}

pub struct SshTunnel {
    child: Child,
    local_addr: String,
    stderr: Arc<Mutex<String>>,
}

impl SshTunnel {
    pub async fn open(t: &SshTarget) -> Result<SshTunnel> {
        let port = free_port()?;
        let local_addr = format!("127.0.0.1:{port}");
        let mut cmd = Command::new(ssh_binary());
        cmd.args([
            "-N",
            "-T",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
        ]);
        if let Some(p) = t.port {
            cmd.arg("-p").arg(p.to_string());
        }
        if let Some(i) = t.identity.as_deref().filter(|s| !s.is_empty()) {
            cmd.arg("-i").arg(expand_tilde(i));
        }
        if let Some(j) = t.jump.as_deref().filter(|s| !s.is_empty()) {
            cmd.arg("-J").arg(j);
        }
        cmd.arg("-L").arg(format!("{local_addr}:{}", t.socket));
        cmd.arg(&t.target);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        cmd.env_remove("STACKLINE_TOKEN").env_remove("SWARMDASH_TOKEN");
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().context("could not start `ssh`. Is the OpenSSH client installed and on PATH?")?;

        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(err) = child.stderr.take() {
            let sink = stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(err).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut s = sink.lock().unwrap();
                    s.push_str(&line);
                    s.push('\n');
                    if s.len() > 8192 {
                        let cut = s.len() - 4096;
                        s.drain(..cut);
                    }
                }
            });
        }

        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let msg = stderr.lock().unwrap().trim().to_string();
                bail!("ssh to {} exited ({status}){}", t.target, if msg.is_empty() { String::new() } else { format!(": {msg}") });
            }
            if tokio::net::TcpStream::connect(&local_addr).await.is_ok() {
                break;
            }
            if started.elapsed() > Duration::from_secs(30) {
                bail!("ssh to {} did not come up within 30 seconds", t.target);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(SshTunnel { child, local_addr, stderr })
    }

    pub fn local_addr(&self) -> &str {
        &self.local_addr
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stops ssh right away, e.g. before Stackline exits or restarts.
    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
    }

    /// The last thing ssh printed, e.g. a permission error on the remote socket.
    pub fn last_message(&self) -> Option<String> {
        let s = self.stderr.lock().unwrap();
        s.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
    }
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// The ssh client to run: `STACKLINE_SSH` (or `SWARMDASH_SSH`) if set, otherwise `ssh` from PATH.
fn ssh_binary() -> String {
    crate::config::env_var("SSH").filter(|s| !s.is_empty()).unwrap_or_else(|| "ssh".to_string())
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).display().to_string();
    }
    p.to_string()
}
