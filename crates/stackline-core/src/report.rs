//! A debug report of one service as Markdown, to paste into a ticket or a
//! chat: what runs, how it is configured, what it logged, when it crashed and
//! what it printed just before. Built only from what Stackline already read.
//! Environment values are left out, since they often hold secrets.

use std::fmt::Write;

use chrono::{DateTime, Utc};

use crate::details::ServiceDetails;
use crate::events::{Event, Kind};
use crate::model::{Host, Service};
use crate::store::{EntryOut, Group, LevelCounts, Quiet};

/// How many lines of an error sample or a log entry are kept.
const MAX_LINES: usize = 15;
/// How many error groups are listed.
const MAX_GROUPS: usize = 5;
/// How many crashes and stops are listed.
const MAX_EVENTS: usize = 12;

pub struct Report<'a> {
    pub connection: &'a str,
    pub host: &'a Host,
    pub service: &'a Service,
    /// None when the configuration could not be read.
    pub details: Option<&'a ServiceDetails>,
    /// Lines of the last 24 hours, by level.
    pub total: u64,
    pub levels: &'a LevelCounts,
    /// Error and warning groups of the last 24 hours, most frequent first.
    pub groups: &'a [Group],
    /// Starts, stops and crashes of the last 24 hours, oldest first.
    pub events: &'a [Event],
    /// What the replica of the newest crash printed just before, oldest first.
    pub before_crash: &'a [EntryOut],
    pub quiet: Option<&'a Quiet>,
    /// Unix ms.
    pub now: i64,
    pub version: &'a str,
}

