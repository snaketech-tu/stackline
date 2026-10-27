//! swarmdash core: a read-only view of Docker hosts, swarms and their logs.

/// Compiles a regular expression once and caches it.
macro_rules! regex {
    ($re:literal $(,)?) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        // Compiled on first use only, so calling this in a loop is cheap.
        #[allow(clippy::regex_creation_in_loops)]
        RE.get_or_init(|| regex::Regex::new($re).unwrap())
    }};
}

pub mod collector;
pub mod config;
pub mod docker;
pub mod logs;
pub mod model;
pub mod runtime;
pub mod snapshot;
pub mod store;
