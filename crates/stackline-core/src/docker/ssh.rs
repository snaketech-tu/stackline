//! SSH access to a remote Docker host.
//!
//! Stackline runs your system `ssh` so that `~/.ssh/config`, the agent, keys and
//! ProxyJump all work exactly as in a terminal. One SSH connection forwards the
//! remote Docker socket; every API request then goes through that single
//! connection. Nothing is installed or opened on the server.
//!
//! Whoever can reach the local end of the forward controls the server's Docker,
//! so on macOS and Linux it is a socket file in a folder only this user can
//! open, never a TCP port that every program on the computer (or a web page in
//! a browser) could connect to. Windows still forwards a loopback port.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::Endpoint;

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
    local: Local,
    stderr: Arc<Mutex<String>>,
}

/// The local end of the forward.
enum Local {
    /// A socket file in a private folder, removed with the tunnel.
    #[cfg(unix)]
    Socket(PrivateDir),
    #[cfg(not(unix))]
    Port(String),
}

impl Local {
    fn new() -> Result<Local> {
        #[cfg(unix)]
        {
            Ok(Local::Socket(PrivateDir::new()?))
        }
        #[cfg(not(unix))]
        {
            Ok(Local::Port(format!("127.0.0.1:{}", free_port()?)))
        }
    }

    /// What `ssh -L` listens on.
    fn listen_spec(&self) -> String {
        match self {
            #[cfg(unix)]
            Local::Socket(d) => d.socket().display().to_string(),
            #[cfg(not(unix))]
            Local::Port(a) => a.clone(),
        }
    }

    fn endpoint(&self) -> Endpoint {
        match self {
            #[cfg(unix)]
            Local::Socket(d) => Endpoint::Unix(d.socket()),
            #[cfg(not(unix))]
            Local::Port(a) => Endpoint::Tcp(a.clone()),
        }
    }
}

impl SshTunnel {
    pub async fn open(t: &SshTarget) -> Result<SshTunnel> {
        let local = Local::new()?;
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
        #[cfg(unix)]
        // Replace a stale socket file, and keep the new one to this user.
        cmd.args(["-o", "StreamLocalBindUnlink=yes", "-o", "StreamLocalBindMask=0177"]);
        cmd.arg("-L").arg(format!("{}:{}", local.listen_spec(), t.socket));
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
            if local.endpoint().connect().await.is_ok() {
                break;
            }
            if started.elapsed() > Duration::from_secs(30) {
                bail!("ssh to {} did not come up within 30 seconds", t.target);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(SshTunnel { child, local, stderr })
    }

    /// Where the Docker API of the server is reached through this tunnel.
    pub fn endpoint(&self) -> Endpoint {
        self.local.endpoint()
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Stops ssh right away, e.g. before Stackline exits or restarts.
    pub fn kill(&mut self) {
        let _ = self.child.start_kill();
        #[cfg(unix)]
        let Local::Socket(dir) = &self.local;
        #[cfg(unix)]
        let _ = std::fs::remove_dir_all(&dir.0);
    }

    /// The last thing ssh printed, e.g. a permission error on the remote socket.
    pub fn last_message(&self) -> Option<String> {
        let s = self.stderr.lock().unwrap();
        s.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
    }
}

/// A folder only this user can open, holding the tunnel's socket. Removed
/// when the tunnel goes away.
#[cfg(unix)]
struct PrivateDir(std::path::PathBuf);

#[cfg(unix)]
impl PrivateDir {
    fn new() -> Result<PrivateDir> {
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // The runtime folder is private already; the temporary folder is shared,
        // so the new folder must not exist yet (no one can have prepared it).
        let base = dirs::runtime_dir().filter(|d| d.is_dir()).unwrap_or_else(std::env::temp_dir);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for _ in 0..20 {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = base.join(format!("stackline-{}-{nanos:x}-{n}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    let d = PrivateDir(dir);
                    // A socket path longer than about 100 bytes cannot be bound (macOS: 104).
                    let len = d.socket().as_os_str().len();
                    if len > 100 {
                        bail!("the folder for the SSH socket has too long a path ({len} bytes): {}", d.0.display());
                    }
                    return Ok(d);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).with_context(|| format!("could not create a folder in {}", base.display())),
            }
        }
        bail!("could not create a private folder in {}", base.display())
    }

    fn socket(&self) -> std::path::PathBuf {
        self.0.join("docker.sock")
    }
}

#[cfg(unix)]
impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(not(unix))]
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::docker::DockerClient;

    /// Stands in for `ssh`: serves the `-L` socket and answers every request
    /// like Docker's `/version`, and writes down the arguments it got.
    const FAKE_SSH: &str = r#"#!/usr/bin/env python3
import os, socket, sys
args = sys.argv[1:]
open(os.environ["FAKE_SSH_ARGS"], "w").write("\n".join(args))
local = args[args.index("-L") + 1].rsplit(":", 1)[0]
s = socket.socket(socket.AF_UNIX)
s.bind(local)
s.listen(8)
while True:
    c, _ = s.accept()
    data = b""
    while b"\r\n\r\n" not in data:
        chunk = c.recv(4096)
        if not chunk:
            break
        data += chunk
    body = b'{"Version":"fake"}'
    try:
        if data:
            c.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s" % (len(body), body))
    except OSError:
        pass
    c.close()
"#;

    #[tokio::test]
    async fn the_tunnel_is_a_private_socket_not_a_port() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("stackline-fake-ssh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ssh = dir.join("ssh");
        std::fs::write(&ssh, FAKE_SSH).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args_file = dir.join("args");
        // Only this test runs ssh, so changing the environment is safe here.
        unsafe {
            std::env::set_var("STACKLINE_SSH", &ssh);
            std::env::set_var("FAKE_SSH_ARGS", &args_file);
        }
        let target = SshTarget { target: "me@example".into(), port: None, identity: None, jump: None, socket: default_socket() };
        let mut tunnel = SshTunnel::open(&target).await.unwrap();

        let Endpoint::Unix(sock) = tunnel.endpoint() else { panic!("an SSH tunnel must not listen on a TCP port") };
        let folder = sock.parent().unwrap().to_path_buf();
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
        let args = std::fs::read_to_string(&args_file).unwrap();
        assert!(args.contains(&format!("{}:/var/run/docker.sock", sock.display())), "{args}");
        assert!(args.contains("StreamLocalBindMask=0177") && args.contains("BatchMode=yes"), "{args}");

        let v: serde_json::Value = DockerClient::new(tunnel.endpoint()).get_json("/version").await.unwrap();
        assert_eq!(v["Version"], "fake");

        tunnel.kill();
        drop(tunnel);
        assert!(!folder.exists(), "the socket folder is removed with the tunnel");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
