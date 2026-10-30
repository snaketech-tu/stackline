//! JSON API used by the UI. Every Docker access behind it is read-only; the
//! only things written are Stackline's own settings and log exports.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use stackline_core::config::{Collect, ConnectionConfig, ConnectionKind, Settings, new_id};
use stackline_core::logs::Level;
use stackline_core::runtime::test_connection;
use stackline_core::store::{ExportFormat, LogFilter};

use crate::AppState;

type St = State<Arc<AppState>>;

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "No such connection".into())
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/meta", get(meta))
        .route("/connections", get(list_connections).post(create_connection))
        .route("/connections/test", post(test))
        .route("/connections/{id}", put(update_connection).delete(delete_connection))
        .route("/c/{id}/snapshot", get(snapshot))
        .route("/c/{id}/pins", put(put_pins))
        .route("/c/{id}/collect", put(put_collect))
        .route("/c/{id}/usage", get(usage))
        .route("/c/{id}/usage/clear", post(clear_usage))
        .route("/c/{id}/logs", get(logs))
        .route("/c/{id}/logs/export", get(export))
        .route("/c/{id}/events", get(events))
        .route("/c/{id}/deploys", get(deploys))
        .route("/c/{id}/errors", get(errors))
        .route("/c/{id}/service", get(service_details))
        .route("/c/{id}/report", get(report))
        .route("/alerts", get(alerts))
        .route("/alerts/test", post(test_alert))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/store", get(store_stats))
        .route("/store/clear", post(clear_store))
        .route("/update", get(update_state))
        .route("/update/check", post(update_check))
        .route("/update/install", post(update_install))
        .route("/open", post(open_url))
}

async fn meta(st: St) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "desktop": st.desktop,
        "storePath": st.manager.store_path().display().to_string(),
        "platform": std::env::consts::OS,
    }))
}

async fn list_connections(st: St) -> Json<Value> {
    let cfg = st.manager.config();
    Json(json!(cfg.connections))
}

#[derive(Deserialize)]
struct ConnectionInput {
    name: String,
    #[serde(flatten)]
    kind: ConnectionKind,
}

fn validate(input: &ConnectionInput) -> Result<(), ApiError> {
    if input.name.trim().is_empty() {
        return Err(bad("Give the connection a name."));
    }
    if let ConnectionKind::Ssh(t) = &input.kind {
        if t.target.trim().is_empty() {
            return Err(bad("Enter a host, user@host or an alias from ~/.ssh/config."));
        }
        for v in [Some(&t.target), t.jump.as_ref(), t.identity.as_ref()].into_iter().flatten() {
            if v.trim_start().starts_with('-') || v.contains(char::is_whitespace) {
                return Err(bad(format!("\"{v}\" is not a valid host or path.")));
            }
        }
        if !t.socket.starts_with('/') {
            return Err(bad("The Docker socket path must start with /."));
        }
    }
    Ok(())
}

async fn create_connection(st: St, Json(input): Json<ConnectionInput>) -> Result<Json<ConnectionConfig>, ApiError> {
    validate(&input)?;
    let mut created = None;
    st.manager.update_config(|c| {
        let taken: Vec<String> = c.connections.iter().map(|x| x.id.clone()).collect();
        let cc = ConnectionConfig { id: new_id(&input.name, &taken), name: input.name.trim().to_string(), kind: input.kind };
        c.connections.push(cc.clone());
        created = Some(cc);
    })?;
    st.manager.open_all();
    Ok(Json(created.unwrap()))
}

async fn update_connection(st: St, Path(id): Path<String>, Json(input): Json<ConnectionInput>) -> Result<Json<ConnectionConfig>, ApiError> {
    validate(&input)?;
    let mut updated = None;
    st.manager.update_config(|c| {
        if let Some(cc) = c.connections.iter_mut().find(|x| x.id == id) {
            cc.name = input.name.trim().to_string();
            cc.kind = input.kind;
            updated = Some(cc.clone());
        }
    })?;
    st.manager.open_all();
    updated.map(Json).ok_or_else(not_found)
}

