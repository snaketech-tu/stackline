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
            ("there was no error here", None),
        ];
        for (line, want) in cases {
            assert_eq!(detect(line), want, "{line}");
        }
    }
}
