//! Structured (JSON-per-line) logs: ECS, Spring Boot, Caddy/zap, pino, Serilog, logrus.

use serde_json::{Map, Value};

use super::level::Level;

#[derive(Clone, Debug, PartialEq)]
pub struct Structured {
    pub level: Option<Level>,
    pub message: String,
    pub stack: Option<String>,
    pub error_type: Option<String>,
    pub logger: Option<String>,
}

/// Parses a JSON log line. Returns `None` for anything that is not a JSON object.
pub fn parse(line: &str) -> Option<Structured> {
    let t = line.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let v: Value = serde_json::from_str(t).ok()?;
    let o = v.as_object()?;

    // MongoDB 4.4 and later: `{"t":{"$date":…},"s":"E","c":"NETWORK","msg":…}`
    let mongo = o.get("t").is_some_and(|t| t.get("$date").is_some()) && o.get("s").is_some_and(Value::is_string);
    let level = first(
        o,
        &[&["log.level"], &["log", "level"], &["level"], &["severity"], &["lvl"], &["loglevel"], &["levelname"], &["@l"], &["Level"]],
    )
    .and_then(|v| match v {
        Value::String(s) => Level::parse_name(s),
        Value::Number(n) => n.as_i64().map(Level::from_number),
        _ => None,
    })
    .or_else(|| mongo.then(|| super::level::mongo_severity(o["s"].as_str().unwrap_or(""))));
    let logger = text(first(o, &[&["log.logger"], &["log", "logger"], &["logger"], &["logger_name"], &["SourceContext"]]));
    let mut message =
        text(first(o, &[&["message"], &["msg"], &["@m"], &["@mt"], &["MessageTemplate"], &["log"], &["event"]])).unwrap_or_default();
    let stack = text(first(
        o,
        &[
            &["error.stack_trace"],
            &["error", "stack_trace"],
            &["stack_trace"],
            &["exception.stacktrace"],
            &["exception", "stacktrace"],
            &["@x"],
            &["Exception"],
            &["exception"],
            &["err", "stack"],
            &["error", "stack"],
            &["stack"],
            &["stacktrace"],
        ],
    ))
    .filter(|s| !s.trim().is_empty());
    let error_type = text(first(o, &[&["error.type"], &["error", "type"], &["err", "type"], &["exception.type"], &["exception", "type"]]));

    // Caddy and other access logs: show the request instead of "handled request".
    let is_access = logger.as_deref().is_some_and(|l| l.contains("access")) || message == "handled request";
    if let Some(req) = o.get("request").and_then(Value::as_object).filter(|_| is_access) {
        let method = req.get("method").and_then(Value::as_str);
        let uri = req.get("uri").and_then(Value::as_str);
        if let (Some(m), Some(u)) = (method, uri) {
            let status = o.get("status").and_then(Value::as_i64).map(|s| s.to_string()).unwrap_or_default();
            let dur = o
                .get("duration")
                .and_then(Value::as_f64)
                .map(|d| if d < 1000.0 { format!(" {:.0}ms", d * 1000.0) } else { format!(" {d:.0}ms") })
                .unwrap_or_default();
            message = format!("{m} {u} {status}{dur}").trim().to_string();
        }
    }
    // Traefik access logs in JSON: `RequestMethod`, `RequestPath`, `DownstreamStatus`.
    if let (Some(m), Some(path)) = (o.get("RequestMethod").and_then(Value::as_str), o.get("RequestPath").and_then(Value::as_str)) {
        let status = o.get("DownstreamStatus").and_then(Value::as_i64).map(|s| s.to_string()).unwrap_or_default();
        let dur = o.get("Duration").and_then(Value::as_f64).map(|ns| format!(" {:.0}ms", ns / 1e6)).unwrap_or_default();
        let router = o.get("RouterName").and_then(Value::as_str).map(|r| format!(" ({r})")).unwrap_or_default();
        message = format!("{m} {path} {status}{dur}{router}").trim().to_string();
    }
    // An access log line with a 5xx status is an error, whatever level it was logged at.
    let status = o.get("status").or_else(|| o.get("DownstreamStatus")).and_then(Value::as_i64);
    let level = match (status, level) {
        (Some(500..=599), l) if (is_access || o.contains_key("RequestPath")) && l.is_none_or(|l| l < Level::Error) => Some(Level::Error),
        (_, l) => l,
    };
    // MongoDB puts the details of an error into `attr`.
    if mongo && let Some(e) = o.get("attr").and_then(|a| a.get("error")) {
        let detail = e.get("errmsg").and_then(Value::as_str).map(str::to_string).or_else(|| text(Some(e)));
        if let Some(d) = detail.filter(|d| !d.is_empty()) {
            message = format!("{message}: {d}");
        }
    }
    if message.is_empty() {
        message = text(first(o, &[&["error.message"], &["error", "message"], &["err", "message"], &["error"]])).unwrap_or_default();
    }
    if message.is_empty() {
        message = t.to_string();
    }
    Some(Structured { level, message, stack, error_type, logger })
}

