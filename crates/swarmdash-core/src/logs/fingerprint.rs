//! Groups similar errors: the same exception from the same place, or the same
//! message with different ids, numbers, addresses or timestamps.

use xxhash_rust::xxh3::xxh3_64;

/// Removes a leading timestamp (ISO 8601, nginx, syslog, Postgres) from a line.
pub fn strip_timestamp(line: &str) -> &str {
    let re = regex!(
        r"^\s*(?:\[?\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2}|\s(?:UTC|GMT|[A-Z]{1,3}T)\b)?\]?|\[\d{2}/\w{3}/\d{4}:\d{2}:\d{2}:\d{2} [+-]\d{4}\]|\w{3} [ \d]\d \d{2}:\d{2}:\d{2}|\[?\d{2}:\d{2}:\d{2}(?:[.,]\d+)?\]?)\s*"
    );
    match re.find(line) {
        Some(m) => &line[m.end()..],
        None => line,
    }
}

/// Removes what log frameworks put before the message: a timestamp, the
/// level, a process id and Spring Boot's `--- [thread]` part.
pub fn strip_prefix(line: &str) -> &str {
    let rest = strip_timestamp(line);
    let re = regex!(
        r"^(?:\[\d+\]\s+)?(?:\[(?i:trace|debug|info|notice|warn|warning|error|err|fatal|crit|critical|alert|emerg|panic)\]:?\s+|(?:TRACE|DEBUG|INFO|NOTICE|WARN|WARNING|ERROR|ERR|FATAL|CRITICAL|PANIC|LOG):?\s+)?(?:\d+#\d+:\s+(?:\*\d+\s+)?)?(?:\d+\s+---\s+(?:\[[^\]]*\]\s*)+)?"
    );
    match re.find(rest) {
        Some(m) if m.end() < rest.len() => &rest[m.end()..],
        _ => rest,
    }
}

/// Replaces the parts of a message that change between occurrences.
pub fn normalize(line: &str) -> String {
    let s = normalize_full(line);
    s[..cut_at(&s)].to_string()
}

/// Patterns are kept to this many bytes: enough to tell groups apart.
const PATTERN_MAX: usize = 300;

fn cut_at(s: &str) -> usize {
    super::level::floor_char_boundary(s, PATTERN_MAX)
}

