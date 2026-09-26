//! Per-ticker news from Yahoo's search endpoint.
//!
//! `GET /v1/finance/search?q={symbol}&newsCount=8&quotesCount=0`
//!
//! Items carry a unix publish time, a publisher and often `relatedTickers`,
//! which gives reliable ticker tagging for free.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;

const ENDPOINT: &str = "https://query1.finance.yahoo.com/v1/finance/search";

#[derive(Debug, Clone, Deserialize)]
pub struct RawNews {
    pub title: String,
    pub link: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(rename = "providerPublishTime", default)]
    pub published_ts: Option<i64>,
    #[serde(rename = "relatedTickers", default)]
    pub related_tickers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    news: Vec<RawNews>,
}

pub async fn fetch_ticker_news(
    client: &reqwest::Client,
    symbol: &str,
    count: usize,
) -> Result<Vec<RawNews>> {
    let url = format!(
        "{ENDPOINT}?q={symbol}&newsCount={count}&quotesCount=0&news_lang=en-US&region=US"
    );
    let parsed: SearchResponse = crate::http::get_json(client, &url).await?;
    Ok(parsed
        .news
        .into_iter()
        .filter(|n| !n.title.trim().is_empty())
        .map(|mut n| {
            // Yahoo occasionally omits the publisher for syndicated items.
            if n.publisher.as_deref().map(str::trim).unwrap_or("").is_empty() {
                n.publisher = Some("Yahoo Finance".to_string());
            }
            if n.related_tickers.is_empty() {
                n.related_tickers.push(symbol.to_ascii_uppercase());
            }
            n
        })
        .collect())
}

/// Fallback for a bad publish timestamp: assume it just landed.
pub fn fallback_ts() -> i64 {
    DateTime::<Utc>::from(std::time::SystemTime::now()).timestamp()
}