fn first<'a>(o: &'a Map<String, Value>, paths: &[&[&str]]) -> Option<&'a Value> {
    for path in paths {
        let mut cur: Option<&Value> = None;
        for (i, key) in path.iter().enumerate() {
            cur = if i == 0 { o.get(*key) } else { cur.and_then(|c| c.get(*key)) };
        }
        if let Some(v) = cur
            && !v.is_null()
        {
            return Some(v);
        }
    }
    None
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        Value::Array(a) => {
            Some(a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join("\n"))
        }
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_boot_ecs_with_stack_trace() {
        let line = r#"{"@timestamp":"2026-10-27T20:15:20.713Z","log.level":"ERROR","process.pid":1,"process.thread.name":"http-nio-8080-exec-3","service.name":"shop","log.logger":"o.a.c.c.C.[.[.[/].[dispatcherServlet]","message":"Servlet.service() threw exception","error.type":"java.lang.IllegalStateException","error.message":"boom","error.stack_trace":"java.lang.IllegalStateException: boom\n\tat com.example.Foo.bar(Foo.java:12)\n\tat com.example.Foo.baz(Foo.java:7)\n","ecs.version":"8.11"}"#;
        let s = parse(line).unwrap();
        assert_eq!(s.level, Some(Level::Error));
        assert_eq!(s.message, "Servlet.service() threw exception");
        assert_eq!(s.error_type.as_deref(), Some("java.lang.IllegalStateException"));
        assert!(s.stack.unwrap().contains("Foo.java:12"));
    }

    #[test]
    fn nested_ecs_and_caddy_access() {
        let s = parse(r#"{"log":{"level":"WARN","logger":"x"},"message":"slow"}"#).unwrap();
        assert_eq!((s.level, s.message.as_str()), (Some(Level::Warn), "slow"));
        let c = parse(r#"{"level":"info","ts":1730060120.7,"logger":"http.log.access.log0","msg":"handled request","request":{"method":"GET","uri":"/api/health"},"duration":0.0042,"status":200}"#).unwrap();
        assert_eq!(c.message, "GET /api/health 200 4ms");
        assert_eq!(c.level, Some(Level::Info));
        let e = parse(r#"{"level":"error","logger":"http.log.error","msg":"dial tcp 172.18.0.4:8080: connect: connection refused","request":{"method":"GET","uri":"/"},"status":502}"#).unwrap();
        assert_eq!(e.message, "dial tcp 172.18.0.4:8080: connect: connection refused");
    }

    #[test]
    fn mongodb_traefik_and_failed_requests() {
        let m = parse(r#"{"t":{"$date":"2026-10-29T17:00:00.000+00:00"},"s":"E","c":"NETWORK","id":4615610,"ctx":"conn12","msg":"Failed to check socket connectivity","attr":{"error":{"code":6,"codeName":"HostUnreachable","errmsg":"Connection reset by peer"}}}"#).unwrap();
        assert_eq!((m.level, m.message.as_str()), (Some(Level::Error), "Failed to check socket connectivity: Connection reset by peer"));
        let i = parse(r#"{"t":{"$date":"2026-10-29T17:00:00.000+00:00"},"s":"I","c":"NETWORK","id":22943,"ctx":"listener","msg":"Connection accepted"}"#).unwrap();
        assert_eq!(i.level, Some(Level::Info));
        let d = parse(r#"{"t":{"$date":"2026-10-29T17:00:00.000+00:00"},"s":"D2","c":"COMMAND","id":1,"msg":"run command"}"#).unwrap();
        assert_eq!(d.level, Some(Level::Debug));

        let t = parse(r#"{"ClientHost":"10.0.0.1","DownstreamStatus":502,"Duration":3021000,"RequestMethod":"POST","RequestPath":"/api/cart","RouterName":"shop@docker","level":"info","msg":"","time":"2026-10-29T17:00:00Z"}"#).unwrap();
        assert_eq!((t.level, t.message.as_str()), (Some(Level::Error), "POST /api/cart 502 3ms (shop@docker)"));
        let ok = parse(r#"{"DownstreamStatus":200,"RequestMethod":"GET","RequestPath":"/","level":"info","msg":""}"#).unwrap();
        assert_eq!(ok.level, Some(Level::Info));

        // Caddy logs a failed request at info level.
        let c = parse(
            r#"{"level":"info","logger":"http.log.access.log0","msg":"handled request","request":{"method":"GET","uri":"/"},"status":502}"#,
        )
        .unwrap();
        assert_eq!((c.level, c.message.as_str()), (Some(Level::Error), "GET / 502"));
    }

    #[test]
    fn pino_numeric_levels_and_non_json() {
        let p = parse(r#"{"level":50,"time":1,"msg":"failed","err":{"type":"TypeError","message":"x","stack":"TypeError: x\n    at a (/app/a.js:1:1)"}}"#).unwrap();
        assert_eq!(p.level, Some(Level::Error));
        assert_eq!(p.error_type.as_deref(), Some("TypeError"));
        assert!(parse("GET / 200").is_none());
        assert!(parse("{not json}").is_none());
    }
}
