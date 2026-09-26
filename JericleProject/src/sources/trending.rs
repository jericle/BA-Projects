//! Yahoo trending tickers, used as a fallback source of "what is moving today"
//! when the watchlist does not cover the story.

use anyhow::Result;
use serde::Deserialize;

const ENDPOINT: &str = "https://query1.finance.yahoo.com/v1/finance/trending/US";

#[derive(Debug, Deserialize)]
struct TrendingResponse {
    finance: FinanceEnvelope,
}

#[derive(Debug, Deserialize)]
struct FinanceEnvelope {
    result: Vec<TrendingResult>,
}

#[derive(Debug, Deserialize)]
struct TrendingResult {
    #[serde(default)]
    quotes: Vec<TrendingQuote>,
}

#[derive(Debug, Deserialize)]
struct TrendingQuote {
    symbol: String,
    #[serde(default, rename = "shortname")]
    shortname: Option<String>,
}

impl TrendingQuote {
    fn name(&self) -> Option<&str> {
        self.shortname.as_deref()
    }
}

pub async fn fetch_trending(client: &reqwest::Client, count: usize) -> Result<Vec<(String, String)>> {
    let url = format!("{ENDPOINT}?count={count}");
    let parsed: TrendingResponse = crate::http::get_json(client, &url).await?;
    Ok(parsed
        .finance
        .result
        .into_iter()
        .flat_map(|r| r.quotes)
        .map(|q| (q.symbol.to_ascii_uppercase(), q.name().unwrap_or("").to_string()))
        .collect())
}
