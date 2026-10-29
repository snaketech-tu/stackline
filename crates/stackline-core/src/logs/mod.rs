//! Log entries: parsing, multi-line assembly and error fingerprints.

pub mod assemble;
pub mod fingerprint;
pub mod json;
pub mod level;

use crate::docker::frames::StreamKind;
pub use level::Level;

#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// Unix time in microseconds, from Docker's own timestamp for the first line.
    pub ts: i64,
    pub stream: StreamKind,
    pub level: Level,
    /// The full entry; multi-line entries keep their line breaks.
    pub text: String,
    /// The original JSON line for structured logs.
    pub raw: Option<String>,
    pub lines: u32,
    /// Carries a stack trace.
    pub exc: bool,
    /// Error group id for warnings and errors.
    pub fp: Option<String>,
    pub pattern: Option<String>,
}

/// Splits Docker's `timestamps=1` prefix (`2026-10-27T20:15:20.713238298Z `) from a line.
/// Returns the time in microseconds.
pub fn split_docker_timestamp(line: &str) -> Option<(i64, &str)> {
    let sp = line.find(' ').unwrap_or(line.len());
    let ts = chrono::DateTime::parse_from_rfc3339(&line[..sp]).ok()?;
    let rest = if sp < line.len() { &line[sp + 1..] } else { "" };
    Some((ts.timestamp_micros(), rest))
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_timestamp() {
        let (ts, rest) = super::split_docker_timestamp("2026-10-27T20:15:20.713238298Z hello world").unwrap();
        assert_eq!(rest, "hello world");
        assert_eq!(ts % 1_000_000, 713238);
        assert!(super::split_docker_timestamp("no timestamp").is_none());
    }
}
