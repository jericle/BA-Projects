//! Option chains from Yahoo's options endpoint.
//!
//! `v7/finance/options` rejects requests without a cookie + crumb pair, so this
//! holds a small session: hit `fc.yahoo.com` to pick up cookies, read the crumb
//! from `query2.finance.yahoo.com/v1/test/getcrumb`, then request the chain.
//! The crumb is cached and refreshed on demand. Per-expiry payloads are ~50 KB,
//! which is why this is preferred over the CBOE fallback.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::options::{build, pick_expiry, OptionRow, OptionSummary};

const CRUMB_URL: &str = "https://query2.finance.yahoo.com/v1/test/getcrumb";
const COOKIE_URL: &str = "https://fc.yahoo.com/";
const OPTIONS_URL: &str = "https://query2.finance.yahoo.com/v7/finance/options";

#[derive(Debug, Deserialize)]
struct OptionsResponse {
    #[serde(rename = "optionChain")]
    option_chain: OptionChain,
}

#[derive(Debug, Deserialize)]
struct OptionChain {
    result: Option<Vec<ChainResult>>,
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChainResult {
    #[serde(rename = "expirationDates")]
    expiration_dates: Vec<i64>,
    #[serde(default)]
    options: Vec<ExpiryOptions>,
    #[serde(default)]
    quote: Option<ChainQuote>,
}

#[derive(Debug, Deserialize)]
struct ChainQuote {
    #[serde(rename = "regularMarketPrice", default)]
    regular_market_price: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ExpiryOptions {
    #[serde(rename = "expirationDate", default)]
    expiration_date: Option<i64>,
    #[serde(default)]
    calls: Vec<Contract>,
    #[serde(default)]
    puts: Vec<Contract>,
}

#[derive(Debug, Clone, Deserialize)]
struct Contract {
    strike: f64,
    #[serde(rename = "openInterest", default)]
    open_interest: Option<i64>,
    #[serde(default)]
    volume: Option<i64>,
    // Needed by the strategy tab, which prices and scores real structures. A
    // contract with no two-sided quote, or with no implied volatility, cannot be
    // traded or modelled — and is rejected downstream rather than guessed at here.
    #[serde(default)]
    bid: Option<f64>,
    #[serde(default)]
    ask: Option<f64>,
    #[serde(rename = "impliedVolatility", default)]
    implied_volatility: Option<f64>,
}

impl Contract {
    fn row(&self) -> OptionRow {
        OptionRow {
            strike: self.strike,
            open_interest: self.open_interest.unwrap_or(0),
            volume: self.volume.unwrap_or(0),
        }
    }

    /// Full quote for the strategy engine. A missing field becomes zero so the type
    /// stays simple; `Quote::tradable` then rejects anything unusable.
    fn quote(&self) -> crate::strategy_build::Quote {
        crate::strategy_build::Quote {
            strike: self.strike,
            bid: self.bid.unwrap_or(0.0),
            ask: self.ask.unwrap_or(0.0),
            iv: self.implied_volatility.unwrap_or(0.0),
            open_interest: self.open_interest.unwrap_or(0),
            volume: self.volume.unwrap_or(0),
        }
    }
}

/// Cookie + crumb session. Cheap to clone-share: the crumb lives behind a lock.
pub struct YahooOptions {
    client: reqwest::Client,
    crumb: tokio::sync::RwLock<Option<String>>,
    /// Serialises the cookie + crumb handshake. The page loads one chain per KPI
    /// column at once, and without this they would each perform their own
    /// handshake on a cold start.
    crumb_lock: tokio::sync::Mutex<()>,
}

impl YahooOptions {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .user_agent(crate::http::USER_AGENT)
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .context("building yahoo options client")?;
        Ok(Self {
            client,
            crumb: tokio::sync::RwLock::new(None),
            crumb_lock: tokio::sync::Mutex::new(()),
        })
    }

    async fn crumb(&self, force: bool) -> Result<String> {
        if !force {
            if let Some(c) = self.crumb.read().await.as_ref() {
                return Ok(c.clone());
            }
        }
        // Double-checked: whoever wins the lock does the handshake, the rest
        // pick up the crumb it stored.
        let _guard = self.crumb_lock.lock().await;
        if !force {
            if let Some(c) = self.crumb.read().await.as_ref() {
                return Ok(c.clone());
            }
        }
        // fc.yahoo.com returns 404 but sets the A1/A3 cookies we need.
        let _ = self.client.get(COOKIE_URL).send().await;
        let text = self
            .client
            .get(CRUMB_URL)
            .send()
            .await
            .context("requesting yahoo crumb")?
            .text()
            .await
            .context("reading yahoo crumb")?;
        let text = text.trim().to_string();
        if text.is_empty() || text.contains("Too Many Requests") || text.contains("<") {
            return Err(anyhow!("yahoo crumb unavailable: {}", truncate(&text, 60)));
        }
        *self.crumb.write().await = Some(text.clone());
        Ok(text)
    }

