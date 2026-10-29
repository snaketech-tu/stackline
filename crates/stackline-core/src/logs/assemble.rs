//! Turns the lines of one replica into log entries, joining multi-line entries
//! (stack traces, PostgreSQL DETAIL/STATEMENT lines, indented continuations).
//!
//! Each replica has its own assembler, so lines from different replicas can
//! never be mixed into one entry.

use crate::docker::frames::StreamKind;

use super::LogEntry;
use super::fingerprint::{exception_type, fingerprint};
use super::json::{self, Structured};
use super::level::{self, Level};

/// Lines further apart than this never belong to the same entry (microseconds).
const MAX_GAP_US: i64 = 1_000_000;
/// An exception head directly after a message line belongs to it if written within this time.
const HEAD_GAP_US: i64 = 250_000;
/// An entry ends at this many lines or bytes, even if more lines seem to
/// belong to it: a service that keeps printing indented lines would
/// otherwise grow one entry without end.
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 512 * 1024;

struct Pending {
    first_ts: i64,
    last_ts: i64,
    stream: StreamKind,
    lines: Vec<String>,
    bytes: usize,
    has_stack: bool,
    python: bool,
    /// A Go or Rust panic: everything printed with it belongs to it.
    panic: bool,
}

pub struct Assembler {
    service: String,
    pending: Option<Pending>,
}

impl Assembler {
    pub fn new(service: &str) -> Self {
        Self { service: service.to_string(), pending: None }
    }

    /// Adds one line. `ts` is in microseconds. Finished entries are pushed to `out`.
    pub fn push(&mut self, ts: i64, stream: StreamKind, line: &str, out: &mut Vec<LogEntry>) {
        if let Some(s) = json::parse(line) {
            self.flush(out);
            out.push(self.structured_entry(ts, stream, line, s));
            return;
        }
        if let Some(p) = self.pending.as_mut()
            && p.lines.len() < MAX_LINES
            && p.bytes < MAX_BYTES
            && is_continuation(p, ts, stream, line)
        {
            if is_frame(line) || line.trim_start().starts_with("Traceback (most recent call last)") {
                p.has_stack = true;
            }
            if line.trim_start().starts_with("Traceback (most recent call last)") {
                p.python = true;
            }
            if !(line.trim().is_empty() && !p.has_stack) {
                p.bytes += line.len() + 1;
                p.lines.push(line.to_string());
            }
            p.last_ts = ts;
            return;
        }
        self.flush(out);
        if line.trim().is_empty() {
            return;
        }
        let python = line.starts_with("Traceback (most recent call last)");
        let panic = is_panic(line);
        self.pending = Some(Pending {
            first_ts: ts,
            last_ts: ts,
            stream,
            lines: vec![line.to_string()],
            bytes: line.len(),
            has_stack: python || panic,
            python,
            panic,
        });
    }

    /// Emits the pending entry if nothing was added to it for a while.
    pub fn flush_if_idle(&mut self, now_us: i64, out: &mut Vec<LogEntry>) {
        if self.pending.as_ref().is_some_and(|p| now_us - p.last_ts > 2 * MAX_GAP_US) {
            self.flush(out);
        }
    }

    pub fn flush(&mut self, out: &mut Vec<LogEntry>) {
        let Some(mut p) = self.pending.take() else { return };
        while p.lines.last().is_some_and(|l| l.trim().is_empty()) {
            p.lines.pop();
        }
        let text = p.lines.join("\n");
        let head = p.lines[0].clone();
        let exc_type = exception_type(&text);
        let mut lvl = level::detect(&head);
        if lvl.is_none() {
            lvl = Some(if p.has_stack || exc_type.is_some() { Level::Error } else { Level::Info });
        }
        let lvl = lvl.unwrap_or(Level::Info);
        let exc = p.has_stack;
        let (fp, pattern) = if lvl >= Level::Warn {
            // A Python traceback's message is its last line; elsewhere the first.
            let shown =
                if p.python { p.lines.iter().rev().find(|l| !l.starts_with(' ')).cloned().unwrap_or(head.clone()) } else { head.clone() };
            let f = fingerprint(&self.service, &shown, exc.then_some(text.as_str()), exc_type.as_deref());
            (Some(f.id), Some(f.pattern))
        } else {
            (None, None)
        };
        out.push(LogEntry { ts: p.first_ts, stream: p.stream, level: lvl, lines: p.lines.len() as u32, text, raw: None, exc, fp, pattern });
    }