pub fn markdown(r: &Report) -> String {
    let mut o = String::new();
    let s = r.service;
    let _ = writeln!(o, "# Debug report: {}\n", s.full_name);
    let _ = writeln!(
        o,
        "Connection **{}** · host `{}` · {} {} · {} · {} by Stackline {}\n",
        r.connection,
        r.host.name,
        r.host.engine,
        r.host.docker_version,
        if r.host.swarm_state == "active" { "swarm" } else { "no swarm" },
        time_ms(r.now),
        r.version
    );

    o.push_str("## Service\n\n| | |\n| --- | --- |\n");
    let image = r.details.map(|d| d.image.as_str()).unwrap_or(&s.image);
    row(&mut o, "Image", &code(image));
    row(&mut o, "Replicas", &format!("{} of {} running ({})", s.running, s.desired, s.mode));
    if !s.ports.is_empty() {
        row(&mut o, "Ports", &s.ports.iter().map(|p| code(p)).collect::<Vec<_>>().join(", "));
    }
    if let Some(t) = s.updated_at {
        row(&mut o, "Last update", &time_ms(t));
    }
    if let Some(d) = r.details {
        let opt = |o: &mut String, label: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
                row(o, label, &code(v));
            }
        };
        opt(&mut o, "Command", &d.command);
        opt(&mut o, "Entrypoint", &d.entrypoint);
        opt(&mut o, "Restart policy", &d.restart);
        if let Some(n) = d.restart_count.filter(|n| *n > 0) {
            row(&mut o, "Restarts", &n.to_string());
        }
        if d.oom_killed {
            row(&mut o, "Out of memory", "the last run was killed at its memory limit");
        }
        opt(&mut o, "Limits", &d.limits);
        opt(&mut o, "Reservations", &d.reservations);
        opt(&mut o, "Health check", &d.healthcheck);
        opt(&mut o, "Update config", &d.update_config);
        if !d.placement.is_empty() {
            row(&mut o, "Placement", &d.placement.iter().map(|p| code(p)).collect::<Vec<_>>().join(", "));
        }
        if !d.mounts.is_empty() {
            let m: Vec<String> =
                d.mounts.iter().map(|m| code(&format!("{}:{}{}", m.source, m.target, if m.read_only { ":ro" } else { "" }))).collect();
            row(&mut o, "Mounts", &m.join(", "));
        }
        if !d.networks.is_empty() {
            row(&mut o, "Networks", &d.networks.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", "));
        }
        if !d.env.is_empty() {
            row(&mut o, "Environment", &format!("{} (values not included)", plural(d.env.len() as u64, "variable", "variables")));
        }
        opt(&mut o, "Log driver", &d.log_driver);
    } else {
        row(&mut o, "Configuration", "could not be read");
    }

    if !s.tasks.is_empty() {
        o.push_str("\n## Replicas\n\n");
        for t in &s.tasks {
            let mut line = format!("- **{}** on `{}`: {}", t.slot, t.node, t.state);
            if let Some(h) = &t.health {
                let _ = write!(line, ", {h}");
            }
            if let Some(m) = t.message.as_deref().filter(|m| !m.is_empty()) {
                let _ = write!(line, " · {}", one_line(m));
            }
            if let Some(e) = t.error.as_deref().filter(|e| !e.is_empty()) {
                let _ = write!(line, " · error: {}", one_line(e));
            }
            if let Some(c) = t.exit_code.filter(|c| *c != 0) {
                let _ = write!(line, " · exit {c}");
            }
            if t.failures > 0 {
                let _ = write!(line, " · {} earlier failed {}", t.failures, if t.failures == 1 { "task" } else { "tasks" });
            }
            o.push_str(&line);
            o.push('\n');
        }
    }

    let l = r.levels;
    let _ = writeln!(
        o,
        "\n## Logs of the last 24 hours\n\n{}: {}, {}, {} info, {} debug.",
        plural(r.total, "entry", "entries"),
        plural(l.error, "error", "errors"),
        plural(l.warn, "warning", "warnings"),
        l.info,
        l.debug
    );
    if let Some(q) = r.quiet {
        let _ = writeln!(
            o,
            "\n**Went quiet:** no lines since {}, after lines in {} of the 120 minutes before, while it still runs.",
            time_us(q.since),
            q.busy_minutes
        );
    }

    if !r.groups.is_empty() {
        o.push_str("\n## Most frequent errors and warnings\n");
        for (i, g) in r.groups.iter().take(MAX_GROUPS).enumerate() {
            let what = if g.pattern.is_empty() { first_line(&g.sample) } else { g.pattern.as_str() };
            let _ = writeln!(
                o,
                "\n{}. **{}×** {} {}, first {}, last {}",
                i + 1,
                g.count,
                g.level.name(),
                code(&cut(what, 200)),
                time_us(g.first),
                time_us(g.last)
            );
            o.push_str(&block(&g.sample, "   "));
        }
    }

    let crashes: Vec<&Event> = r.events.iter().filter(|e| e.kind != Kind::Start).collect();
    if !crashes.is_empty() {
        o.push_str("\n## Stops and crashes\n\n");
        let skip = crashes.len().saturating_sub(MAX_EVENTS);
        if skip > 0 {
            let _ = writeln!(o, "{skip} earlier ones left out.\n");
        }
        for e in &crashes[skip..] {
            let mut line = format!("- {} replica {}: {}", time_us(e.ts), e.replica, event_label(e));
            if let Some(d) = e.detail.as_deref().filter(|d| !d.is_empty() && e.kind != Kind::Failed) {
                let _ = write!(line, " · {}", one_line(d));
            }
            o.push_str(&line);
            o.push('\n');
        }
    }

    if let Some(crash) = r.events.iter().rev().find(|e| matches!(e.kind, Kind::Crash | Kind::Oom | Kind::Failed)) {
        let _ =
            writeln!(o, "\n## Lines before the last crash\n\nReplica {}, {}, {}.", crash.replica, event_label(crash), time_us(crash.ts));
        if r.before_crash.is_empty() {
            o.push_str("\nNo lines were stored from the 2 minutes before it.\n");
        } else {
            let text: Vec<String> = r
                .before_crash
                .iter()
                .map(|e| {
                    let t = DateTime::<Utc>::from_timestamp_micros(e.ts).map(|d| d.format("%H:%M:%S%.3f").to_string()).unwrap_or_default();
                    format!("{t} {:<5} {}", e.level.name(), keep_lines(&e.text, MAX_LINES))
                })
                .collect();
            o.push('\n');
            o.push_str(&block(&text.join("\n"), ""));
        }
    }
    o
}

fn row(o: &mut String, label: &str, value: &str) {
    let _ = writeln!(o, "| {label} | {} |", value.replace('|', "\\|").replace('\n', " "));
}

