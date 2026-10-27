# Changelog

Release notes for each version. The section of a version becomes the text of
its GitHub release, and swarmdash shows it in the update dialog.

## Unreleased

Fewer clicks everywhere.

### Everywhere
- **Ctrl K** (or ⌘ K) jumps to any service, stack, error group, page or connection; Shift Enter opens its logs
- Switch connections from the top bar; the theme button cycles System, Light and Dark
- Keyboard: `g` then `o` `s` `l` `c` `p` to change page, `/` to search logs, `j` `k` to move through lines, `c` to copy one
- The Nodes page is hidden while you are connected to a single node

### Overview
- The tiles open what they count, and "Needs attention" says why: the newest log line of a failing service, how many tasks failed before, and error groups that are new in the last 15 minutes
- Logs and Details buttons right on each item

### Stacks and services
- One table for all stacks with the columns lined up, and All, Problems and Pinned filters
- Hover a row for its logs or errors; every stack has a Stack logs button

### Service details
- Overview, Logs and Errors tabs, so you no longer leave the details to read logs
- A failing service shows its newest line, exit code and earlier failures at the top
- Logs of a single replica from the tasks table; step to the previous or next service with `[` and `]`

### Logs
- Click a level to see only that level, Shift click to add one
- One Sources picker for stacks and services, with pinned services on top
- Click an error group to filter the stream to it, with chips to undo each filter
- Hover a line to filter by its service, see the 30 seconds around it, or copy it
- The time range starts as Auto: everything stored, or the last 3 hours on a fresh install
- Download is one click, with the format and "everything" options in its menu

### Connections and settings
- Connections are one list: click to open, edit inline, test and see each step
- Adding a connection takes a name and `user@host`; port, key, jump host and socket are under Advanced
- Settings are one row each with a switch or menu on the right, saved as you change them
- Fixed the oversized checkboxes in settings and stack traces that ran out of their box

## 0.1.0

First release.

### Dashboard
- Overview with host or node health, services, running tasks or containers and errors of the last hour
- Docker Swarm (single and multi node) and plain Docker with Compose projects
- Stacks and services with replicas, image, ports, CPU, memory and errors per service
- **Pin your favorite services** with the star; they stay on top of the lists and on the overview
- Details for every service and node, including why a task or container stopped

### Logs
- Logs are read per replica, so multi-line stack traces never get mixed between replicas
- Multi-line entries from Java, .NET, Python, Node.js, PostgreSQL and MariaDB are joined
- JSON logs (ECS, Logstash, Serilog, pino, zap and similar) are read field by field
- Similar errors are grouped, with a count, sparkline and the full trace
- Time graph by level with exceptions on top; drag across it to zoom
- Search with free text or `service:` `stack:` `level:` `node:` `replica:`
- Live tail, collapse repeats, and download as `.log`, `.jsonl` or `.csv`
- Kept on your computer for 7 days (configurable), so the server is only asked for new lines

### Connections
- This computer, or any server over SSH using your own keys, agent and `~/.ssh/config`
- Strictly read-only: only GET requests ever reach the Docker API

### App
- Desktop app for Windows, macOS and Linux, or `swarmdash serve` to use it in the browser
- Checks for a new version on startup, shows what changed, and installs it with one click after verifying its signature
- Light and dark theme, following the system by default
