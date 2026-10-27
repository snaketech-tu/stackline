# swarmdash

A fast, read-only dashboard for Docker Swarm and Docker Compose hosts, on your
own computer or on a server you reach over SSH.

- **Overview** of nodes, stacks, services and containers, with what needs attention first
- **Logs that keep stack traces whole.** swarmdash reads every replica on its own, so
  multi-line traces from Java, .NET, Python, Node.js, PostgreSQL or MariaDB are never
  interleaved, unlike `docker service logs`
- **Errors grouped** by exception and message, with a time graph, search, live tail and download
- **Pinned services** stay on top of every list
- **Local log store** (SQLite, 7 days by default), so the server is only asked for new lines
- **Strictly read-only.** The Docker client in swarmdash can only send GET requests. Nothing
  can start, stop, scale or remove anything. Downloading logs to your computer is the only write.
- Desktop app for Windows, macOS and Linux, or a single binary that serves the UI to your browser
- Updates itself after showing what changed, verifying every download's signature first

## Install

Download the latest release from the
[releases page](https://github.com/snaketech-tu/swarmdash/releases/latest):

| | Desktop app | Single binary (`swarmdash serve`) |
| --- | --- | --- |
| Windows | `.msi` or `-setup.exe` | `swarmdash-x86_64-pc-windows-msvc.exe` |
| macOS | `.dmg` (Apple Silicon and Intel) | `swarmdash-aarch64-apple-darwin`, `swarmdash-x86_64-apple-darwin` |
| Linux | `.AppImage`, `.deb`, `.rpm` | `swarmdash-x86_64-unknown-linux-gnu`, `swarmdash-aarch64-unknown-linux-gnu` |

The single binary needs nothing else. Make it executable and run it:

```sh
chmod +x swarmdash-x86_64-unknown-linux-gnu
./swarmdash-x86_64-unknown-linux-gnu
```

It listens on `127.0.0.1:7070` and opens your browser with a sign-in link. Options:

```
swarmdash serve [--port 7070] [--host 127.0.0.1] [--no-open] [--data-dir DIR]
```

## Connecting to a server

Add the server under **Connections** with `user@host` or an alias from
`~/.ssh/config`. swarmdash runs your own `ssh`, so your keys, agent, ports and
ProxyJump work exactly as in a terminal, and it never stores keys. It forwards
the server's Docker socket through that one SSH connection.

The SSH user needs read access to the Docker socket on the server, which
usually means being in the `docker` group. If `ssh user@host docker ps` works,
swarmdash works.

For a swarm, connect to a manager node. For plain Docker, containers are
grouped by their Compose project.

## Where data is kept

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Connections and settings | `~/.config/swarmdash` | `~/Library/Application Support/swarmdash` | `%APPDATA%\swarmdash` |
| Log store | `~/.local/share/swarmdash` | `~/Library/Application Support/swarmdash` | `%LOCALAPPDATA%\swarmdash` |

## Development

You need Rust (stable) and Node.js 24.

```sh
cd ui && npm ci && npm run build && cd ..   # the UI is embedded into the binary
cargo run -- serve --port 7070               # server plus the built UI
```

For UI work, run the server and `npm run dev` in `ui/`; Vite proxies `/api`
to port 7070. Open the dev server with the `?token=` from the server's link.

The desktop app lives in `desktop/` (Tauri 2):

```sh
cd desktop && npm ci && npx tauri dev
```

On Linux it needs `libwebkit2gtk-4.1-dev`, `librsvg2-dev` and
`libayatana-appindicator3-dev`.

`dev/demo` has a Compose project and a swarm stack that write Java, Python,
PostgreSQL, Caddy and JSON logs, for trying swarmdash locally.

Releases are described in [RELEASING.md](RELEASING.md).

## License

Apache-2.0
