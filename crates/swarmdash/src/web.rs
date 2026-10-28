//! Routing, sign-in and the embedded UI.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use rust_embed::RustEmbed;

use crate::AppState;

#[derive(RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

const COOKIE: &str = "swarmdash_token";

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/api", crate::api::router())
        .fallback(static_file)
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Only answers requests addressed to this machine, and only API calls that carry the session token.
async fn guard(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if !st.allowed_hosts.iter().any(|h| h == host) {
        return (StatusCode::FORBIDDEN, "swarmdash only answers on its own address").into_response();
    }
    let path = req.uri().path().to_string();
    if let Some(t) = query_param(req.uri(), "token")
        && t == st.token
    {
        let mut res = Redirect::to(&path).into_response();
        let cookie = format!("{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict", st.token);
        res.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
        return res;
    }
    if path.starts_with("/api/") && !authorized(req.headers(), &st.token) {
        return (StatusCode::UNAUTHORIZED, "Open swarmdash with the link printed when it started.").into_response();
    }
    next.run(req).await
}

fn authorized(h: &HeaderMap, token: &str) -> bool {
    if h.get("x-swarmdash-token").and_then(|v| v.to_str().ok()) == Some(token) {
        return true;
    }
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|c| c.trim().strip_prefix(&format!("{COOKIE}=")) == Some(token))
}

fn query_param(uri: &Uri, key: &str) -> Option<String> {
    uri.query()?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

async fn static_file(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (f, path.to_string()),
        _ => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_string()),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    // A release build embeds the files: serve them from the binary instead of
    // copying every file on every request.
    let body = match file.data {
        std::borrow::Cow::Borrowed(b) => Body::from(axum::body::Bytes::from_static(b)),
        std::borrow::Cow::Owned(v) => Body::from(v),
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .unwrap()
}
