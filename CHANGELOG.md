# Changelog

Release notes for each version. The section of a version becomes the text of
its GitHub release, and swarmdash shows it in the update dialog.

## 0.3.2

The local store shrinks again when lines are deleted, a smoother log stream, and stacks no longer show as failing because of services you stopped.

### Smoother log stream
- Log rows are much lighter: the hover buttons are only drawn for rows you point at, which cuts the page from about 91,000 to 33,000 elements at 5,000 lines. **Show more** is about twice as fast, and scrolling through 1,300 lines stays at full frame rate
- A very long single line is cut short in the collapsed row (the full text shows when you open it), and an open entry with hundreds of lines shows the first 200 with a **Show all** button
- The Logs tab of a service starts with 300 lines, like the Logs page, and says how many more **Show more** adds

### Fixed
- After **Clear stored logs**, swarmdash now loads the logs Docker still has again, for the whole retention. Before, it only collected lines written after the clear, so longer ranges such as 3h or 24h stayed empty in the graph and the stream
- Deleted log lines now give their space back to the disk. Before, the store file kept its size after deleting a service's lines, after clearing all stored logs and when old lines expired, so the size in Preferences never went down. A store that grew this way shrinks on its own within a minute of starting 0.3.2
- The size of the local store in Preferences now updates while the page is open
- A container stopped with `docker stop` or `docker compose stop`, or created but never started, counts as stopped instead of failing, so its stack no longer shows as failing. A container that crashed still does
- A container killed for running out of memory still counts as failing and says so, although it ends with the same exit code (137) as one that was stopped
- A stack with an unhealthy container shows as degraded on the Overview too, as it already did on the Stacks page, and the service shows "unhealthy"

### Smaller changes
- Stacks show how many of their services are stopped next to their status, on the Overview and the Stacks page
- CPU usage shows on the first refresh instead of the second
- If removing old lines fails (for example because the disk is full), the error is logged and Preferences warns that the store can grow past its limit

## 0.3.1

Much less work for your server, a faster Logs page and faster log deletion.

### Lighter on the server
- Reading new log lines no longer makes Docker read each container's whole log file on every poll. On a container with a 330 MB log one poll cost the server about 5 seconds of CPU, every 3 seconds; now it reads only the end of the file
- CPU and memory per container take about 10 ms to read instead of a full second each

### Faster Logs page
- The time graph and level counts come from per-minute totals instead of reading every line: on a store with 3 million lines a 7-day view takes about 0.2 s instead of 0.7 s
- Picking a service or stack is fast even when the service logs rarely (a quiet service over 7 days: 0.07 s instead of 1.5 s), which also speeds up the Logs tab of the service panel
- Searches, downloads and the views no longer wait for each other or for new lines being stored: a large download used to hold up everything else for as long as it ran
- Downloads are written out while they are read, instead of being built in memory first
- In Live mode with thousands of lines shown, refreshing no longer freezes the page for a moment, and the page uses less memory

### Faster log deletion
- **Clear stored logs** empties the store at once instead of removing the lines one by one
- Deleting the stored lines of one service runs in the background, so the app stays usable, and Preferences shows "Deleting…" until it is done
- Deleting one service no longer reads the whole store again for every batch of lines, which made large deletes much slower than they had to be
- When a connection reaches another host, its old lines are cleared the same faster way

### Fixed
- Lines of a stack trace written within the same microsecond could be dropped
- A service that keeps printing indented lines could grow a single log entry without end

### Good to know
- The first start after the update builds one more index in the background, which can take a minute on a large store; the views work meanwhile. CPU usage shows up after the second refresh

## 0.3.0

More room for logs, more about each service, alerts, and a store that keeps to its size limit.

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

### Faster
- The Logs page reads its time graph, level counts and error groups in one pass over two new indexes: on a store with 1.5 million lines a 7-day view takes about 0.4 s instead of 2 s
- Deleting old lines no longer blocks the app while it runs, and repeated error patterns are no longer rewritten on every new line
- The service panel stops refetching on every refresh, big log pages refresh every 15 s, and hidden tabs stop polling
- The first start after the update builds the new indexes once, which can take a moment on a large store

### Fixed
- The connection menu and other dropdowns are no longer drawn under the overview cards
- The Errors tab of a service shows long messages in full at a readable size, and large counts are no longer cut off

### Disk space and log collection
- The store size limit is enforced: swarmdash checks it every minute instead of every hour, deletes the oldest lines down to about 90% of the limit, and gives the space back to the disk. Before, deleted lines left the file at its largest size
- Stores made by earlier versions are rebuilt once on the first start, so they can shrink. On a large store this takes a while and needs free disk space about the size of the store
- The cleanup works in small steps, so searches and new lines are not held up while it runs
- Choose whose logs are collected: **Collect** switches per service and per stack on the Stacks page, and **Only chosen** to collect nothing but the services you switch on. A service switched on again starts from that moment
- Preferences → Local store shows the space each service takes, with a switch to stop collecting it and a button to delete its lines
- The service panel says when its logs are not collected

### Filters stay put
- The services picked on the Logs page are kept per connection when you move around the app and after a restart. Opening the Logs page from the overview no longer clears them
- The Stacks page keeps its filter and search text

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
