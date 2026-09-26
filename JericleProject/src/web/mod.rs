//! axum router and JSON API. The UI is a single embedded HTML page.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::pipeline::{self, AppState};

const INDEX_HTML: &str = include_str!("index.html");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/dashboard", get(api_dashboard))
        .route("/api/refresh", get(api_refresh))
        .route("/api/options/{symbol}", get(api_options))
        .route("/api/history/{symbol}", get(api_history))
        .route("/health", get(health))
        .with_state(state)
}

/// `no-store` because the page is embedded in the binary: a cached copy would keep
/// showing a stale dashboard after a rebuild, which is exactly the kind of thing
/// that makes you doubt your own eyes.
async fn index() -> impl IntoResponse {
    (
        [
            (header::CACHE_CONTROL, "no-store, max-age=0"),
            (header::PRAGMA, "no-cache"),
        ],
        Html(INDEX_HTML),
    )
}

/// Option walls for the current-month expiry. On demand and cached, because the
/// upstream payloads are large and open interest only changes once a day.
async fn api_options(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.is_empty() || symbol.len() > 12 || !symbol.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return Json(json!({ "ok": false, "error": "bad symbol" }));
    }
    match state.option_walls(&symbol, state.config.options_cache_seconds).await {
        Ok(summary) => {
            // Distance-from-spot and the put/call ratio are derived, so they are added
            // here rather than stored on the summary.
            let mut v = serde_json::to_value(&*summary).unwrap_or_else(|_| json!({}));
            if let Some(o) = v.as_object_mut() {
                o.insert("ok".into(), json!(true));
                o.insert("putCallRatio".into(), json!(summary.put_call_ratio()));
                o.insert("callWallDistancePct".into(), json!(summary.distance_pct(summary.call_wall)));
                o.insert("putWallDistancePct".into(), json!(summary.distance_pct(summary.put_wall)));
            }
            Json(v)
        }
        Err(e) => Json(json!({ "ok": false, "symbol": symbol, "error": e.to_string() })),
    }
}

async fn api_dashboard(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let d = state.snapshot();
    let age = (chrono::Utc::now().timestamp() - d.generated_ts).max(0);
    let mut body = serde_json::to_value(&*d).unwrap_or_else(|_| json!({}));
    if let Some(obj) = body.as_object_mut() {
        obj.insert("snapshotAgeSeconds".into(), json!(age));
        obj.insert("servedAt".into(), json!(pipeline::now_iso()));
    }
    (
        [(header::CACHE_CONTROL, "no-store, max-age=0")],
        Json(body),
    )
}

async fn api_refresh(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.request_refresh();
    match pipeline::refresh(&state).await {
        Ok(d) => Json(json!({
            "ok": true,
            "generatedEt": d.generated_et,
            "headlines": d.news.len(),
            "warnings": d.warnings,
        })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

async fn api_history(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> impl IntoResponse {
    match state.store.symbol_history(&symbol, 500) {
        Ok(rows) => Json(json!({ "ok": true, "symbol": symbol.to_uppercase(), "rows": rows })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let d = state.snapshot();
    let (snapshots, news, prices) = state.store.counts();
    Json(json!({
        "ok": true,
        "generatedEt": d.generated_et,
        "phase": d.phase.phase,
        "rows": { "snapshots": snapshots, "news": news, "prices": prices },
    }))
}
