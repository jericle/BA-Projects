//! Serialisable types shared by the fetch pipeline, the HTTP API and the UI.

use serde::{Deserialize, Serialize};

/// One point of a price sparkline. `t` is a unix timestamp, `p` a price.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SparkPoint {
    pub t: i64,
    pub p: f64,
}

/// Pre-market / latest price state for one watchlist symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub symbol: String,
    pub name: String,
    pub group: String,
    pub currency: String,
    pub prior_close: f64,
    pub last: f64,
    /// `last` versus prior close, in percent.
    pub change_pct: f64,
    /// Pre-market last versus prior close, in percent. `None` outside pre-market.
    pub gap_pct: Option<f64>,
    /// Pre-market first bar to last bar, in percent.
    pub session_pct: Option<f64>,
    pub premarket_last: Option<f64>,
    pub premarket_vwap: Option<f64>,
    pub premarket_high: Option<f64>,
    pub premarket_low: Option<f64>,
    pub premarket: bool,
    pub session_date: String,
    pub spark: Vec<SparkPoint>,
    /// Traded volume summed from this session's 1-minute bars. Yahoo reports zero
    /// for every pre-market bar, so this is only meaningful once the cash session
    /// has opened.
    pub volume: f64,
    /// `meta.regularMarketVolume`: the running session volume while the market is
    /// open, otherwise the last completed session's volume. This is the honest
    /// pre-open liquidity baseline.
    pub regular_volume: f64,
    /// Pre-market minutes that had a print. Not volume, but the only pre-open
    /// activity signal the feed exposes.
    pub premarket_prints: usize,
    pub news_count: usize,
    pub sentiment_avg: Option<f64>,
    pub error: Option<String>,
}

/// Why a headline scored the way it did. Surfaced in the UI so the number is never a black box.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    pub recency: f64,
    pub cluster: f64,
    pub impact: f64,
    pub source: f64,
    pub watchlist: f64,
    pub impact_hits: Vec<String>,
    pub cluster_size: usize,
    pub outlets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub source: String,
    pub url: String,
    pub published_ts: i64,
    pub published_et: String,
    pub age_minutes: i64,
    pub tickers: Vec<String>,
    pub sentiment: f64,
    pub salience: f64,
    pub breakdown: ScoreBreakdown,
    /// Stories sharing a cluster id are the same story. Internal bookkeeping:
    /// the UI only needs `breakdown.cluster_size` and `breakdown.outlets`.
    #[serde(skip)]
    pub cluster_id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mover {
    pub symbol: String,
    pub gap_pct: f64,
    pub last: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseInfo {
    pub phase: String,
    pub is_trading_day: bool,
    pub session_date: String,
    pub market_open_et: String,
    pub countdown_seconds: i64,
    pub et_now: String,
    pub local_now: String,
    pub local_tz: String,
}

/// The full payload behind `GET /api/dashboard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub generated_ts: i64,
    pub generated_et: String,
    pub generated_local: String,
    pub phase: PhaseInfo,
    pub watchlist: Vec<Quote>,
    pub news: Vec<NewsItem>,
    pub all_news_count: usize,
    pub gainers: Vec<Mover>,
    pub losers: Vec<Mover>,
    /// Yahoo's trending-US list, i.e. what is moving regardless of the watchlist.
    pub trending: Vec<String>,
    pub warnings: Vec<String>,
    pub news_queries: Vec<String>,
}