    /// Chain for the current-month expiry of `symbol`, or the nearest one.
    pub async fn current_month_summary(
        &self,
        symbol: &str,
        now_et: &marketclock_et::Et,
    ) -> Result<OptionSummary> {
        // First call returns the nearest expiry plus the full expiry list.
        let first = self.fetch(symbol, None).await?;
        let (target_ts, is_current) = pick_expiry(&first.expiration_dates, now_et)
            .ok_or_else(|| anyhow!("no listed expiries for {symbol}"))?;

        let spot = first.spot;
        let rows = if target_ts == first.loaded_expiry {
            (first.calls, first.puts)
        } else {
            let chain = self.fetch(symbol, Some(target_ts)).await?;
            (chain.calls, chain.puts)
        };

        Ok(build(
            symbol,
            spot,
            target_ts,
            is_current,
            &rows.0,
            &rows.1,
            "yahoo",
        ))
    }

    /// A fully quoted chain for the strategy engine, at a chosen expiry.
    ///
    /// `expiry` is a unix timestamp as Yahoo lists it, i.e. midnight **UTC** — the
    /// same encoding `pick_expiry` works in. Passing one from `expiration_dates` is
    /// therefore safe, and reading the same value as ET would shift the day.
    pub async fn quoted_chain(
        &self,
        symbol: &str,
        expiry: Option<i64>,
    ) -> Result<crate::strategy_build::ChainSide> {
        let chain = self.fetch(symbol, expiry).await?;
        Ok(crate::strategy_build::ChainSide {
            calls: chain.call_quotes,
            puts: chain.put_quotes,
        })
    }

    /// Spot price and the list of listed expiries, for choosing a maturity.
    pub async fn expiries(&self, symbol: &str) -> Result<(f64, Vec<i64>)> {
        let chain = self.fetch(symbol, None).await?;
        Ok((chain.spot, chain.expiration_dates))
    }

    async fn fetch(&self, symbol: &str, date: Option<i64>) -> Result<RawChain> {
        // One retry: a cached crumb can go stale, which comes back as "Invalid Crumb".
        let mut last_err = None;
        for attempt in 0..2 {
            let crumb = self.crumb(attempt == 1).await?;
            let mut url = format!("{OPTIONS_URL}/{symbol}?crumb={crumb}");
            if let Some(d) = date {
                url.push_str(&format!("&date={d}"));
            }
            let body = self.client.get(&url).send().await;
            let text = match body {
                Ok(r) => r.text().await.unwrap_or_default(),
                Err(e) => {
                    last_err = Some(anyhow!("{e}"));
                    continue;
                }
            };
            if text.contains("Invalid Crumb") || text.contains("Unauthorized") {
                last_err = Some(anyhow!("yahoo rejected the crumb"));
                continue;
            }
            let parsed: OptionsResponse = serde_json::from_str(&text)
                .with_context(|| format!("decoding options for {symbol}: {}", truncate(&text, 120)))?;
            return RawChain::from_response(symbol, parsed);
        }
        Err(last_err.unwrap_or_else(|| anyhow!("could not load options for {symbol}")))
    }
}

struct RawChain {
    expiration_dates: Vec<i64>,
    loaded_expiry: i64,
    calls: Vec<OptionRow>,
    puts: Vec<OptionRow>,
    spot: f64,
    /// The same contracts carrying prices and implied volatility, for the strategy
    /// tab. The wall code only ever needs strike and open interest.
    call_quotes: Vec<crate::strategy_build::Quote>,
    put_quotes: Vec<crate::strategy_build::Quote>,
}

impl RawChain {
    fn from_response(symbol: &str, parsed: OptionsResponse) -> Result<Self> {
        let chain = parsed.option_chain;
        if let Some(err) = chain.error.as_ref().and_then(|e| e.get("description")) {
            return Err(anyhow!("yahoo options error: {err}"));
        }
        let mut result = chain
            .result
            .and_then(|mut r| if r.is_empty() { None } else { Some(r.remove(0)) })
            .ok_or_else(|| anyhow!("empty option chain for {symbol}"))?;

        let expiration_dates = std::mem::take(&mut result.expiration_dates);
        let spot = result
            .quote
            .as_ref()
            .and_then(|q| q.regular_market_price)
            .unwrap_or(0.0);

        let expiry = result
            .options
            .first()
            .and_then(|o| o.expiration_date)
            .ok_or_else(|| anyhow!("no contracts returned for {symbol}"))?;

        let options = result.options.remove(0);
        let calls = options.calls.iter().map(Contract::row).collect();
        let puts = options.puts.iter().map(Contract::row).collect();
        let call_quotes = options.calls.iter().map(Contract::quote).collect();
        let put_quotes = options.puts.iter().map(Contract::quote).collect();

        Ok(RawChain {
            expiration_dates,
            loaded_expiry: expiry,
            calls,
            puts,
            spot,
            call_quotes,
            put_quotes,
        })
    }
}

mod marketclock_et {
    pub use crate::marketclock::Et;
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}
