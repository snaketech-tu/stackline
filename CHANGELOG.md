# Changelog

Release notes for each version. The section of a version becomes the text of
its GitHub release, and swarmdash shows it in the update dialog.

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
