<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="ui/public/brand/stackline-dark.svg">
    <img src="ui/public/brand/stackline-light.svg" alt="Stackline" width="420">
  </picture>
</h1>

[![CI](https://github.com/snaketech-tu/stackline/actions/workflows/ci.yml/badge.svg)](https://github.com/snaketech-tu/stackline/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/snaketech-tu/stackline)](https://github.com/snaketech-tu/stackline/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**A dashboard for Docker Swarm and Compose that is read-only until you say otherwise, with logs that keep stack traces together.**

See which services need attention, inspect failed tasks, and investigate errors
across replicas. Run Stackline on your computer and connect to Docker locally
or over SSH, with no agent to install on your servers.

Available as a desktop app for Windows, macOS and Linux, or a single binary
that opens the dashboard in your browser.

[Download](https://github.com/snaketech-tu/stackline/releases/latest) ·
[Get started](#get-started) · [Connect a host](#connect-a-host) ·
[Security](#security) · [Development](#development) · [Changelog](CHANGELOG.md)

## Features

- **Find what needs attention.** See nodes, stacks, services, replicas and stopped
  containers, with failure details and pinned services close at hand.
- **Follow a complete stack trace.** Logs are collected separately for each replica
  and multiline entries are reassembled, so traces from different replicas stay
  separate. Supports Java, .NET, Python, Node.js, Go, Rust, PHP, Ruby, PostgreSQL,
  MariaDB, MongoDB, Redis, nginx, Traefik and Caddy patterns.
- **Make repeated errors readable.** Group similar errors by exception and message,
  inspect their frequency over time, and expand the full trace.
- **Search and share logs.** Filter by service, stack, level, node or replica;
  zoom into the timeline, follow live logs, copy a link to the view, and export
  `.log`, `.jsonl` or `.csv`.
  Structured JSON logs include support for ECS, Logstash, Serilog, pino and zap.
- **See when containers crashed.** Starts, stops, crashes, out-of-memory kills
  and failed health checks from Docker's events are marked on the time graphs,
  each with the log lines its replica printed just before.
- **See what a deploy changed.** Deploys are marked too, with what changed
  (image, replicas, environment variable names, ports, limits), and errors that
  only started after a deploy are flagged as **new since deploy**.
- **See how a service is set up.** Image, command, ports, networks, mounts,
  restart policy, limits, health checks with their latest results, environment
  variables (secret-looking values stay hidden), labels and swarm settings.
- **Get told when errors pile up.** An optional alert sends a notification when a
  connection logs more errors than you allow within 5 to 15 minutes.
- **Keep logs locally.** A SQLite store retains two days by default. Searches
  use the local store; collection resumes from the last stored timestamp.
  Choose per service whose logs are collected, so chatty services do not fill
  the disk.
- **Inspect without changing workloads.** The Docker client only sends `GET`
  requests. Stackline cannot start, stop, scale or remove Docker resources.

## Get started

### Install

Choose a download from the [latest release](https://github.com/snaketech-tu/stackline/releases/latest).
The desktop app provides its own window; the standalone binary serves the same
dashboard in your browser. Neither requires Rust or Node.js to run.

| Platform | Desktop app | Standalone binary |
| --- | --- | --- |
| Windows x64 | `.msi` or `-setup.exe` | `stackline-x86_64-pc-windows-msvc.exe` |
| macOS, Apple Silicon | Universal `.dmg` | `stackline-aarch64-apple-darwin` |
| macOS, Intel | Universal `.dmg` | `stackline-x86_64-apple-darwin` |
| Linux x64 | `.AppImage`, `.deb` or `.rpm` | `stackline-x86_64-unknown-linux-gnu` |
| Linux ARM64 | — | `stackline-aarch64-unknown-linux-gnu` |

Windows installers are not yet code-signed, and macOS builds are not yet
notarized, so the operating system may show a warning on first launch.
The built-in updater verifies update signatures before installation;
see [release details](RELEASING.md).

### Launch

Open the installed desktop app, or run the standalone binary from your terminal.
For example, on Linux x64:

```sh
chmod +x stackline-x86_64-unknown-linux-gnu
./stackline-x86_64-unknown-linux-gnu
```

On macOS or Linux ARM64, substitute the filename for your platform. On Windows,
use PowerShell:

```powershell
.\stackline-x86_64-pc-windows-msvc.exe
```

The standalone binary listens on `127.0.0.1:7070` and opens your browser with a
sign-in link. If the browser does not open, use the link printed in the terminal.
Press `Ctrl+C` to stop the server.

Running without a subcommand is equivalent to `stackline serve`. After renaming
the binary to `stackline` (`stackline.exe` on Windows) and putting it on your `PATH`,
you can customize startup:

```text
stackline serve [--port 7070] [--host 127.0.0.1] [--no-open] [--data-dir DIR] [--read-only]
```

## Connect a host

### Local Docker

Start Docker, then select **This computer** in Stackline. It uses `DOCKER_HOST`
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

Stackline uses your system `ssh`, including your SSH agent, config aliases,
ports and `ProxyJump`. Authentication must work without interactive password
or passphrase prompts; load an encrypted key into your SSH agent first.
Private keys are never copied or stored by Stackline.

One SSH tunnel forwards the remote Docker socket, `/var/run/docker.sock` by
default. SSH forwarding must be allowed, and the SSH user must be able to open
that socket without `sudo`. You can override the socket path, key file, port
and jump host under **Advanced** in the connection form.

### Podman

Podman answers the same API as Docker, so Stackline reads it the same way.
Switch on its API socket first: `systemctl enable --now podman.socket` as root,
or `systemctl --user enable --now podman.socket` for Podman without root.
**This computer** finds the socket by itself when there is no Docker. Over SSH,
pick **Podman as root** (`/run/podman/podman.sock`) or **Podman without root**
(`/run/user/<uid>/podman/podman.sock`, where `id -u` on the server prints the
uid) under **Advanced**. Containers started with `podman compose` or
`podman-compose` are grouped like Compose projects. Podman has no swarm mode.

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

**Pick any time range.** Besides the buttons (Auto, 15m, 1h, 3h, 24h, 7d), the
clock button (or `t`) takes a typed range, quick picks, exact From and To
fields and your recent ranges. Ctrl K takes the same text. For example:

```text
90m                      last 90 minutes
today / yesterday        the whole day (today runs until now)
since 8:00               from 08:00 until now, following new lines
14:00-15:30              today, or yesterday when that is still ahead
yesterday 22:00 to 02:00 across midnight
9.11. 8:00 - 12:00       a day without a year is the last one that passed
2026-11-09               one whole day
14:05 ± 5m               around a moment
now-6h to now-2h         relative to now
```

`[` and `]` move to the range before or after (same length), `-` zooms out.
Copy link keeps the exact range.

Drag across the timeline to narrow the time range, select an error group to
filter the stream, then expand a log entry to inspect its trace. Enable **Live**
to follow new entries. Downloads use the current filters by default.

The stream uses the full width, with the error groups above it; the layout
button moves the groups beside it, and the expand button (or `f`) fills the
window with the stream. **Show more** loads older lines, up to 5,000 at once.

Stored logs belong to the host or swarm a connection reaches. When you point a
connection at a different host, its old lines are deleted so the two never mix.

By default Stackline collects the logs of every service, on every saved
connection from the moment it starts, so the lines are there when something
breaks on a host you did not have open. Switch off **Collect every connection
from the start** in Preferences to collect only the connections you open. On **Stacks &
services**, the **Collect** switches turn collection off for single services or
whole stacks, and **Only chosen** collects nothing but the services you switch
on (it starts from your pinned services). A service that is switched on again
is read from that moment, without fetching what it logged in between. The
filters you pick on the Logs and Stacks pages stay as they are when you move
around the app or restart it.

**Debug report** in a service's panel copies a Markdown summary for a ticket:
configuration, replicas, the most frequent errors, crashes and the lines just
before the last crash. Environment values are never included. The Overview
also says when a running service that logged steadily **went quiet** for 10
minutes, which is often a hanging process.

Collection depends on the logs Docker exposes through its API. Stackline can
retain logs it has collected, but cannot recover logs Docker already removed
before collection.

## Settings and local data

Under **Preferences**, adjust log retention, the store size limit, refresh
intervals, CPU and memory collection, theme, and update checks. The defaults
are two days of logs and a 1 GiB store size limit. Stackline checks both
every minute: when the store is over the limit it deletes the oldest lines
until it is at about 90% of it, and the file on disk shrinks with it. The
**Local store** section shows how much each service of the selected connection
takes, with a switch to stop collecting it and a button to delete its lines.

**Alerts** are off until you switch them on. Stackline then counts the errors
of every connection opened since it started, over the last 5, 10 or 15 minutes,
and notifies you once when the count reaches your threshold. It notifies again
only after the count dropped and at least one window has passed. The desktop
app uses system notifications; in a browser, allow notifications for the
Stackline tab. Only the services whose logs are collected count toward the
alert.

**Every connection starts read-only**: only reads reach Docker. Click
**Read-only** in the top bar to allow changes on that connection. Stackline can
then restart, stop and start a service's containers, and restart or scale a
swarm service. It never creates or removes anything, and each change asks first
and shows the `docker` command it amounts to. The server enforces this, not
only the page. `stackline serve --read-only` (or `STACKLINE_READ_ONLY=1`) keeps
every connection read-only, whatever was switched before.

Stackline writes its own settings, cached logs, exports and installed updates
on the computer where it runs.

| Data | Linux | macOS | Windows |
| --- | --- | --- | --- |
| `config.json` — connections and settings | `~/.config/stackline` | `~/Library/Application Support/stackline` | `%APPDATA%\stackline` |
| `logs.db` — log store | `~/.local/share/stackline` | `~/Library/Application Support/stackline` | `%LOCALAPPDATA%\stackline` |

These are the default directories; Linux also honors XDG directory settings.

Stackline was called swarmdash before. On its first start it moves the
`swarmdash` folders above to `stackline`, so an update keeps everything.

Use `stackline serve --data-dir DIR` to keep both files in a chosen directory.

When an update is available, Stackline shows the release notes and lets you
choose when to install it. Every update is verified against the app's built-in
public key before installation.

## Security

Stackline can reach the Docker of every server you connect, and whoever
controls a server's Docker controls the server. This is how it keeps that
access to you:

- **Only reads by default.** Stackline sends Docker requests that read:
  lists, logs and events. Log downloads are files written on your computer.
- **SSH connections stay private.** Stackline reaches a server's Docker
  socket through your own `ssh`. On macOS and Linux the tunnel ends in a
  socket file in a folder only your user can open, which is removed when
  Stackline stops. On Windows it still ends in a port on `127.0.0.1` that
  other programs on the computer could use, so do not run Stackline on a
  Windows machine you share with people you do not trust. Your private keys
  are never read or copied.
- **Only your browser tab can use it.** The standalone server listens on
  `127.0.0.1`. Every API call needs the random sign-in token from the link it
  prints; the link stores it in a cookie that only Stackline's own pages send
  (`HttpOnly`, `SameSite=Strict`). Requests addressed to any other name than
  `localhost` or `127.0.0.1` are refused, which keeps web pages from reaching
  Stackline through DNS rebinding.
- **`--host 0.0.0.0` makes it reachable from the network**, for anyone who has
  the link. The connection is plain HTTP, so the token can be read on the way.
  Only do this on a network you trust, or put Stackline behind a reverse proxy
  that adds TLS. Stackline prints a warning when it listens beyond loopback.
- **Updates are signed.** An update is installed only when its signature
  matches the public key built into the app.

To report a security problem, open a private
[security advisory](https://github.com/snaketech-tu/stackline/security/advisories/new)
on GitHub instead of an issue.

## Development

You need **Rust stable** and **Node.js 24**. From a fresh clone:

```sh
git clone https://github.com/snaketech-tu/stackline.git
cd stackline
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
repository root and select **This computer** in Stackline:

```sh
docker compose -p stackline-demo -f dev/demo/compose.yaml up -d
```

Stop and remove the demo containers when finished:

```sh
docker compose -p stackline-demo -f dev/demo/compose.yaml down
```

A [demo Swarm stack](dev/demo/stack.yaml) is also available. For release
instructions, see [RELEASING.md](RELEASING.md).

### UI tests

The [UI tests](e2e/tests) click through Stackline in a browser while the demo
Compose project runs, and check the things that broke before: error messages
cut off, menus drawn under cards, the store size not updating, filters lost
on the way between pages. Stackline reaches Docker through a guard that
refuses anything but reads, and the last test fails if Stackline sent one.
CI runs them on every pull request. To run them yourself on Linux or macOS,
start the demo project, build the UI and Stackline, then:

```sh
cd e2e
npm ci
npx playwright install chromium
npx playwright test --project ui --project read-only
```

They start their own Stackline on port 7171 with an empty data folder
(`e2e/.run`), so your own connections and stored logs are not touched. Set
`STACKLINE_BIN` to test another build, such as `target/release/stackline`.

The [swarm tests](e2e/tests/swarm.spec.ts) need the demo Swarm stack instead
(`docker swarm init`, then `docker stack deploy -c stack.yaml shop` in
`dev/demo`) and run with `npx playwright test --project swarm`. CI runs them in
a job of their own.

## License

Licensed under [Apache-2.0](LICENSE).