    fn structured_entry(&self, ts: i64, stream: StreamKind, raw: &str, s: Structured) -> LogEntry {
        let exc = s.stack.is_some();
        let lvl = s.level.unwrap_or(if exc { Level::Error } else { Level::Info });
        let text = match &s.stack {
            Some(st) if !s.message.is_empty() && !st.starts_with(&s.message) => format!("{}\n{}", s.message, st.trim_end()),
            Some(st) => st.trim_end().to_string(),
            None => s.message.clone(),
        };
        let (fp, pattern) = if lvl >= Level::Warn {
            let f = fingerprint(&self.service, &s.message, s.stack.as_deref(), s.error_type.as_deref());
            (Some(f.id), Some(f.pattern))
        } else {
            (None, None)
        };
        LogEntry {
            ts,
            stream,
            level: lvl,
            lines: text.lines().count().max(1) as u32,
            text,
            raw: Some(raw.trim().to_string()),
            exc,
            fp,
            pattern,
        }
    }
}

fn is_frame(line: &str) -> bool {
    let t = line.trim_start();
    // Java, .NET, Node (`at`), Python (`File "`), Ruby (`from`).
    let indented = line.len() != t.len()
        && (t.starts_with("at ")
            || t.starts_with("File \"")
            || t.starts_with("from ")
            || t.starts_with("... ")
            || t.starts_with("--- End of"));
    // PHP: `Stack trace:`, then `#0 /var/www/…`.
    indented || is_php_trace(t)
}

fn is_php_trace(t: &str) -> bool {
    t.starts_with("Stack trace:") || t.starts_with("PHP Stack trace:") || regex!(r"^(?:PHP +\d+\. |#\d+ )").is_match(t)
}

/// The first line of a Go or Rust panic.
fn is_panic(line: &str) -> bool {
    regex!(r"^(?:panic: |fatal error: |thread '[^']*' panicked at )").is_match(line)
}

/// A line that starts a new log entry: it begins with a date or a JSON object.
fn starts_entry(line: &str) -> bool {
    line.starts_with('{') || regex!(r"^\[?\d{4}[-/]\d{2}[-/]\d{2}|^time=|^\w+=\S+ level=").is_match(line)
}

