//! Where the Docker Engine API lives and how to open a byte stream to it.

use std::io;
use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncWrite};

/// A bidirectional byte stream to the Docker Engine API.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// Unix domain socket, e.g. `/var/run/docker.sock`.
    Unix(PathBuf),
    /// Windows named pipe, e.g. `\\.\pipe\docker_engine`.
    NamedPipe(String),
    /// Plain TCP, `host:port`. Used for the local end of an SSH tunnel.
    Tcp(String),
}

impl Endpoint {
    /// The Docker endpoint of this computer: `DOCKER_HOST` if set, otherwise the
    /// platform default (named pipe on Windows, the first existing socket elsewhere).
    /// Podman's sockets come after Docker's, so a computer with only Podman works too.
    pub fn local_default() -> Endpoint {
        if let Ok(host) = std::env::var("DOCKER_HOST")
            && let Some(e) = Endpoint::parse(&host)
        {
            return e;
        }
        #[cfg(windows)]
        {
            let docker = r"\\.\pipe\docker_engine";
            let podman = r"\\.\pipe\podman-machine-default";
            let pick = if !std::path::Path::new(docker).exists() && std::path::Path::new(podman).exists() { podman } else { docker };
            Endpoint::NamedPipe(pick.to_string())
        }
        #[cfg(not(windows))]
        {
            let mut candidates = vec![PathBuf::from("/var/run/docker.sock")];
            if let Some(home) = dirs::home_dir() {
                candidates.push(home.join(".docker/run/docker.sock"));
                candidates.push(home.join(".colima/default/docker.sock"));
                candidates.push(home.join(".orbstack/run/docker.sock"));
            }
            if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
                candidates.push(PathBuf::from(&rt).join("docker.sock"));
                // Podman without root: `systemctl --user enable --now podman.socket`.
                candidates.push(PathBuf::from(rt).join("podman/podman.sock"));
            }
            // Podman as root: `systemctl enable --now podman.socket`.
            candidates.push(PathBuf::from("/run/podman/podman.sock"));
            // Podman machine on macOS.
            if let Some(home) = dirs::home_dir() {
                candidates.push(home.join(".local/share/containers/podman/machine/podman.sock"));
            }
            if let Ok(tmp) = std::env::var("TMPDIR") {
                candidates.push(PathBuf::from(tmp).join("podman/podman-machine-default-api.sock"));
            }
            let found = candidates.iter().find(|p| p.exists()).cloned();
            Endpoint::Unix(found.unwrap_or_else(|| candidates[0].clone()))
        }
    }

    /// Parses `unix://…`, `npipe://…` and `tcp://host:port`.
    pub fn parse(s: &str) -> Option<Endpoint> {
        let s = s.trim();
        if let Some(p) = s.strip_prefix("unix://") {
            return Some(Endpoint::Unix(PathBuf::from(p)));
        }
        if let Some(p) = s.strip_prefix("npipe://") {
            return Some(Endpoint::NamedPipe(p.replace('/', "\\")));
        }
        if let Some(p) = s.strip_prefix("tcp://").or_else(|| s.strip_prefix("http://")) {
            return Some(Endpoint::Tcp(p.trim_end_matches('/').to_string()));
        }
        if s.starts_with('/') {
            return Some(Endpoint::Unix(PathBuf::from(s)));
        }
        if s.starts_with(r"\\.\pipe\") {
            return Some(Endpoint::NamedPipe(s.to_string()));
        }
        None
    }

    pub fn describe(&self) -> String {
        match self {
            Endpoint::Unix(p) => format!("unix://{}", p.display()),
            Endpoint::NamedPipe(p) => format!("npipe://{}", p.replace('\\', "/")),
            Endpoint::Tcp(a) => format!("tcp://{a}"),
        }
    }

    pub async fn connect(&self) -> io::Result<Box<dyn Stream>> {
        match self {
            Endpoint::Tcp(addr) => {
                let s = tokio::net::TcpStream::connect(addr).await?;
                s.set_nodelay(true)?;
                Ok(Box::new(s))
            }
            #[cfg(unix)]
            Endpoint::Unix(path) => Ok(Box::new(tokio::net::UnixStream::connect(path).await?)),
            #[cfg(not(unix))]
            Endpoint::Unix(_) => Err(io::Error::new(io::ErrorKind::Unsupported, "Unix sockets are not available on this platform")),
            #[cfg(windows)]
            Endpoint::NamedPipe(name) => {
                use tokio::net::windows::named_pipe::ClientOptions;
                const ERROR_PIPE_BUSY: i32 = 231;
                let mut tries = 0;
                loop {
                    match ClientOptions::new().open(name) {
                        Ok(c) => return Ok(Box::new(c)),
                        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && tries < 20 => {
                            tries += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            #[cfg(not(windows))]
            Endpoint::NamedPipe(_) => Err(io::Error::new(io::ErrorKind::Unsupported, "Named pipes are only available on Windows")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_host_values() {
        assert_eq!(Endpoint::parse("unix:///var/run/docker.sock"), Some(Endpoint::Unix("/var/run/docker.sock".into())));
        assert_eq!(Endpoint::parse("npipe:////./pipe/docker_engine"), Some(Endpoint::NamedPipe(r"\\.\pipe\docker_engine".into())));
        assert_eq!(Endpoint::parse("tcp://127.0.0.1:2375"), Some(Endpoint::Tcp("127.0.0.1:2375".into())));
        assert_eq!(Endpoint::parse("ssh://me@host"), None);
    }
}
