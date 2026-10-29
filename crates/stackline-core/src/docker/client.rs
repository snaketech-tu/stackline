//! A deliberately small Docker Engine API client.
//!
//! Stackline is read-only by construction: this client can only send `GET`
//! requests. There is no code path that creates, updates or removes anything.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{HOST, USER_AGENT};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;

use super::endpoint::Endpoint;

const JSON_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
pub struct DockerClient {
    endpoint: Endpoint,
}

impl DockerClient {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Sends a GET request and returns the response with its body still streaming.
    pub async fn get(&self, path_and_query: &str) -> Result<Response<Incoming>> {
        let stream = self.endpoint.connect().await.with_context(|| format!("could not reach Docker at {}", self.endpoint.describe()))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.context("HTTP handshake with Docker failed")?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let req = Request::builder()
            .method(Method::GET)
            .uri(path_and_query)
            .header(HOST, "docker")
            .header(USER_AGENT, concat!("stackline/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())?;
        let res = sender.send_request(req).await.context("Docker request failed")?;
        Ok(res)
    }

    /// GET a JSON document.
    pub async fn get_json<T: DeserializeOwned>(&self, path_and_query: &str) -> Result<T> {
        let fut = async {
            let res = self.get(path_and_query).await?;
            let status = res.status();
            let body = res.into_body().collect().await?.to_bytes();
            if !status.is_success() {
                return Err(api_error(status, &body, path_and_query));
            }
            serde_json::from_slice::<T>(&body).with_context(|| format!("unexpected response from Docker for {path_and_query}"))
        };
        tokio::time::timeout(JSON_TIMEOUT, fut)
            .await
            .map_err(|_| anyhow!("Docker did not answer {path_and_query} within {JSON_TIMEOUT:?}"))?
    }

    /// GET a streaming endpoint (logs). Fails on a non-2xx status.
    pub async fn get_stream(&self, path_and_query: &str) -> Result<Incoming> {
        let res = self.get(path_and_query).await?;
        let status = res.status();
        if !status.is_success() {
            let body = res.into_body().collect().await?.to_bytes();
            return Err(api_error(status, &body, path_and_query));
        }
        Ok(res.into_body())
    }

    pub async fn ping(&self) -> Result<()> {
        let res = self.get("/_ping").await?;
        if res.status() != StatusCode::OK {
            bail!("Docker answered /_ping with {}", res.status());
        }
        Ok(())
    }
}

fn api_error(status: StatusCode, body: &[u8], path: &str) -> anyhow::Error {
    let msg = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string());
    anyhow!("Docker returned {status} for {path}: {msg}")
}
