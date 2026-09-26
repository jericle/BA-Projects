//! Pre-market and intraday prices from Yahoo's chart endpoint.
//!
//! `GET /v8/finance/chart/{symbol}?interval=1m&range=1d&includePrePost=true`
//!
//! Returns one-minute bars from 04:00 ET (pre-market open) through "now" for the
//! current session, plus `meta.previousClose`. No API key required.

use anyhow::{Context, Result};
use chrono::{DateTime, Timelike};
use serde::Deserialize;

use crate::config::Config;
use crate::marketclock::{self, Et, PREMARKET_OPEN, CASH_OPEN};
use crate::model::{Quote, SparkPoint};

const ENDPOINT: &str = "https://query1.finance.yahoo.com/v8/finance/chart";

#[derive(Debug, Deserialize)]
struct ChartResponse {
    chart: ChartEnvelope,
}

#[derive(Debug, Deserialize)]
struct ChartEnvelope {
    #[serde(default)]
    result: Option<Vec<ChartResult>>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChartResult {
    meta: Meta,
    #[serde(default)]
    timestamp: Option<Vec<i64>>,
    #[serde(default)]
    indicators: Option<Indicators>,
}

#[derive(Debug, Deserialize)]
struct Meta {
    symbol: String,
    #[serde(rename = "shortName", default)]
    short_name: Option<String>,
    #[serde(rename = "longName", default)]
    long_name: Option<String>,
    #[serde(rename = "fullExchangeName", default)]
    full_exchange_name: Option<String>,
    #[serde(rename = "exchangeTimezoneName", default)]
    exchange_tz: Option<String>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(rename = "previousClose", default)]
    previous_close: Option<f64>,
    #[serde(rename = "chartPreviousClose", default)]
    chart_previous_close: Option<f64>,
    #[serde(rename = "regularMarketPrice", default)]
    regular_market_price: Option<f64>,
    #[serde(rename = "regularMarketVolume", default)]
    regular_market_volume: Option<f64>,
    #[serde(rename = "hasPrePostMarketData", default)]
    has_pre_post: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Indicators {
    #[serde(default)]
    quote: Vec<QuoteBars>,
}

#[derive(Debug, Deserialize)]
struct QuoteBars {
    #[serde(default)]
    close: Vec<Option<f64>>,
    #[serde(default)]
    high: Vec<Option<f64>>,
    #[serde(default)]
    low: Vec<Option<f64>>,
    #[serde(default)]
    volume: Vec<Option<f64>>,
}

struct Bar {
    ts: i64,
    et: Et,
    close: f64,
    high: Option<f64>,
    low: Option<f64>,
    volume: Option<f64>,
}

/// Fetch and derive pre-market state for one symbol.
pub async fn fetch_quote(
    client: &reqwest::Client,
    symbol: &str,
    cfg: &Config,
) -> Result<Quote> {
    let url = format!(
        "{ENDPOINT}/{symbol}?interval=1m&range=1d&includePrePost=true"
    );
    let parsed: ChartResponse = crate::http::get_json(client, &url)
        .await
        .with_context(|| format!("chart request for {symbol}"))?;

    if let Some(err) = parsed.chart.error {
        anyhow::bail!("yahoo chart error for {symbol}: {err}");
    }
    let result = parsed
        .chart
        .result
        .and_then(|mut r| if r.is_empty() { None } else { Some(r.remove(0)) })
        .with_context(|| format!("empty chart result for {symbol}"))?;

    let meta = result.meta;
    let now = marketclock::now_et();

    // Assemble the bar series, keeping only bars that actually have a close.
    let mut bars: Vec<Bar> = Vec::new();
    if let (Some(ts), Some(ind)) = (result.timestamp, result.indicators) {
        if let Some(q) = ind.quote.first() {
            for (i, &t) in ts.iter().enumerate() {
                let Some(close) = q.close.get(i).copied().flatten() else { continue };
                if !close.is_finite() || close <= 0.0 {
                    continue;
                }
                let Some(utc) = DateTime::from_timestamp(t, 0) else { continue };
                bars.push(Bar {
                    ts: t,
                    et: utc.with_timezone(&marketclock::ET),
                    close,
                    high: q.high.get(i).copied().flatten(),
                    low: q.low.get(i).copied().flatten(),
                    volume: q.volume.get(i).copied().flatten(),
                });
            }
        }
    }

    let prior_close = meta
        .previous_close
        .or(meta.chart_previous_close)
        .or_else(|| {
            // Fall back to the last regular-session bar of the series.
            bars.iter()
                .rev()
                .find(|b| {
                    let hm = (b.et.hour(), b.et.minute());
                    hm >= CASH_OPEN && hm < marketclock::CASH_CLOSE && b.ts < now.timestamp()
                })
                .map(|b| b.close)
        })
        .with_context(|| format!("no previous close for {symbol}"))?;

    // The session this data belongs to: today in ET if it is a trading day,
    // otherwise whatever day the bars themselves are stamped with.
    let data_date = bars
        .iter()
        .map(|b| marketclock::et_date(&b.et))
        .max()
        .unwrap_or_else(|| marketclock::session_date(&now));
    let today_et = marketclock::et_date(&now);
    let premarket_window = marketclock::is_trading_day(today_et) && data_date == today_et;

    let in_premarket = |b: &Bar| {
        marketclock::et_date(&b.et) == data_date && (b.et.hour(), b.et.minute()) >= PREMARKET_OPEN
            && (b.et.hour(), b.et.minute()) < CASH_OPEN
    };

    let pm: Vec<&Bar> = bars.iter().filter(|b| in_premarket(b)).collect();
    let (spark_bars, has_premarket_bars) =
        if pm.is_empty() { (bars.iter().collect::<Vec<_>>(), false) } else { (pm.clone(), true) };

    let last = bars
        .last()
        .map(|b| b.close)
        .or(meta.regular_market_price)
        .unwrap_or(prior_close);

    let premarket_last = if has_premarket_bars { pm.last().map(|b| b.close) } else { None };
    let premarket_first = if has_premarket_bars { pm.first().map(|b| b.close) } else { None };

    let (premarket_high, premarket_low) = if has_premarket_bars {
        let highs: Vec<f64> = pm.iter().filter_map(|b| b.high).collect();
        let lows: Vec<f64> = pm.iter().filter_map(|b| b.low).collect();
        (
            highs.iter().copied().reduce(f64::max),
            lows.iter().copied().reduce(f64::min),
        )
    } else {
        (None, None)
    };

    let premarket_vwap = if has_premarket_bars { volume_weighted(&pm) } else { None };

    // Volume so far this session. Yahoo emits null/0 for minutes with no prints, and
    // reports 0 for every pre-market bar, so this is only non-zero once the cash
    // session is running. `regularMarketVolume` covers the pre-open case.
    let volume: f64 = bars.iter().filter_map(|b| b.volume).sum();
    let premarket_prints = pm.len();

    let gap_pct = premarket_last.map(|p| pct(p, prior_close));
    let session_pct = match (premarket_first, premarket_last) {
        (Some(f), Some(l)) if f > 0.0 => Some(pct(l, f)),
        _ => None,
    };

    let name = meta
        .long_name
        .or(meta.short_name)
        .unwrap_or_else(|| symbol.to_string());

    let spark = downsample(
        spark_bars
            .iter()
            .map(|b| SparkPoint { t: b.ts, p: b.close })
            .collect(),
        60,
    );

    let _ = (meta.full_exchange_name, meta.exchange_tz, meta.has_pre_post);

    Ok(Quote {
        symbol: meta.symbol.to_ascii_uppercase(),
        name,
        group: cfg.group_of(symbol),
        currency: meta.currency.unwrap_or_else(|| "USD".to_string()),
        prior_close,
        last,
        change_pct: pct(last, prior_close),
        gap_pct,
        session_pct,
        premarket_last,
        premarket_vwap,
        premarket_high,
        premarket_low,
        premarket: has_premarket_bars && premarket_window,
        session_date: data_date.to_string(),
        spark,
        volume,
        regular_volume: meta.regular_market_volume.unwrap_or(volume),
        premarket_prints,
        news_count: 0,
        sentiment_avg: None,
        error: None,
    })
}

fn pct(value: f64, base: f64) -> f64 {
    if base <= 0.0 {
        0.0
    } else {
        (value / base - 1.0) * 100.0
    }
}

/// Volume-weighted average where volume exists, plain mean of closes where it does not.
fn volume_weighted(bars: &[&Bar]) -> Option<f64> {
    let mut pv = 0.0;
    let mut vol = 0.0;
    let mut sum = 0.0;
    for b in bars {
        sum += b.close;
        if let Some(v) = b.volume.filter(|v| *v > 0.0) {
            pv += b.close * v;
            vol += v;
        }
    }
    if vol > 0.0 {
        Some(pv / vol)
    } else if !bars.is_empty() {
        Some(sum / bars.len() as f64)
    } else {
        None
    }
}

/// Evenly thin a series down to at most `max` points, always keeping the last point.
fn downsample(points: Vec<SparkPoint>, max: usize) -> Vec<SparkPoint> {
    if points.len() <= max {
        return points;
    }
    let step = points.len() as f64 / max as f64;
    let mut out = Vec::with_capacity(max + 1);
    for i in 0..max {
        out.push(points[(i as f64 * step).floor() as usize].clone());
    }
    if let Some(last) = points.last() {
        out.push(last.clone());
    }
    out
}
