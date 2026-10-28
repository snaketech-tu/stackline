# Changelog

Release notes for each version. The section of a version becomes the text of
its GitHub release, and swarmdash shows it in the update dialog.

## Unreleased

### Logs
- The log stream now uses the full width, with the error groups above it. A layout button puts them back beside the stream, and the groups can be hidden
- The expand button (or `f`) fills the window with the stream; Esc goes back
- **Show more** loads older lines, up to 5,000

### Service details
- The details panel opens wide and can fill the window
- New sections: command and entrypoint, restart policy and restarts, limits and reservations, health check with its latest results, networks, mounts, environment variables, labels, and swarm update, placement, secrets and configs
- Values of environment variables that look like secrets (passwords, tokens, keys) are never shown, and passwords inside URLs are masked
- The Logs tab has its own search and shows up to 5,000 lines

### One host at a time
- Stored logs are tied to the host or swarm a connection reaches. If a connection is pointed at another host, its old lines are deleted instead of showing up next to the new ones
- Views start fresh when you switch connections, and the overview names the host its errors come from

### Alerts
- Optional notification when a connection logs too many errors: pick 5, 10 or 15 minutes and a threshold under Preferences
- One notification per crossing; the overview shows a banner while the rate stays high
- Native notifications in the desktop app, browser notifications in `swarmdash serve`

## 0.2.1

- Apply the Stackline identity to the app header, browser favicon, and desktop icons.
- Add a vector logo with light and dark variants to the README.
- Include editable logo masters and a script for regenerating the branding assets.

## 0.2.0

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
