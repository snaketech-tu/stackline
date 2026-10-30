//! The HTTP layer, driven without a socket: sign-in, the host check,
//! settings validation and what deleting a connection cleans up.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use stackline::update::Updater;
use stackline::{AppState, router};
use stackline_core::config::Paths;
use stackline_core::runtime::Manager;
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const HOST: &str = "127.0.0.1:7070";

struct App {
    router: Router,
    state: Arc<AppState>,
    dir: std::path::PathBuf,
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn app() -> App {
    // A counter, because tests run in parallel and macOS clocks tick in microseconds.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("stackline-api-test-{}-{nanos}-{n}", std::process::id()));
    let manager = Manager::new(Paths::in_dir(&dir)).unwrap();
    let state =
        Arc::new(AppState { manager, token: TOKEN.into(), desktop: false, allowed_hosts: vec![HOST.into()], updater: Updater::new(None) });
    App { router: router(state.clone()), state, dir }
}

/// A signed-in request to this app's own address.
fn req(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut b =
        Request::builder().method(method).uri(path).header(header::HOST, HOST).header(header::COOKIE, format!("stackline_token={TOKEN}"));
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

async fn send(app: &App, r: Request<Body>) -> (StatusCode, Value) {
    let res = app.router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, body)
}

#[tokio::test]
async fn the_api_needs_the_session_token() {
    let app = app();
    let anonymous = Request::builder().uri("/api/meta").header(header::HOST, HOST).body(Body::empty()).unwrap();
    let (status, body) = send(&app, anonymous).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    let (status, body) = send(&app, req("GET", "/api/meta", None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["desktop"], false);

    // The header form, used during UI development.
    let with_header =
        Request::builder().uri("/api/meta").header(header::HOST, HOST).header("x-stackline-token", TOKEN).body(Body::empty()).unwrap();
    assert_eq!(send(&app, with_header).await.0, StatusCode::OK);

    // A tab signed in by swarmdash, the name before Stackline, stays signed in after the update.
    let old_cookie = Request::builder()
        .uri("/api/meta")
        .header(header::HOST, HOST)
        .header(header::COOKIE, format!("theme=dark; swarmdash_token={TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, old_cookie).await.0, StatusCode::OK);
    let wrong = Request::builder()
        .uri("/api/meta")
        .header(header::HOST, HOST)
        .header(header::COOKIE, "swarmdash_token=0000")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, wrong).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn only_answers_on_its_own_address() {
    let app = app();
    let elsewhere = Request::builder()
        .uri("/api/meta")
        .header(header::HOST, "dashboard.example.com")
        .header(header::COOKIE, format!("stackline_token={TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, elsewhere).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_link_with_the_token_signs_the_browser_in() {
    let app = app();
    let open = Request::builder().uri(format!("/?token={TOKEN}#logs")).header(header::HOST, HOST).body(Body::empty()).unwrap();
    let res = app.router.clone().oneshot(open).await.unwrap();
    assert!(res.status().is_redirection(), "{}", res.status());
    let cookie = res.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with(&format!("stackline_token={TOKEN};")), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");

    // A wrong token signs nobody in.
    let wrong = Request::builder().uri("/?token=nope").header(header::HOST, HOST).body(Body::empty()).unwrap();
    let res = app.router.clone().oneshot(wrong).await.unwrap();
    assert!(res.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn settings_are_checked_before_they_are_saved() {
    let app = app();
    let (_, mut s) = send(&app, req("GET", "/api/settings", None)).await;
    assert_eq!(s["retentionDays"], 2);

    s["retentionDays"] = json!(0);
    let (status, body) = send(&app, req("PUT", "/api/settings", Some(s.clone()))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("1 to 90"), "{body}");

    s["retentionDays"] = json!(14);
    s["theme"] = json!("dark");
    let (status, body) = send(&app, req("PUT", "/api/settings", Some(s))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(app.state.manager.config().settings.retention_days, 14);
    assert_eq!(app.state.manager.config().settings.theme, "dark");
}

#[tokio::test]
async fn deleting_a_connection_forgets_its_pins_and_its_collect_choice() {
    let app = app();
    let input = json!({ "name": "Prod EU", "kind": "ssh", "target": "deploy@203.0.113.10", "socket": "/var/run/docker.sock" });
    let (status, c) = send(&app, req("POST", "/api/connections", Some(input))).await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(c["id"], "prod-eu");

    let (status, _) = send(&app, req("PUT", "/api/c/prod-eu/pins", Some(json!(["shop/api"])))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, req("PUT", "/api/c/prod-eu/collect", Some(json!({ "only": true, "services": ["shop_api"] })))).await;
    assert_eq!(status, StatusCode::OK);
    let cfg = app.state.manager.config();
    assert_eq!(cfg.pins["prod-eu"], vec!["shop/api"]);
    assert!(cfg.collect["prod-eu"].only);

    let (status, _) = send(&app, req("DELETE", "/api/connections/prod-eu", None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let cfg = app.state.manager.config();
    assert!(cfg.connections.iter().all(|c| c.id != "prod-eu"));
    assert!(!cfg.pins.contains_key("prod-eu"), "pins stayed behind: {:?}", cfg.pins);
    assert!(!cfg.collect.contains_key("prod-eu"), "the collect choice stayed behind: {:?}", cfg.collect);

    // Bad input never reaches the config.
    let (status, body) =
        send(&app, req("POST", "/api/connections", Some(json!({ "name": "x", "kind": "ssh", "target": "-oProxyCommand=evil" })))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
