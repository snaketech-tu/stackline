//! Log levels and how to recognise them in plain text.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Level {
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl Level {
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    pub fn from_i64(v: i64) -> Level {
        match v {
            1 => Level::Debug,
            3 => Level::Warn,
            4 => Level::Error,
            _ => Level::Info,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }

    pub fn parse_name(s: &str) -> Option<Level> {
        match s.to_ascii_uppercase().as_str() {
            "FATAL" | "PANIC" | "EMERG" | "EMERGENCY" | "ALERT" | "CRITICAL" | "CRIT" | "SEVERE" | "ERROR" | "ERR" | "E" => {
                Some(Level::Error)
            }
            "WARNING" | "WARN" | "W" => Some(Level::Warn),
            "NOTICE" | "INFO" | "INFORMATION" | "LOG" | "NOTE" | "I" => Some(Level::Info),
            "DEBUG" | "TRACE" | "FINE" | "FINER" | "FINEST" | "VERBOSE" | "D" => Some(Level::Debug),
            _ => None,
        }
    }

    /// Pino/Bunyan numeric levels.
    pub fn from_number(n: i64) -> Level {
        match n {
            ..=20 => Level::Debug,
            21..=30 => Level::Info,
            31..=40 => Level::Warn,
            _ => Level::Error,
        }
    }
}

/// Finds the level of a plain-text log line, looking only at its first 200 bytes.
pub fn detect(line: &str) -> Option<Level> {
    let head = &line[..floor_char_boundary(line, 200)];
    // Access logs (nginx, Apache, Traefik): the status code says it. Checked
    // first, so a path like `/errors` never sets the level.
    if let Some(c) = regex!(r#"^\S+ \S+ \S+ \[[^\]]+\] "[A-Z]+ [^"]*" (\d{3}) "#).captures(head) {
        return Some(if c[1].starts_with('5') { Level::Error } else { Level::Info });
    }
    // Go and Rust panics.
    if regex!(r"^(?:panic: |fatal error: |thread '[^']*' panicked at )").is_match(head) {
        return Some(Level::Error);
    }
    // PHP: `PHP Fatal error:  Uncaught …`, `PHP Warning:  …`
    if let Some(c) = regex!(
        r"\bPHP (Fatal error|Parse error|Catchable fatal error|Recoverable fatal error|Warning|Notice|Deprecated|Strict Standards)\b"
    )
    .captures(head)
    {
        return Some(match &c[1] {
            "Warning" => Level::Warn,
            "Notice" | "Deprecated" | "Strict Standards" => Level::Info,
            _ => Level::Error,
        });
    }
    // Redis: `1:M 29 Oct 2026 17:00:00.123 # Warning…`, the mark after the time is the level.
    if let Some(c) = regex!(r"^\d+:[XCSM] \d{1,2} \w{3} \d{4} \d{2}:\d{2}:\d{2}\.\d{3} ([.\-*#]) ").captures(head) {
        return Some(match &c[1] {
            "#" => Level::Warn,
            "*" => Level::Info,
            _ => Level::Debug,
        });
    }
    // MongoDB before 4.4: `2026-10-29T17:00:00.000+0000 E NETWORK  [conn1] …`
    if let Some(c) = regex!(r"^\d{4}-\d{2}-\d{2}T[\d:.]+[+-]\d{4} ([FEWID])\d? +[A-Z]+ +\[").captures(head) {
        return Some(mongo_severity(&c[1]));
    }
    // zerolog console (Traefik 3): `2026-10-29T17:00:00Z ERR …`
    if let Some(c) = regex!(r"^\S+ (TRC|DBG|INF|WRN|ERR|FTL|PNC) ").captures(head) {
        return Some(match &c[1] {
            "ERR" | "FTL" | "PNC" => Level::Error,
            "WRN" => Level::Warn,
            "INF" => Level::Info,
            _ => Level::Debug,
        });
    }
    // Microsoft.Extensions.Logging console format: `warn: Category[0]`
    if let Some(c) = regex!(r"^(trce|dbug|info|warn|fail|crit):\s").captures(head) {
        return Some(match &c[1] {
            "fail" | "crit" => Level::Error,
            "warn" => Level::Warn,
            "info" => Level::Info,
            _ => Level::Debug,
        });
    }
    // level=error, "level":"warn"
    if let Some(c) = regex!(r#"(?i)\b(?:level|lvl|severity)"?\s*[=:]\s*"?([a-z]+)"#).captures(head)
        && let Some(l) = Level::parse_name(&c[1])
    {
        return Some(l);
    }
    // [error], [Warning], [Note], [ERROR] (nginx, MariaDB, many others)
    for c in regex!(r"\[([A-Za-z]{4,9})\]").captures_iter(head) {
        if let Some(l) = Level::parse_name(&c[1]) {
            return Some(l);
        }
    }
    // PostgreSQL: `LOG:`, `ERROR:`, `FATAL:`; Python: `ERROR:root:`
    if let Some(c) = regex!(r"(?:^|\s)(LOG|ERROR|FATAL|PANIC|WARNING|NOTICE|INFO|DEBUG\d?|CRITICAL):").captures(head) {
        return Level::parse_name(c[1].trim_end_matches(char::is_numeric));
    }
    // Upper-case words, e.g. Spring Boot `  INFO 1 --- [main]`, logback, Serilog `[INF]`.
    if let Some(c) =
        regex!(r"(?:^|[\s|:(\[<])(FATAL|PANIC|CRITICAL|CRIT|SEVERE|ERROR|WARNING|WARN|NOTICE|INFO|DEBUG|TRACE)(?:$|[\s|:)\]>,])")
            .captures(head)
    {
        return Level::parse_name(&c[1]);
    }
    if let Some(c) = regex!(r"[\[\s](FTL|ERR|WRN|INF|DBG|VRB)\]").captures(head) {
        return Some(match &c[1] {
            "FTL" | "ERR" => Level::Error,
            "WRN" => Level::Warn,
            "INF" => Level::Info,
            _ => Level::Debug,
        });
    }
    None
}

/// MongoDB's severity letters: F, E, W, I and D1 to D5.
pub fn mongo_severity(s: &str) -> Level {
    match s.chars().next() {
        Some('F' | 'E') => Level::Error,
        Some('W') => Level::Warn,
        Some('D') => Level::Debug,
        _ => Level::Info,
    }
}

pub fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_formats() {
        let cases = [
            ("2026-10-27T20:15:20.713Z  INFO 1 --- [main] o.s.b.StartupInfoLogger : Started", Some(Level::Info)),
            ("2026-10-27 20:15:20.713 ERROR 1 --- [nio-8080-exec-1] o.a.c.c.C.[.[.[/] : Servlet failed", Some(Level::Error)),
            ("2026-10-27 20:00:00.000 UTC [1] LOG:  database system is ready", Some(Level::Info)),
            ("2026-10-27 20:00:00.000 UTC [77] ERROR:  duplicate key value", Some(Level::Error)),
            ("2026/10/27 20:15:20 [error] 31#31: *4 upstream timed out", Some(Level::Error)),
            ("2026-10-27 20:15:20 0 [Warning] Access denied for user", Some(Level::Warn)),
            ("2026-10-27 20:15:20 0 [Note] InnoDB: Buffer pool load completed", Some(Level::Info)),
            ("time=2026-10-27T20:15:20Z level=warning msg=\"slow\"", Some(Level::Warn)),
            ("ERROR:root:something broke", Some(Level::Error)),
            ("[20:15:20 WRN] Retrying", Some(Level::Warn)),
            ("[20:15:20 INF] Request finished", Some(Level::Info)),
            ("warn: Microsoft.AspNetCore.Server.Kestrel[0]", Some(Level::Warn)),
            ("GET /api/cart 200 34ms", None),
            // Access logs: nginx, Traefik (CLF), Apache.
            (r#"10.0.0.1 - - [29/Oct/2026:17:00:00 +0000] "GET /errors HTTP/1.1" 200 512 "-" "curl/8""#, Some(Level::Info)),
            (
                r#"10.0.0.1 - - [29/Oct/2026:17:00:00 +0000] "POST /api/cart HTTP/1.1" 502 11 "-" "-" 42 "shop@docker" "http://10.0.1.5:80" 3ms"#,
                Some(Level::Error),
            ),
            ("panic: runtime error: invalid memory address or nil pointer dereference", Some(Level::Error)),
            ("fatal error: all goroutines are asleep - deadlock!", Some(Level::Error)),
            ("thread 'main' panicked at src/main.rs:5:5:", Some(Level::Error)),
            ("[29-Oct-2026 17:00:00 UTC] PHP Fatal error:  Uncaught Exception: boom in /var/www/html/index.php:3", Some(Level::Error)),
            ("[29-Oct-2026 17:00:00 UTC] PHP Warning:  Undefined variable $cart in /var/www/html/cart.php on line 12", Some(Level::Warn)),
            ("[29-Oct-2026 17:00:00 UTC] PHP Deprecated:  strlen(): Passing null is deprecated", Some(Level::Info)),
            ("[29-Oct-2026 17:00:00] WARNING: [pool www] server reached pm.max_children setting (5)", Some(Level::Warn)),
            ("E, [2026-10-29T17:00:00.123456 #1] ERROR -- : Redis::CannotConnectError", Some(Level::Error)),
            ("1:M 29 Oct 2026 17:00:00.123 # WARNING Memory overcommit must be enabled!", Some(Level::Warn)),
            ("1:M 29 Oct 2026 17:00:00.123 * Ready to accept connections tcp", Some(Level::Info)),
            ("2026-10-29T17:00:00.000+0000 E NETWORK  [conn12] SocketException: Broken pipe", Some(Level::Error)),
            ("2026-10-29T17:00:00Z ERR error=\"service not found\" entryPointName=web routerName=shop@docker", Some(Level::Error)),
            ("2026-10-29T17:00:00Z WRN Some warning", Some(Level::Warn)),
            ("there was no error here", None),
        ];
        for (line, want) in cases {
            assert_eq!(detect(line), want, "{line}");
        }
    }
}