fn normalize_full(line: &str) -> String {
    let mut s = strip_prefix(line).to_string();
    s = regex!(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").replace_all(&s, "<uuid>").into_owned();
    s = regex!(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+").replace_all(&s, "<email>").into_owned();
    s = regex!(r"\b\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?\b").replace_all(&s, "<ip>").into_owned();
    // Hex ids: at least 8 hex characters with both a digit and a letter.
    s = regex!(r"\b(?:0x)?[0-9a-fA-F]{8,}\b")
        .replace_all(&s, |c: &regex::Captures| {
            let m = &c[0];
            let body = m.trim_start_matches("0x");
            if body.bytes().any(|b| b.is_ascii_digit()) && body.bytes().any(|b| b.is_ascii_alphabetic()) {
                "<hex>".to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned();
    s = regex!(r#""[^"\s]*\d[^"\s]*""#).replace_all(&s, "\"<str>\"").into_owned();
    s = regex!(r"'[^'\s]*\d[^'\s]*'").replace_all(&s, "'<str>'").into_owned();
    s = regex!(r"\d+(?:\.\d+)?").replace_all(&s, "<n>").into_owned();
    s = regex!(r"\s+").replace_all(&s, " ").into_owned();
    s.trim().to_string()
}

/// A stack frame without its line number, so a redeploy keeps the same group.
fn frame_key(line: &str) -> Option<String> {
    let t = line.trim();
    let is_frame = t.starts_with("at ") || t.starts_with("File \"") || regex!(r"^\w[\w.$<>]*\(.*\)$").is_match(t);
    if !is_frame {
        return None;
    }
    Some(regex!(r":\d+(?::\d+)?|line \d+|:line \d+").replace_all(t, "").into_owned())
}

/// The exception class at the head of a trace, e.g. `java.lang.IllegalStateException`.
pub fn exception_type(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if let Some(c) = regex!(
            r"^(?:Caused by: |Exception in thread .*? )?((?:[a-zA-Z_$][\w$]*\.)*[A-Z][\w$]*(?:Exception|Error|Throwable|Fault|Failure))\b"
        )
        .captures(t)
        {
            return Some(c[1].to_string());
        }
    }
    None
}

pub struct Fingerprint {
    pub id: String,
    /// Normalised text shown as the group's title, with `<n>`-style placeholders.
    pub pattern: String,
}

/// `head` is the message line shown to people; `trace` the stack, if any.
pub fn fingerprint(service: &str, head: &str, trace: Option<&str>, error_type: Option<&str>) -> Fingerprint {
    let full = normalize_full(head);
    let pattern = &full[..cut_at(&full)];
    let mut key = format!("{service}\n");
    match trace {
        Some(tr) => {
            let ty = error_type.map(str::to_string).or_else(|| exception_type(tr)).unwrap_or_default();
            key.push_str(&ty);
            for f in tr.lines().filter_map(frame_key).take(3) {
                key.push('\n');
                key.push_str(&f);
            }
            if ty.is_empty() {
                key.push('\n');
                key.push_str(pattern);
            }
        }
        None => key.push_str(pattern),
    }
    // A cut title says so, instead of ending in the middle of a word.
    let pattern = if pattern.len() < full.len() { format!("{pattern}…") } else { pattern.to_string() };
    Fingerprint { id: format!("{:016x}", xxh3_64(key.as_bytes())), pattern }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_variable_parts() {
        assert_eq!(
            normalize("2026-10-27 20:15:20.713 UTC [77] ERROR:  duplicate key value violates unique constraint \"user_email_key\""),
            "duplicate key value violates unique constraint \"user_email_key\""
        );
        assert_eq!(
            normalize("Read timed out calling payments (order 973538) from 10.0.1.12:8080 id=3f2a9c1e-5b7d-4e8a-9c21-7d4e5f6a8b90"),
            "Read timed out calling payments (order <n>) from <ip> id=<uuid>"
        );
        assert_eq!(normalize("container 4f9a2c7d1e33 died"), "container <hex> died");
        assert_eq!(normalize("Error 111 connecting to redis:6379"), "Error <n> connecting to redis:<n>");
    }

    #[test]
    fn drops_framework_prefix() {
        assert_eq!(normalize("2026-10-27 20:34:21.000 ERROR 1 --- [exec-1] c.a.Checkout : failed"), "c.a.Checkout : failed");
        assert_eq!(
            normalize("2026-10-27T20:34:21.000Z ERROR 1 --- [shop] [nio-8080-exec-1] c.a.Checkout : failed for 42"),
            "c.a.Checkout : failed for <n>"
        );
        assert_eq!(normalize("[ERROR] Payment declined"), "Payment declined");
        assert_eq!(
            normalize("2026/10/27 20:15:20 [error] 29#29: *17 connect() failed (111: Connection refused)"),
            "connect() failed (<n>: Connection refused)"
        );
        assert_eq!(normalize("log rotated"), "log rotated");
        assert_eq!(normalize("ERROR"), "ERROR");
    }

    #[test]
    fn a_long_title_is_cut_with_an_ellipsis() {
        let long = format!("job import gave up: {}", "upstream timed out, ".repeat(30));
        let f = fingerprint("worker", &long, None, None);
        assert!(f.pattern.ends_with('…'), "{}", f.pattern);
        assert!(f.pattern.len() <= PATTERN_MAX + '…'.len_utf8());
        // The group stays the one older versions made from the cut text.
        let key = format!("worker\n{}", normalize(&long));
        assert_eq!(f.id, format!("{:016x}", xxh3_64(key.as_bytes())));
        assert!(!fingerprint("worker", "job import gave up", None, None).pattern.ends_with('…'));
    }

    #[test]
    fn same_trace_different_lines_same_group() {
        let a = "java.lang.NullPointerException: x\n\tat a.B.c(B.java:88)\n\tat a.B.d(B.java:41)";
        let b = "java.lang.NullPointerException: y\n\tat a.B.c(B.java:90)\n\tat a.B.d(B.java:41)";
        let fa = fingerprint("app", "NPE", Some(a), None);
        let fb = fingerprint("app", "NPE", Some(b), None);
        assert_eq!(fa.id, fb.id);
        assert_ne!(fa.id, fingerprint("web", "NPE", Some(a), None).id);
        assert_eq!(exception_type(a).as_deref(), Some("java.lang.NullPointerException"));
    }
}
