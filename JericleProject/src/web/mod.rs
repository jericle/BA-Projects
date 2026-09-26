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
        .route("/api/strategies/{symbol}", get(api_strategies))
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

/// A strategy candidate, shaped for the browser.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateView {
    name: String,
    bias: String,
    rationale: String,
    legs: Vec<crate::strategy::Leg>,
    metrics: crate::strategy::Metrics,
    /// The sampled payoff curve, ready to plot.
    curve: Vec<crate::strategy::PayoffPoint>,
    score: f64,
    entry_cost: f64,
    notes: Vec<String>,
}

/// Annualised risk-free rate, from the environment so it can move without a rebuild.
///
/// Defaults to 4%, the right order of magnitude for USD. Over a few weeks the
/// discount term is a rounding error beside the volatility term, so this does not
/// drive any conclusion on the tab.
fn risk_free_rate() -> f64 {
    std::env::var("OPENDASH_RISK_FREE")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|r| r.is_finite() && r.abs() < 1.0)
        .unwrap_or(0.04)
}

/// Payoff-curve bounds across every candidate for one symbol: wide enough to show a
/// wide structure's plateau, narrow enough that the interesting region is not
/// squeezed into a few pixels.
fn curve_bounds(spot: f64, cands: &[crate::strategy_build::Candidate]) -> (f64, f64) {
    let mut lo = spot;
    let mut hi = spot;
    for c in cands {
        for l in &c.strategy.legs {
            lo = lo.min(l.strike);
            hi = hi.max(l.strike);
        }
    }
    // Pad outside the outermost strikes so the flat regions are visible, and keep a
    // sane band even when a structure's strikes sit far from spot.
    let pad = ((hi - lo) * 0.25).max(spot * 0.05);
    ((lo - pad).max(0.0), hi + pad)
}

fn err_response(
    status: axum::http::StatusCode,
    msg: String,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    (status, axum::Json(json!({ "ok": false, "error": msg })))
}

#[derive(Debug, serde::Deserialize)]
pub struct StrategyQuery {
    /// Yahoo's expiry encoding: unix seconds at midnight UTC.
    expiry: Option<String>,
}

/// Ranked defined-risk candidates for one symbol at one expiry.
///
/// The expiry is optional; without it the current-month expiry is used, which is the
/// same one the option-walls card shows, so the two tabs agree by default.
async fn api_strategies(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
    axum::extract::Query(q): axum::extract::Query<StrategyQuery>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, axum::Json<serde_json::Value>)> {
    use crate::strategy_build as sb;

    let symbol = clean_symbol(&symbol)
        .map_err(|st| (st, axum::Json(json!({ "ok": false, "error": "bad symbol" }))))?;
    let et = crate::marketclock::now_et();
    let now_ts = et.timestamp();

    let (spot, expiries) = state
        .options
        .expiries(&symbol)
        .await
        .map_err(|e| err_response(axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Only unexpired contracts are offered: a structure on a dead contract has a
    // probability of profit that is already decided, not modelled.
    let upcoming: Vec<i64> = expiries.iter().cloned().filter(|ts| *ts > now_ts).collect();
    if upcoming.is_empty() {
        return Ok(axum::Json(json!({
            "ok": true, "symbol": symbol, "spot": spot, "candidates": [], "expiries": [],
            "reason": "no unexpired contracts listed for this symbol",
        })));
    }

    let chosen = q
        .expiry
        .and_then(|e| e.parse::<i64>().ok())
        .filter(|e| upcoming.contains(e))
        .or_else(|| crate::options::pick_expiry(&expiries, &et).map(|(ts, _)| ts))
        .unwrap_or(upcoming[0]);

    let chain = state
        .options
        .quoted_chain(&symbol, Some(chosen))
        .await
        .map_err(|e| err_response(axum::http::StatusCode::BAD_GATEWAY, e.to_string()))?;

    // Yahoo lists an expiry as midnight UTC, but a structure is settled at the
    // 09:30 ET close. Measuring to midnight instead shortens every probability by
    // most of a day.
    let settle = chosen + 13 * 3600 + 1800;
    let t_years = ((settle - now_ts) as f64 / (365.25 * 86_400.0)).max(0.0);

    let u = sb::Underlying {
        symbol: symbol.clone(),
        spot,
        expiry_ts: chosen,
        expiry: chrono::DateTime::from_timestamp(chosen, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        t_years,
        rate: risk_free_rate(),
        chain,
    };

    let built = sb::build_candidates(&u);
    let (lo, hi) = curve_bounds(spot, &built);

    let views: Vec<CandidateView> = built
        .iter()
        .map(|c| CandidateView {
            name: c.strategy.name.clone(),
            bias: c.strategy.bias.as_str().to_string(),
            rationale: c.strategy.rationale.clone(),
            legs: c.strategy.legs.clone(),
            metrics: c.metrics.clone(),
            curve: crate::strategy::payoff_curve(&c.strategy.legs, lo, hi, 121),
            score: c.score,
            entry_cost: c.entry_cost,
            notes: c.notes.clone(),
        })
        .collect();

    let expiry_labels: Vec<String> = upcoming
        .iter()
        .filter_map(|t| {
            chrono::DateTime::from_timestamp(*t, 0).map(|d| d.format("%Y-%m-%d").to_string())
        })
        .collect();

    Ok(axum::Json(json!({
        "ok": true,
        "symbol": symbol,
        "spot": spot,
        "expiry": u.expiry,
        "expiries": expiry_labels,
        "daysToExpiry": (((chosen - now_ts).max(0) as f64) / 86_400.0 * 10.0).round() as i64 / 10,
        "curve": { "lo": lo, "hi": hi },
        "candidates": views,
    })))
}

fn json_ok(v: serde_json::Value) -> axum::Json<serde_json::Value> {
    axum::Json(v)
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