/// Inline code that survives backticks in the value.
fn code(s: &str) -> String {
    let s = one_line(s);
    let ticks = "`".repeat(longest_run(&s, '`') + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    format!("{ticks}{pad}{s}{pad}{ticks}")
}

/// A fenced block that survives fences in the text, indented for a list item.
fn block(text: &str, indent: &str) -> String {
    let text = keep_lines(text.trim_end(), MAX_LINES * 4);
    let fence = "`".repeat(longest_run(&text, '`').max(2) + 1);
    let mut o = format!("\n{indent}{fence}\n");
    for l in text.lines() {
        let _ = writeln!(o, "{indent}{l}");
    }
    let _ = writeln!(o, "{indent}{fence}");
    o
}

fn longest_run(s: &str, c: char) -> usize {
    s.split(|x| x != c).map(str::len).max().unwrap_or(0)
}

fn keep_lines(s: &str, max: usize) -> String {
    let n = s.lines().count();
    if n <= max {
        return s.to_string();
    }
    let mut out: Vec<&str> = s.lines().take(max).collect();
    let more = format!("… {} more lines", n - max);
    out.push(&more);
    out.join("\n")
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    format!("{}…", &s[..crate::logs::level::floor_char_boundary(s, max)])
}

fn event_label(e: &Event) -> String {
    let code = e.exit_code.map(|c| format!(", exit {c}")).unwrap_or_default();
    match e.kind {
        Kind::Start => "started".into(),
        Kind::Exit => format!("exited{code}"),
        Kind::Stop => format!("stopped{code}"),
        Kind::Crash => format!("crashed{code}"),
        Kind::Oom => "out of memory".into(),
        Kind::Unhealthy => "health check failed".into(),
        Kind::Failed => match e.detail.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => format!("failed: {}{code}", one_line(d)),
            None => format!("failed{code}"),
        },
    }
}

fn time_ms(t: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(t).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()
}

fn time_us(t: i64) -> String {
    DateTime::<Utc>::from_timestamp_micros(t).map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::Level;
    use crate::model::Task;

    fn host() -> Host {
        Host {
            id: "h".into(),
            cluster_id: None,
            name: "box".into(),
            os: "Linux".into(),
            kernel: "6".into(),
            engine: "Docker".into(),
            docker_version: "27.1".into(),
            api_version: "1.46".into(),
            ncpu: 4,
            mem_total: 0,
            containers_running: 1,
            containers_total: 1,
            images: 1,
            swarm_state: "inactive".into(),
            is_manager: false,
            cpu_percent: None,
            mem_used: None,
        }
    }

    #[test]
    fn a_report_names_what_matters_and_survives_backticks() {
        let svc = Service {
            id: "s".into(),
            name: "api".into(),
            full_name: "shop_api".into(),
            stack: "shop".into(),
            image: "shop/api:1.2".into(),
            mode: "compose".into(),
            desired: 1,
            running: 0,
            ports: vec!["8080:80/tcp".into()],
            updated_at: Some(1_761_750_000_000),
            tty: false,
            tasks: vec![Task {
                id: "t".into(),
                slot: "1".into(),
                node: "box".into(),
                state: "exited".into(),
                desired_state: "running".into(),
                message: Some("Exited (1) 2 minutes ago".into()),
                error: None,
                health: None,
                container_id: Some("abc".into()),
                container_name: None,
                since: None,
                exit_code: Some(1),
                failures: 0,
                cpu_percent: None,
                mem_used: None,
            }],
            cpu_percent: None,
            mem_used: None,
        };
        let crash = Event {
            key: "k".into(),
            ts: 1_761_750_100_000_000,
            stack: "shop".into(),
            service: "shop_api".into(),
            replica: "1".into(),
            node: "box".into(),
            source: "abc".into(),
            kind: Kind::Crash,
            exit_code: Some(1),
            detail: None,
        };
        let line = EntryOut {
            id: 1,
            ts: crash.ts - 1_000_000,
            stack: "shop".into(),
            service: "shop_api".into(),
            replica: "1".into(),
            node: "box".into(),
            source: "abc".into(),
            stream: "stderr",
            level: Level::Error,
            exc: true,
            lines: 2,
            fp: None,
            text: "Unhandled exception\n```weird```".into(),
            raw: None,
        };
        let levels = LevelCounts { error: 3, warn: 1, info: 10, debug: 0 };
        let quiet = Quiet { service: "shop_api".into(), since: crash.ts, busy_minutes: 118 };
        let h = host();
        let md = markdown(&Report {
            connection: "prod",
            host: &h,
            service: &svc,
            details: None,
            total: 14,
            levels: &levels,
            groups: &[],
            events: std::slice::from_ref(&crash),
            before_crash: std::slice::from_ref(&line),
            quiet: Some(&quiet),
            now: 1_761_750_200_000,
            version: "0.5.0",
        });
        assert!(md.starts_with("# Debug report: shop_api\n"), "{md}");
        assert!(md.contains("| Image | `shop/api:1.2` |"), "{md}");
        assert!(md.contains("0 of 1 running"), "{md}");
        assert!(md.contains("crashed, exit 1"), "{md}");
        assert!(md.contains("Went quiet"), "{md}");
        assert!(md.contains("14 entries: 3 errors, 1 warning"), "{md}");
        // The fence around the lines is longer than the backticks inside them.
        assert!(md.contains("\n````\n") && md.contains("ERROR Unhandled exception\n```weird```\n````"), "{md}");
        assert_eq!(code("a`b"), "``a`b``");
    }
}