async fn delete_connection(st: St, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    st.manager.stop(&id);
    st.manager.update_config(|c| {
        c.connections.retain(|x| x.id != id);
        c.pins.remove(&id);
        c.collect.remove(&id);
    })?;
    let store = st.manager.store.clone();
    tokio::task::spawn_blocking(move || store.clear(Some(&id))).await.map_err(|e| anyhow::anyhow!(e))??;
    Ok(StatusCode::NO_CONTENT)
}

async fn test(Json(input): Json<ConnectionInput>) -> Result<Json<Value>, ApiError> {
    validate(&input)?;
    let cfg = ConnectionConfig { id: "test".into(), name: input.name, kind: input.kind };
    Ok(Json(json!(test_connection(&cfg).await)))
}

async fn snapshot(st: St, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let rt = st.manager.runtime(&id).ok_or_else(not_found)?;
    let mut cfg = st.manager.config();
    let pins = cfg.pins.remove(&id).unwrap_or_default();
    let collect = cfg.collect.remove(&id).unwrap_or_default();
    Ok(Json(json!({ "state": rt.state(), "snapshot": rt.snapshot().as_deref(), "pins": pins, "collect": collect })))
}

/// Replaces the pinned services of a connection. Each pin is `stack/service`.
async fn put_pins(st: St, Path(id): Path<String>, Json(pins): Json<Vec<String>>) -> Result<Json<Vec<String>>, ApiError> {
    let mut clean: Vec<String> = Vec::new();
    for p in pins {
        let p = p.trim().to_string();
        if p.is_empty() || p.len() > 300 || !p.contains('/') {
            return Err(bad(format!("\"{p}\" is not a service.")));
        }
        if !clean.contains(&p) {
            clean.push(p);
        }
    }
    if clean.len() > 500 {
        return Err(bad("Too many pinned services."));
    }
    let mut found = false;
    st.manager.update_config(|c| {
        found = c.connections.iter().any(|x| x.id == id);
        if !found {
            return;
        }
        if clean.is_empty() {
            c.pins.remove(&id);
        } else {
            c.pins.insert(id.clone(), clean.clone());
        }
    })?;
    if !found {
        return Err(not_found());
    }
    Ok(Json(clean))
}

/// Chooses which services of a connection have their logs collected.
async fn put_collect(st: St, Path(id): Path<String>, Json(c): Json<Collect>) -> Result<Json<Collect>, ApiError> {
    if !st.manager.config().connections.iter().any(|x| x.id == id) {
        return Err(not_found());
    }
    let mut services: Vec<String> = Vec::new();
    for s in c.services {
        let s = s.trim().to_string();
        if s.is_empty() || s.len() > 300 {
            return Err(bad(format!("\"{s}\" is not a service.")));
        }
        if !services.contains(&s) {
            services.push(s);
        }
    }
    if services.len() > 2000 {
        return Err(bad("Too many services."));
    }
    let c = Collect { only: c.only, services };
    st.manager.set_collect(&id, c.clone())?;
    Ok(Json(c))
}

/// Stored lines and text per service of a connection, and the services
/// whose lines are being deleted.
async fn usage(st: St, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let store = st.manager.store.clone();
    let conn = id.clone();
    let u = tokio::task::spawn_blocking(move || store.usage(&conn)).await.map_err(|e| anyhow::anyhow!(e))??;
    Ok(Json(json!({ "services": u, "deleting": st.manager.deleting(&id) })))
}

#[derive(Deserialize)]
struct ClearInput {
    service: String,
}

/// Starts deleting the stored lines of one service. Docker is not touched.
async fn clear_usage(st: St, Path(id): Path<String>, Json(input): Json<ClearInput>) -> (StatusCode, Json<Value>) {
    let started = st.manager.clear_service(&id, &input.service);
    (StatusCode::ACCEPTED, Json(json!({ "started": started })))
}

#[derive(Deserialize)]
struct LogParams {
    /// Unix milliseconds.
    from: i64,
    to: i64,
    q: Option<String>,
    /// Comma-separated stacks and full service names from the source picker.
    /// A line matches when it comes from any of them.
    stack: Option<String>,
    service: Option<String>,
    /// Comma-separated: ERROR,WARN,INFO,DEBUG
    levels: Option<String>,
    buckets: Option<usize>,
    limit: Option<usize>,
    format: Option<String>,
    save: Option<bool>,
}

