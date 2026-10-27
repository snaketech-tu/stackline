# swarmdash

[![CI](https://github.com/snaketech-tu/swarmdash/actions/workflows/ci.yml/badge.svg)](https://github.com/snaketech-tu/swarmdash/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/snaketech-tu/swarmdash)](https://github.com/snaketech-tu/swarmdash/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**A read-only dashboard for Docker Swarm and Compose, with logs that keep stack traces together.**

See which services need attention, inspect failed tasks, and investigate errors
across replicas. Run swarmdash on your computer and connect to Docker locally
or over SSH, with no agent to install on your servers.

Available as a desktop app for Windows, macOS and Linux, or a single binary
that opens the dashboard in your browser.

[Download](https://github.com/snaketech-tu/swarmdash/releases/latest) ·
[Get started](#get-started) · [Connect a host](#connect-a-host) ·
[Development](#development) · [Changelog](CHANGELOG.md)

## Features

- **Find what needs attention.** See nodes, stacks, services, replicas and stopped
  containers, with failure details and pinned services close at hand.
- **Follow a complete stack trace.** Logs are collected separately for each replica
  and multiline entries are reassembled, so traces from different replicas stay
  separate. Supports Java, .NET, Python, Node.js, PostgreSQL and MariaDB patterns.
- **Make repeated errors readable.** Group similar errors by exception and message,
  inspect their frequency over time, and expand the full trace.
- **Search and share logs.** Filter by service, stack, level, node or replica;
  zoom into the timeline, follow live logs, and export `.log`, `.jsonl` or `.csv`.
  Structured JSON logs include support for ECS, Logstash, Serilog, pino and zap.
- **Keep logs locally.** A SQLite store retains seven days by default. Searches
  use the local store; collection resumes from the last stored timestamp.
- **Inspect without changing workloads.** The Docker client only sends `GET`
  requests. swarmdash cannot start, stop, scale or remove Docker resources.

## Get started

### Install

Choose a download from the [latest release](https://github.com/snaketech-tu/swarmdash/releases/latest).
The desktop app provides its own window; the standalone binary serves the same
dashboard in your browser. Neither requires Rust or Node.js to run.

| Platform | Desktop app | Standalone binary |
| --- | --- | --- |
| Windows x64 | `.msi` or `-setup.exe` | `swarmdash-x86_64-pc-windows-msvc.exe` |
| macOS, Apple Silicon | Universal `.dmg` | `swarmdash-aarch64-apple-darwin` |
| macOS, Intel | Universal `.dmg` | `swarmdash-x86_64-apple-darwin` |
| Linux x64 | `.AppImage`, `.deb` or `.rpm` | `swarmdash-x86_64-unknown-linux-gnu` |
| Linux ARM64 | — | `swarmdash-aarch64-unknown-linux-gnu` |

Windows installers are not yet code-signed, and macOS builds are not yet
notarized, so the operating system may show a warning on first launch.
The built-in updater verifies update signatures before installation;
see [release details](RELEASING.md).

### Launch

Open the installed desktop app, or run the standalone binary from your terminal.
For example, on Linux x64:

```sh
chmod +x swarmdash-x86_64-unknown-linux-gnu
./swarmdash-x86_64-unknown-linux-gnu
```

On macOS or Linux ARM64, substitute the filename for your platform. On Windows,
use PowerShell:

```powershell
.\swarmdash-x86_64-pc-windows-msvc.exe
```

The standalone binary listens on `127.0.0.1:7070` and opens your browser with a
sign-in link. If the browser does not open, use the link printed in the terminal.
Press `Ctrl+C` to stop the server.

Running without a subcommand is equivalent to `swarmdash serve`. After renaming
the binary to `swarmdash` (`swarmdash.exe` on Windows) and putting it on your `PATH`,
you can customize startup:

```text
swarmdash serve [--port 7070] [--host 127.0.0.1] [--no-open] [--data-dir DIR]
```

## Connect a host

### Local Docker

Start Docker, then select **This computer** in swarmdash. It uses `DOCKER_HOST`
when set, otherwise a platform default socket or Windows named pipe. You can
set an explicit **Docker endpoint** under **Connections → Edit**, for example
`unix:///run/user/1000/docker.sock` for a rootless Docker installation.

### Remote Docker over SSH

You need an OpenSSH client on your computer and an SSH account that can access
the remote Docker socket. Docker does not need to be installed locally for this.

1. Connect once in your terminal to verify the host and set up authentication:
   `ssh user@host`. Exit the remote shell when finished.
2. Check that authentication and Docker access work without prompts:
   `ssh -o BatchMode=yes user@host docker ps`.
3. Open **Connections → Add connection**, choose **SSH to a server**, and enter
   `user@host` or an alias from `~/.ssh/config`.
4. Choose **Test and save**.

swarmdash uses your system `ssh`, including your SSH agent, config aliases,
ports and `ProxyJump`. Authentication must work without interactive password
or passphrase prompts; load an encrypted key into your SSH agent first.
Private keys are never copied or stored by swarmdash.

One SSH tunnel forwards the remote Docker socket, `/var/run/docker.sock` by
default. SSH forwarding must be allowed, and the SSH user must be able to open
that socket without `sudo`. You can override the socket path, key file, port
and jump host under **Advanced** in the connection form.

For **Docker Swarm**, connect to a **manager node** to see cluster nodes,
services and task logs. A worker connection shows only that host's containers.
CPU and memory metrics cover containers on the connected Docker host.
For **Compose**, containers are grouped by project and service.

## Investigate logs

Open **Logs** and combine free text with filters in the search box:

```text
service:api level:error timeout
stack:shop replica:2
node:worker-01 level:warn
```

Drag across the timeline to narrow the time range, select an error group to
filter the stream, then expand a log entry to inspect its trace. Enable **Live**
to follow new entries. Downloads use the current filters by default.

Collection depends on the logs Docker exposes through its API. swarmdash can
retain logs it has collected, but cannot recover logs Docker already removed
before collection.

## Settings and local data

Under **Preferences**, adjust log retention, the store size limit, refresh
intervals, CPU and memory collection, theme, and update checks. The defaults
are seven days of logs and a 5 GiB store size limit.

swarmdash is read-only toward Docker. It writes its own settings, cached logs,
exports and installed updates on the computer where it runs.

| Data | Linux | macOS | Windows |
| --- | --- | --- | --- |
| `config.json` — connections and settings | `~/.config/swarmdash` | `~/Library/Application Support/swarmdash` | `%APPDATA%\swarmdash` |
| `logs.db` — log store | `~/.local/share/swarmdash` | `~/Library/Application Support/swarmdash` | `%LOCALAPPDATA%\swarmdash` |

These are the default directories; Linux also honors XDG directory settings.
Use `swarmdash serve --data-dir DIR` to keep both files in a chosen directory.

When an update is available, swarmdash shows the release notes and lets you
choose when to install it. Every update is verified against the app's built-in
public key before installation.

## Development

You need **Rust stable** and **Node.js 24**. From a fresh clone:

```sh
git clone https://github.com/snaketech-tu/swarmdash.git
cd swarmdash
npm --prefix ui ci
npm --prefix ui run build
cargo run -- serve --port 7070
```

Build the UI first: its output in `ui/dist` is embedded into the Rust binary.

### UI development

Keep the Rust server running, then start Vite in another terminal from the
repository root:

```sh
npm --prefix ui run dev
```

Open the URL Vite prints, adding `?token=<token>` with the token from the Rust
server's sign-in link. Vite proxies `/api` to `127.0.0.1:7070`.

### Desktop development

The desktop app uses Tauri 2. After building the UI, run:

```sh
cd desktop
npm ci
npm run tauri -- dev
```

On Ubuntu/Debian, desktop builds additionally need `libwebkit2gtk-4.1-dev`,
`librsvg2-dev`, `libayatana-appindicator3-dev` and `libxdo-dev`.
The [CI workflow](.github/workflows/ci.yml) documents the build checks, and the
[release workflow](.github/workflows/release.yml) covers packaging per platform.

### Checks and demo data

Run the core and UI checks from the repository root:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
npm --prefix ui run check
npm --prefix ui run build
```

The [demo Compose project](dev/demo/compose.yaml) generates Java-style traces,
Python, PostgreSQL, Caddy and JSON logs. With Docker running, start it from the
repository root and select **This computer** in swarmdash:

```sh
docker compose -p swarmdash-demo -f dev/demo/compose.yaml up -d
```

Stop and remove the demo containers when finished:

```sh
docker compose -p swarmdash-demo -f dev/demo/compose.yaml down
```

A [demo Swarm stack](dev/demo/stack.yaml) is also available. For release
instructions, see [RELEASING.md](RELEASING.md).

## License

Licensed under [Apache-2.0](LICENSE).
