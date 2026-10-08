//! A tiny in-memory, per-key fixed-window rate limiter plus the middleware that
//! applies it to the auth and chat routes.
//!
//! This is intentionally process-local: it protects a single replica from
//! brute-force login attempts and runaway chat loops. Across replicas, put a
//! shared limiter (Redis/Envoy) in front; the window here is a coarse guard,
//! not a distributed quota.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::state::AppState;

struct Window {
    started: Instant,
    count: u32,
}

/// Fixed-window counters keyed by an arbitrary string (route scope + client).
#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Window>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one hit against `key`. Returns `Err(retry_after)` when the window
    /// is exhausted.
    pub fn check(&self, key: &str, limit: u32, window: Duration) -> Result<(), Duration> {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("rate limiter mutex poisoned");

        // Opportunistic cleanup so idle keys cannot grow the map unbounded.
        if buckets.len() > 10_000 {
            buckets.retain(|_, w| now.duration_since(w.started) < window);
        }

        let entry = buckets.entry(key.to_string()).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(entry.started) >= window {
            entry.started = now;
            entry.count = 0;
        }
        if entry.count >= limit {
            return Err(window.saturating_sub(now.duration_since(entry.started)));
        }
        entry.count += 1;
        Ok(())
    }
}

/// Resolve the client address, honouring a reverse proxy's `X-Forwarded-For` /
/// `X-Real-IP` before falling back to the socket peer.
fn client_key(headers: &HeaderMap, addr: SocketAddr) -> String {
    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return ip.to_string();
    }
    if let Some(ip) = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return ip.to_string();
    }
    addr.ip().to_string()
}

fn too_many(retry_after: Duration) -> Response {
    let seconds = retry_after.as_secs().max(1);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({ "error": "rate limit exceeded, slow down" })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
    response
}

async fn limited(
    state: &AppState,
    key: String,
    limit: u32,
    request: Request,
    next: Next,
) -> Response {
    match state.rate_limit.check(&key, limit, Duration::from_secs(60)) {
        Ok(()) => next.run(request).await,
        Err(retry) => too_many(retry),
    }
}

/// Middleware for `/api/auth/*`.
pub async fn limit_auth(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    if !state.cfg.rate_limit_enabled {
        return next.run(request).await;
    }
    let key = format!("auth:{}", client_key(&headers, addr));
    limited(
        &state,
        key,
        state.cfg.rate_limit_auth_per_minute,
        request,
        next,
    )
    .await
}

/// Middleware for `/api/chat`.
pub async fn limit_chat(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    if !state.cfg.rate_limit_enabled {
        return next.run(request).await;
    }
    let key = format!("chat:{}", client_key(&headers, addr));
    limited(
        &state,
        key,
        state.cfg.rate_limit_chat_per_minute,
        request,
        next,
    )
    .await
}