impl LogParams {
    fn filter(&self) -> LogFilter {
        let mut f = LogFilter { from: self.from * 1000, to: self.to * 1000, ..Default::default() };
        let list = |v: &Option<String>| -> Vec<String> {
            v.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty() && *s != "all").map(str::to_string).collect()
        };
        f.pick_stacks = list(&self.stack);
        f.exact_services = list(&self.service);
        if let Some(l) = &self.levels {
            f.levels = l.split(',').filter_map(Level::parse_name).collect();
            f.no_levels = f.levels.is_empty();
        }
        if let Some(q) = &self.q {
            f.apply_query(q);
        }
        f
    }
}

async fn logs(st: St, Path(id): Path<String>, Query(p): Query<LogParams>) -> Result<Json<Value>, ApiError> {
    let rt = st.manager.runtime(&id).ok_or_else(not_found)?;
    let f = p.filter();
    let store = st.manager.store.clone();
    let (buckets, limit) = (p.buckets.unwrap_or(60), p.limit.unwrap_or(300));
    let res = tokio::task::spawn_blocking(move || store.query(&id, &f, buckets, limit)).await.map_err(|e| anyhow::anyhow!(e))??;
    let state = rt.state();
    Ok(Json(json!({ "result": res, "syncedAt": state.logs_synced_at, "logErrors": state.log_errors })))
}

/// Container starts, stops and crashes, with the source filters of the logs.
async fn events(st: St, Path(id): Path<String>, Query(p): Query<LogParams>) -> Result<Json<Value>, ApiError> {
    st.manager.runtime(&id).ok_or_else(not_found)?;
    let f = p.filter();
    let store = st.manager.store.clone();
    let limit = p.limit.unwrap_or(500).min(5000);
    let events = tokio::task::spawn_blocking(move || store.events(&id, &f, limit)).await.map_err(|e| anyhow::anyhow!(e))??;
    Ok(Json(json!({ "events": events })))
}

/// Deploys in the time range, newest first, each with the error groups that
/// first showed up after it (until the service's next deploy).
async fn deploys(st: St, Path(id): Path<String>, Query(p): Query<LogParams>) -> Result<Json<Value>, ApiError> {
    st.manager.runtime(&id).ok_or_else(not_found)?;
    let f = p.filter();
    let store = st.manager.store.clone();
    let now = chrono::Utc::now().timestamp_micros();
    let out = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
        let mut events: Vec<_> =
            store.events(&id, &f, 5000)?.into_iter().filter(|e| e.kind == stackline_core::events::Kind::Deploy).collect();
        events.sort_by_key(|e| std::cmp::Reverse(e.ts));
        events.truncate(50);
        let mut next: std::collections::HashMap<String, i64> = Default::default();
        let mut out = Vec::new();
        for e in events {
            let until = next.get(&e.service).copied().unwrap_or(now + 1);
            let new_errors = store.new_since(&id, &e.service, e.ts, until)?;
            next.insert(e.service.clone(), e.ts);
            out.push(json!({ "event": e, "until": until, "newErrors": new_errors }));
        }
        Ok(out)
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))??;
    Ok(Json(json!({ "deploys": out })))
}

async fn errors(st: St, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let rt = st.manager.runtime(&id).ok_or_else(not_found)?;
    let store = st.manager.store.clone();
    let now = chrono::Utc::now().timestamp_micros();
    let conn = id.clone();
    let mut s = tokio::task::spawn_blocking(move || store.error_summary(&conn, now)).await.map_err(|e| anyhow::anyhow!(e))??;
    // Quiet only counts for services that still run and are collected, while
    // collecting works: a stopped service or a broken connection is silent too.
    let synced = rt.state().logs_synced_at.is_some_and(|t| now / 1000 - t < 120_000);
    let collect = st.manager.config().collect.remove(&id).unwrap_or_default();
    let running: Vec<String> = rt
        .snapshot()
        .map(|snap| snap.stacks.iter().flat_map(|st| &st.services).filter(|s| s.running > 0).map(|s| s.full_name.clone()).collect())
        .unwrap_or_default();
    s.quiet.retain(|q| synced && collect.wants(&q.service) && running.contains(&q.service));
    Ok(Json(json!(s)))
}

#[derive(Deserialize)]
struct ServiceParams {
    /// The full service name.
    name: String,
}

