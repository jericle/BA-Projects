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
        .route("/api/series/{symbol}", get(api_series))
        .route("/api/series-meta", get(api_series_meta))
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

/// Validates a symbol the same way `api_options` does, so a path segment cannot
/// smuggle a query string or a slash into an upstream request.
fn clean_symbol(raw: &str) -> std::result::Result<String, axum::http::StatusCode> {
    let s = raw.trim().to_ascii_uppercase();
    if s.is_empty()
        || s.len() > 12
        || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    Ok(s)
}

/// OHLC series for one symbol at one interval, from Twelve Data.
///
/// The browser never learns the upstream URL or the key: it asks this endpoint,
/// which spends a credit server-side and returns only bars. The response is
/// `no-store` so a proxied chart cannot be cached by the browser or a shared proxy.
async fn api_series(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
    axum::extract::Query(q): axum::extract::Query<SeriesQuery>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    let symbol = clean_symbol(&symbol).map_err(|st| {
        (st, axum::Json(json!({ "ok": false, "error": "bad symbol" })))
    })?;

    let interval = q
        .interval
        .as_deref()
        .unwrap_or("1day")
        .trim()
        .to_ascii_lowercase();
    if !crate::sources::twelvedata::is_valid_interval(&interval) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "ok": false,
                "error": format!("unsupported interval {interval:?}"),
                "supported": crate::sources::twelvedata::INTERVALS
                    .iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            })),
        ));
    }
    let outputsize = q
        .outputsize
        .unwrap_or_else(|| crate::sources::twelvedata::default_outputsize(&interval) as u32)
        .min(5000) as usize;

    match state.series(&symbol, &interval, outputsize).await {
        Ok(series) => Ok((
            [(header::CACHE_CONTROL, "no-store, max-age=0")],
            axum::Json(json!({ "ok": true, "series": &*series })),
        )),
        // A rate refusal is the client's problem to retry, not a server fault, so
        // it gets 429 with the wait spelled out rather than a 500.
        Err(e) if e.to_string().contains("rate limit") => Err((
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({ "ok": false, "error": e.to_string() })),
        )),
        Err(e) => Ok((
            [(header::CACHE_CONTROL, "no-store, max-age=0")],
            axum::Json(json!({ "ok": false, "symbol": symbol, "error": e.to_string() })),
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct SeriesQuery {
    interval: Option<String>,
    outputsize: Option<u32>,
}

/// What the UI needs to render the timeframe picker, plus whether the feature is
/// usable at all. Carries no key and no account detail — only whether one exists.
async fn api_series_meta(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "ok": true,
        "enabled": state.series_enabled,
        "provider": crate::sources::twelvedata::PROVIDER,
        "intervals": crate::sources::twelvedata::INTERVALS
            .iter()
            .map(|(i, n)| json!({
                "interval": i,
                "defaultOutputsize": n,
                // The UI needs this to pick a time-of-day or date axis label.
                "intraday": crate::sources::twelvedata::is_intraday(i),
            }))
            .collect::<Vec<_>>(),
        "rateLimitPerMinute": state.config.twelvedata.rate_limit_per_minute,
        "creditsAvailable": state.series_rate.available(),
        "reason": if !state.series_enabled {
            if !state.config.twelvedata.enabled {
                Some("disabled in config.toml ([twelvedata] enabled = false)")
            } else {
                Some("no [providers.twelvedata] api_key in the secrets file")
            }
        } else {
            None
        },
    }))
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