fn is_continuation(p: &Pending, ts: i64, stream: StreamKind, line: &str) -> bool {
    if stream != p.stream || ts - p.last_ts > MAX_GAP_US {
        return false;
    }
    let t = line.trim_start();
    if line.is_empty() {
        return p.has_stack;
    }
    if line.len() != t.len() {
        // Indented: stack frames, wrapped messages, .NET console logger bodies.
        return true;
    }
    // A Go panic prints goroutines with unindented function lines, a Rust one
    // its message and notes: everything up to the next dated line is part of it.
    if p.panic {
        return !starts_entry(line);
    }
    // PHP: the trace and the `thrown in` line after it.
    if is_php_trace(t) || (p.has_stack && t.starts_with("thrown in ")) {
        return true;
    }
    if t.starts_with("Caused by:")
        || t.starts_with("Suppressed:")
        || t.starts_with("... ")
        || t.starts_with("Traceback (most recent call last)")
        || t.starts_with("During handling of the above exception")
        || t.starts_with("The above exception was the direct cause")
        || t.starts_with("--- End of")
    {
        return true;
    }
    // PostgreSQL detail lines carry the same prefix as the line they belong to.
    if let Some(m) = regex!(r"\b(DETAIL|HINT|STATEMENT|CONTEXT|QUERY|LOCATION):\s").find(t)
        && m.start() <= 64
    {
        return true;
    }
    let is_head =
        regex!(r"^(?:(?:[a-zA-Z_$][\w$]*\.)+[A-Z][\w$]*(?:Exception|Error|Throwable|Fault)|[A-Z]\w*(?:Error|Exception))(?::|$|\s)")
            .is_match(t);
    if is_head {
        // The final "ValueError: …" line of a Python traceback.
        if p.python {
            return true;
        }
        // `log.error("…", e)` prints the message, then the exception on the next line.
        if p.lines.len() == 1 && ts - p.last_ts <= HEAD_GAP_US {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lines: &[(i64, &str)]) -> Vec<LogEntry> {
        let mut a = Assembler::new("svc");
        let mut out = vec![];
        for (ts, l) in lines {
            a.push(*ts, StreamKind::Stdout, l, &mut out);
        }
        a.flush(&mut out);
        out
    }

    #[test]
    fn java_logback_with_cause() {
        let out = run(&[
            (0, "2026-10-27 20:15:20.713 ERROR 1 --- [exec-1] c.a.CheckoutController : Checkout failed"),
            (10, "java.lang.IllegalStateException: cart is empty"),
            (10, "\tat com.acme.Checkout.total(Checkout.java:88)"),
            (11, "\tat com.acme.Controller.preview(Controller.java:41)"),
            (11, "Caused by: java.lang.NullPointerException: null"),
            (11, "\t... 47 common frames omitted"),
            (5_000_000, "2026-10-27 20:15:25.000  INFO 1 --- [exec-2] c.a.CheckoutController : OK"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].level, out[0].lines, out[0].exc), (Level::Error, 6, true));
        assert!(out[0].fp.is_some());
        assert_eq!(out[1].level, Level::Info);
        assert!(out[1].fp.is_none());
    }

    #[test]
    fn python_traceback_uses_last_line_as_title() {
        let out = run(&[
            (0, "Traceback (most recent call last):"),
            (1, "  File \"/app/worker/queue.py\", line 57, in poll"),
            (1, "    job = self.redis.blpop(self.queues, timeout=5)"),
            (2, "redis.exceptions.ConnectionError: Error 111 connecting to redis:6379. Connection refused."),
            (900_000_000, "INFO:worker:picked up 3 jobs"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].level, out[0].lines), (Level::Error, 4));
        assert_eq!(
            out[0].pattern.as_deref(),
            Some("redis.exceptions.ConnectionError: Error <n> connecting to redis:<n>. Connection refused.")
        );
    }

    #[test]
    fn postgres_detail_and_statement() {
        let out = run(&[
            (0, "2026-10-27 20:00:00.000 UTC [77] ERROR:  duplicate key value violates unique constraint \"users_email_key\""),
            (1, "2026-10-27 20:00:00.000 UTC [77] DETAIL:  Key (email)=(a@b.c) already exists."),
            (1, "2026-10-27 20:00:00.000 UTC [77] STATEMENT:  INSERT INTO users (email) VALUES ($1)"),
            (2, "2026-10-27 20:00:00.100 UTC [1] LOG:  checkpoint starting: time"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].level, out[0].lines, out[0].exc), (Level::Error, 3, false));
        assert_eq!(out[1].level, Level::Info);
    }

    #[test]
    fn node_dotnet_and_separate_entries() {
        let out = run(&[
            (0, "TypeError: Cannot read properties of undefined (reading 'orderId')"),
            (1, "    at mapOrder (/app/src/mappers/order.js:14:28)"),
            (2, "    at Array.map (<anonymous>)"),
            (3_000_000, "fail: Billing.Api.InvoiceService[0]"),
            (3_000_001, "      Invoice lookup failed"),
            (3_000_002, "      System.InvalidOperationException: Sequence contains no elements"),
            (3_000_003, "         at System.Linq.ThrowHelper.ThrowNoElementsException()"),
            (3_000_004, "GET /api 200 3ms"),
            (3_000_005, "GET /api 200 4ms"),
        ]);
        assert_eq!(out.len(), 4);
        assert_eq!((out[0].level, out[0].exc, out[0].lines), (Level::Error, true, 3));
        assert_eq!((out[1].level, out[1].exc, out[1].lines), (Level::Error, true, 4));
        assert_eq!(out[2].text, "GET /api 200 3ms");
    }

    #[test]
    fn json_lines_are_complete_entries() {
        let out = run(&[
            (
                0,
                r#"{"log.level":"ERROR","message":"Request failed","error.type":"java.lang.IllegalStateException","error.stack_trace":"java.lang.IllegalStateException: boom\n\tat a.B.c(B.java:1)"}"#,
            ),
            (1, r#"{"level":"info","msg":"handled request","request":{"method":"GET","uri":"/"},"status":200}"#),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].level, out[0].exc, out[0].lines), (Level::Error, true, 3));
        assert!(out[0].text.starts_with("Request failed\njava.lang.IllegalStateException"));
        assert_eq!(out[1].text, "GET / 200");
        assert!(out[1].raw.is_some());
    }

    #[test]
    fn go_and_rust_panics_are_one_entry() {
        let out = run(&[
            (0, "panic: runtime error: invalid memory address or nil pointer dereference"),
            (1, "[signal SIGSEGV: segmentation violation code=0x1 addr=0x0 pc=0x4a2b3c]"),
            (1, ""),
            (2, "goroutine 1 [running]:"),
            (2, "main.(*Server).handle(0x0, {0xc000010000, 0x5})"),
            (2, "\t/app/server.go:42 +0x1c"),
            (3, "main.main()"),
            (3, "\t/app/main.go:17 +0x25"),
            (500_000, "2026/10/29 17:00:01 starting server on :8080"),
            (2_000_000, "thread 'main' panicked at src/main.rs:5:5:"),
            (2_000_001, "called `Option::unwrap()` on a `None` value"),
            (2_000_002, "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace"),
        ]);
        assert_eq!(out.len(), 3, "{out:#?}");
        assert_eq!((out[0].level, out[0].exc, out[0].lines), (Level::Error, true, 8));
        assert!(out[0].pattern.as_deref().unwrap().starts_with("panic: runtime error"), "{:?}", out[0].pattern);
        assert_eq!(out[1].level, Level::Info);
        assert_eq!((out[2].level, out[2].lines), (Level::Error, 3));
    }

    #[test]
    fn php_and_ruby_traces_are_one_entry() {
        let out = run(&[
            (0, "[29-Oct-2026 17:00:00 UTC] PHP Fatal error:  Uncaught Exception: Cart 12 not found in /var/www/html/cart.php:8"),
            (1, "Stack trace:"),
            (1, "#0 /var/www/html/index.php(3): load_cart(12)"),
            (1, "#1 {main}"),
            (1, "  thrown in /var/www/html/cart.php on line 8"),
            (2_000_000, "/app/lib/cart.rb:12:in `total': undefined method `items' for nil:NilClass (NoMethodError)"),
            (2_000_001, "\tfrom /app/lib/checkout.rb:5:in `call'"),
            (2_000_002, "\tfrom app.rb:3:in `<main>'"),
        ]);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!((out[0].level, out[0].exc, out[0].lines), (Level::Error, true, 5));
        assert_eq!((out[1].level, out[1].exc, out[1].lines), (Level::Error, true, 3));
    }

    #[test]
    fn endless_indented_output_is_cut_into_entries() {
        let mut a = Assembler::new("svc");
        let mut out = vec![];
        a.push(0, StreamKind::Stdout, "table:", &mut out);
        for i in 1..5000 {
            a.push(i, StreamKind::Stdout, &format!("    row {i}"), &mut out);
        }
        a.flush(&mut out);
        assert_eq!(out.iter().map(|e| e.lines as usize).sum::<usize>(), 5000);
        assert!(out.iter().all(|e| e.lines as usize <= MAX_LINES));
        assert_eq!(out.len(), 3);
    }
}