async fn service_details(st: St, Path(id): Path<String>, Query(p): Query<ServiceParams>) -> Result<Json<Value>, ApiError> {
    let rt = st.manager.runtime(&id).ok_or_else(not_found)?;
    let d = rt.details(&p.name).await?;
    d.map(|d| Json(json!(d))).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("{} is not running here.", p.name)))
}

/// A Markdown debug report of one service, from what is stored here and the
/// service's configuration. See `stackline_core::report`.
async fn report(st: St, Path(id): Path<String>, Query(p): Query<ServiceParams>) -> Result<Response, ApiError> {
    use stackline_core::events::Kind;
    let rt = st.manager.runtime(&id).ok_or_else(not_found)?;
    let snap = rt.snapshot().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "Not connected yet.".into()))?;
    let service = snap
        .stacks
        .iter()
        .flat_map(|s| &s.services)
        .find(|s| s.full_name == p.name)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("{} is not running here.", p.name)))?;
    // The configuration is a nice to have: a report without it still helps.
    let details = rt.details(&p.name).await.ok().flatten();
    let now = chrono::Utc::now().timestamp_micros();
    let day = LogFilter { from: now - 86_400_000_000, to: now + 1, exact_services: vec![p.name.clone()], ..Default::default() };
    let (store, conn) = (st.manager.store.clone(), id.clone());
    let (logs, problems, events, summary, before, fresh) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let logs = store.query(&conn, &day, 1, 0)?;
        let problems = store.query(&conn, &LogFilter { levels: vec![Level::Error, Level::Warn], ..day.clone() }, 1, 0)?;
        let mut events = store.events(&conn, &day, 500)?;
        events.sort_by_key(|e| e.ts);
        let summary = store.error_summary(&conn, now)?;
        // What the replica printed in the 2 minutes before its newest crash.
        let crash = events.iter().rev().find(|e| matches!(e.kind, Kind::Crash | Kind::Oom | Kind::Failed));
        let before = match crash {
            Some(c) => {
                let f = LogFilter { from: c.ts - 120_000_000, to: c.ts + 15_000_000, sources: vec![c.source.clone()], ..day.clone() };
                let mut e = store.query(&conn, &f, 1, 40)?.entries;
                e.reverse();
                e
            }
            None => vec![],
        };
        let fresh = match events.iter().rev().find(|e| e.kind == Kind::Deploy) {
            Some(d) => store.new_since(&conn, &d.service, d.ts, now + 1)?,
            None => vec![],
        };
        Ok((logs, problems, events, summary, before, fresh))
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))??;
    let quiet = summary.quiet.iter().find(|q| q.service == p.name && service.running > 0);
    let name = st.manager.config().connections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or(id);
    let md = stackline_core::report::markdown(&stackline_core::report::Report {
        connection: &name,
        host: &snap.host,
        service,
        details: details.as_ref(),
        total: logs.total,
        levels: &logs.levels,
        groups: &problems.groups,
        events: &events,
        before_crash: &before,
        quiet,
        new_since_deploy: &fresh,
        now: now / 1000,
        version: env!("CARGO_PKG_VERSION"),
    });
    Ok(([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], md).into_response())
}

async fn alerts(st: St) -> Json<Value> {
    Json(json!({ "alerts": st.manager.alerts.list(), "native": st.manager.alerts.native() }))
}

/// Shows a sample notification, so people can check that alerts reach them.
async fn test_alert(st: St) -> Json<Value> {
    let s = st.manager.config().settings;
    let native = st.manager.alerts.send(
        "Errors on your server",
        &format!("This is how Stackline tells you about {} or more errors in {} minutes.", s.alert_threshold, s.alert_window_mins),
    );
    Json(json!({ "native": native }))
}

async fn export(st: St, Path(id): Path<String>, Query(p): Query<LogParams>) -> Result<Response, ApiError> {
    st.manager.runtime(&id).ok_or_else(not_found)?;
    let (format, ext, mime) = match p.format.as_deref().unwrap_or("log") {
        "jsonl" => (ExportFormat::Jsonl, "jsonl", "application/x-ndjson"),
        "csv" => (ExportFormat::Csv, "csv", "text/csv"),
        _ => (ExportFormat::Log, "log", "text/plain"),
    };
    let name = format!("stackline-{id}-{}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M"));
    let f = p.filter();
    let store = st.manager.store.clone();
    if p.save.unwrap_or(false) {
        let dir = dirs_downloads();
        let path = dir.join(&name);
        let path2 = path.clone();
        let n = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
            std::fs::create_dir_all(&dir)?;
            let mut file = std::io::BufWriter::new(std::fs::File::create(&path2)?);
            store.export(&id, &f, format, &mut file)
        })
        .await
        .map_err(|e| anyhow::anyhow!(e))??;
        return Ok(Json(json!({ "path": path.display().to_string(), "entries": n })).into_response());
    }
    // Streamed while it is read: a week of logs can be gigabytes, which must
    // not sit in memory. If the download is cancelled, writing fails and the
    // export stops.
    let (rx, tx) = tokio::io::duplex(256 * 1024);
    tokio::task::spawn_blocking(move || {
        let mut out = std::io::BufWriter::with_capacity(64 * 1024, tokio_util::io::SyncIoBridge::new(tx));
        if let Err(e) = store.export(&id, &f, format, &mut out).and_then(|_| Ok(std::io::Write::flush(&mut out)?)) {
            tracing::debug!("export stopped: {e:#}");
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, format!("{mime}; charset=utf-8")),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(rx)),
    )
        .into_response())
}

fn dirs_downloads() -> std::path::PathBuf {
    stackline_core::config::downloads_dir()
}

async fn get_settings(st: St) -> Json<Settings> {
    Json(st.manager.config().settings)
}

async fn put_settings(st: St, Json(s): Json<Settings>) -> Result<Json<Settings>, ApiError> {
    if !["system", "light", "dark"].contains(&s.theme.as_str()) {
        return Err(bad("Theme must be system, light or dark."));
    }
    if !(1..=90).contains(&s.retention_days) {
        return Err(bad("Keep logs for 1 to 90 days."));
    }
    if s.max_store_mb < 100 {
        return Err(bad("Allow at least 100 MB for logs."));
    }
    if !(2..=600).contains(&s.refresh_secs) || !(1..=600).contains(&s.log_poll_secs) {
        return Err(bad("Refresh intervals must be between 2 and 600 seconds."));
    }
    if !(5..=15).contains(&s.alert_window_mins) {
        return Err(bad("Count errors over 5 to 15 minutes."));
    }
    if !(1..=1_000_000).contains(&s.alert_threshold) {
        return Err(bad("The alert threshold must be at least 1 error."));
    }
    let c = st.manager.update_config(|c| c.settings = s)?;
    st.manager.open_all();
    Ok(Json(c.settings))
}

async fn update_state(st: St) -> Json<Value> {
    Json(json!(st.updater.state()))
}

async fn update_check(st: St) -> Json<Value> {
    Json(json!(st.updater.check().await))
}

async fn update_install(st: St) -> Result<Json<Value>, ApiError> {
    st.updater.install().map_err(|e| bad(format!("{e:#}")))?;
    Ok(Json(json!(st.updater.state())))
}

#[derive(Deserialize)]
struct OpenInput {
    url: String,
}

/// Opens a web link in the system browser. The desktop window has no tabs of
/// its own, so links from the UI (release notes, downloads) go through here.
async fn open_url(st: St, Json(input): Json<OpenInput>) -> Result<StatusCode, ApiError> {
    if !st.desktop {
        return Err(bad("Only the desktop app opens links this way."));
    }
    if !input.url.starts_with("https://") || input.url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad("Only https links can be opened."));
    }
    webbrowser::open(&input.url).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not open the browser: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn store_stats(st: St) -> Result<Json<Value>, ApiError> {
    let store = st.manager.store.clone();
    let s = tokio::task::spawn_blocking(move || store.stats()).await.map_err(|e| anyhow::anyhow!(e))??;
    let mut v = json!(s);
    v["maintenanceError"] = json!(st.manager.maintenance_error());
    Ok(Json(v))
}

async fn clear_store(st: St) -> Result<StatusCode, ApiError> {
    let store = st.manager.store.clone();
    tokio::task::spawn_blocking(move || store.clear(None)).await.map_err(|e| anyhow::anyhow!(e))??;
    Ok(StatusCode::NO_CONTENT)
}
