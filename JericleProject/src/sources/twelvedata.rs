//! Twelve Data `time_series`: OHLC bars at a chosen interval.
//!
//! Three things about this upstream drive the shape of the module.
//!
//! **The key is a header, not a query parameter.** `Authorization: apikey <key>`
//! is supported and is what we use, so the key never appears in a URL — and so it
//! cannot land in an access log, a `Referer`, or an `anyhow` error that echoes the
//! request line.
//!
//! **The free tier allows 8 credits per minute, one credit per symbol.** A
//! 22-symbol watchlist therefore cannot be refetched on the dashboard's 30s poll.
//! So this is strictly on-demand, one symbol at a time, behind a token bucket that
//! refuses rather than overrunning the quota, and behind a cache keyed by
//! symbol + interval. Twelve Data returns HTTP 200 with `{"status":"error", ...}`
//! for a quota breach, so the limiter is what prevents the failure, not the
//! response code.
//!
//! **Values arrive as strings, newest first, with two different timestamp shapes.**
//! Intraday intervals give `2026-09-25 15:59:00`; `1day`, `1week` and `1month` give
//! a bare `2026-09-01` and ignore the `timezone` parameter entirely. Both are naive
//! wall-clock, so they are localised to ET explicitly rather than assumed to be UTC.

use anyhow::{anyhow, Context, Result};
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};

use crate::http::USER_AGENT;
use crate::marketclock::Et;

/// Provider name as it appears in the secrets file's `[providers.*]` table.
pub const PROVIDER: &str = "twelvedata";

/// Every interval Twelve Data accepts, with a default bar count chosen so each
/// chart is legible rather than merely large. 1min over 5000 bars is a smear;
/// 1month over 30 bars is a stub. Overridable per request, clamped to 1..=5000.
pub const INTERVALS: &[(&str, usize)] = &[
    ("1min", 390),   // one 6.5h session
    ("5min", 390),   // ~3 sessions
    ("15min", 300),  // ~3 sessions
    ("30min", 300),  // ~6 sessions
    ("45min", 260),  // ~2 months
    ("1h", 250),     // ~6 weeks
    ("2h", 240),     // ~2 months
    ("4h", 200),     // ~3 months
    ("8h", 180),     // ~1 year
    ("1day", 180),   // ~9 months
    ("1week", 156),  // 3 years
    ("1month", 120), // 10 years
];

/// True when `interval` is one Twelve Data actually serves. The API answers an
/// unknown interval with HTTP 400, but rejecting it here keeps a typo in the URL
/// from becoming an upstream round trip.
pub fn is_valid_interval(interval: &str) -> bool {
    INTERVALS.iter().any(|(i, _)| *i == interval)
}

pub fn default_outputsize(interval: &str) -> usize {
    INTERVALS
        .iter()
        .find(|(i, _)| *i == interval)
        .map(|(_, n)| *n)
        .unwrap_or(120)
}

/// Intervals of a day or more come back date-only and ignore `timezone`.
///
/// The UI needs this to pick an axis formatter: a time-of-day label is right for
/// intraday and meaningless for a daily bar, where the date *is* the label.
pub fn is_intraday(interval: &str) -> bool {
    matches!(
        interval,
        "1min" | "5min" | "15min" | "30min" | "45min" | "1h" | "2h" | "4h" | "8h"
    )
}

/// One OHLC bar. `t` is a unix timestamp, already localised to ET.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OhlcBar {
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
}

/// What the browser receives for one symbol at one interval. Deliberately carries
/// no key, no upstream URL and no account detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub symbol: String,
    pub interval: String,
    pub bars: Vec<OhlcBar>,
    pub currency: String,
    pub exchange: String,
    /// Bars that were dropped because a field did not parse. Surfaced rather than
    /// hidden: a silently short chart reads as "that is all there was".
    pub skipped: usize,
    /// True when served from cache without touching the API.
    pub cached: bool,
    /// Epoch seconds the underlying fetch happened, so a stale chart is visible.
    pub fetched_ts: i64,
}

// ------------------------------------------------------------------ upstream

#[derive(Debug, Deserialize)]
struct RawSeries {
    #[serde(default)]
    meta: Option<RawMeta>,
    #[serde(default)]
    values: Option<Vec<RawBar>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    // A single-symbol response is the object above; a batch response is a map of
    // symbol -> that same object. Both land here when deserialising a bare object,
    // because serde would otherwise reject the unknown `AAPL` key.
    #[serde(flatten)]
    others: std::collections::BTreeMap<String, RawSeries>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMeta {
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawBar {
    #[serde(default)]
    datetime: String,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    high: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    close: Option<String>,
    #[serde(default)]
    volume: Option<String>,
}

/// Turn one raw bar into an `OhlcBar`, or explain why it cannot.
///
/// Twelve Data documents that fields may be `null` when data is unavailable, and
/// its floats carry representation noise (`"225.080002"`), so every field goes
/// through a parse rather than a cast.
fn parse_bar(raw: &RawBar) -> std::result::Result<OhlcBar, String> {
    let num = |v: &Option<String>, what: &str| -> std::result::Result<f64, String> {
        let s = v.as_deref().unwrap_or("").trim();
        if s.is_empty() {
            return Err(format!("{what} is null"));
        }
        s.parse::<f64>()
            .map_err(|_| format!("{what}={s:?} is not a number"))
    };

    let c = num(&raw.close, "close")?;
    let o = num(&raw.open, "open")?;
    let h = num(&raw.high, "high")?;
    let l = num(&raw.low, "low")?;
    // Volume is optional in the upstream contract; a missing volume is not a
    // reason to throw away a usable price bar.
    let v = num(&raw.volume, "volume").unwrap_or(0.0);

    // Two shapes: intraday `YYYY-MM-DD HH:MM:SS`, and a bare date for 1day and up.
    // Both are exchange wall-clock, so both are localised to ET here rather than
    // assumed to be UTC — assuming UTC is the classic reason a chart lands hours
    // away from the session it is supposed to show.
    let dt = raw.datetime.trim();
    let naive = if dt.contains(' ') {
        NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(dt, "%Y-%m-%dT%H:%M:%S"))
            .map_err(|_| format!("datetime={dt:?} is not a recognised timestamp"))?
    } else {
        NaiveDate::parse_from_str(dt, "%Y-%m-%d")
            .map_err(|_| format!("datetime={dt:?} is not a recognised date"))?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| format!("datetime={dt:?} is not a valid date"))?
    };

    let et: Et = crate::marketclock::ET
        .from_local_datetime(&naive)
        .single()
        .ok_or_else(|| format!("datetime={dt:?} is ambiguous or does not exist in ET (DST gap)"))?;

    Ok(OhlcBar {
        t: et.timestamp(),
        o,
        h,
        l,
        c,
        v,
    })
}

/// Normalise a decoded response: sort chronologically and drop unusable bars.
fn normalise(raw: RawSeries) -> std::result::Result<(Vec<OhlcBar>, usize, RawMeta), String> {
    let meta = raw.meta.clone().unwrap_or(RawMeta {
        currency: None,
        exchange: None,
    });
    let values = raw.values.clone().unwrap_or_default();

    let mut bars = Vec::with_capacity(values.len());
    let mut skipped = 0usize;
    for b in &values {
        match parse_bar(b) {
            // `order=asc` is requested upstream, but a batch reply or a cached
            // payload can still arrive reversed; sorting here makes the chart
            // independent of that rather than trusting it.
            Ok(bar) => bars.push(bar),
            Err(_) => skipped += 1,
        }
    }
    bars.sort_by_key(|b| b.t);
    bars.dedup_by_key(|b| b.t);
    Ok((bars, skipped, meta))
}

/// Build the request URL. The key is deliberately absent: it goes in a header.
fn series_url(base: &str, symbol: &str, interval: &str, outputsize: usize) -> String {
    // order=asc so the chart is already chronological; dp keeps the payload small
    // without losing precision that matters (dp=5 still resolves a $0.01 tick).
    format!(
        "{base}/time_series?symbol={}&interval={}&outputsize={}&order=asc&dp=5\
         &timezone=America%2FNew_York&prepost=false",
        urlencode(symbol),
        urlencode(interval),
        outputsize
    )
}

/// Twelve Data is permissive about these, but a symbol reaches us from a URL path
/// segment and a free-text query, so it is encoded rather than trusted.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fetch one symbol at one interval. `api_key` is sent as a header only.
pub async fn fetch_series(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    symbol: &str,
    interval: &str,
    outputsize: usize,
) -> Result<Series> {
    let url = series_url(base_url, symbol, interval, outputsize);
    let resp = client
        .get(&url)
        .header(reqwest::header::AUTHORIZATION, format!("apikey {api_key}"))
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .with_context(|| format!("GET time_series for {symbol} ({interval})"))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .with_context(|| format!("reading time_series body for {symbol}"))?;

    // The body is parsed before the status is judged: Twelve Data reports quota
    // and bad-key failures as HTTP 200 with a `status: "error"` body, and that
    // message is far more useful than "HTTP 200".
    let raw: RawSeries = serde_json::from_str(&text)
        .with_context(|| format!("decoding time_series json for {symbol}"))?;

    if raw.status.as_deref() == Some("error") {
        let code = raw.code.unwrap_or(0);
        let msg = raw.message.clone().unwrap_or_default();
        return Err(match code {
            401 | 403 => anyhow!("Twelve Data rejected the API key ({code}): {msg}"),
            429 => anyhow!("Twelve Data rate limit hit ({code}): {msg}"),
            other => anyhow!("Twelve Data error {other}: {msg}"),
        });
    }
    if !status.is_success() {
        return Err(anyhow!("time_series for {symbol} -> HTTP {status}"));
    }

    // A batch reply nests one series per symbol; take the one that was asked for.
    let chosen = raw
        .others
        .get(symbol)
        .or_else(|| {
            raw.others
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(symbol))
                .map(|(_, v)| v)
        });
    let target = match chosen {
        Some(s) => {
            // Re-attach the parent's meta/status: a nested entry is bare.
            RawSeries {
                meta: raw.meta.clone().or_else(|| s.meta.clone()),
                values: s.values.clone(),
                status: s.status.clone().or_else(|| raw.status.clone()),
                code: s.code,
                message: s.message.clone(),
                others: Default::default(),
            }
        }
        None => raw,
    };

    let (bars, skipped, meta) = normalise(target)
        .map_err(|e| anyhow!("Twelve Data payload for {symbol}: {e}"))?;

    if bars.is_empty() {
        return Err(anyhow!(
            "Twelve Data returned no usable bars for {symbol} at {interval} \
             (symbol unknown to the feed, or interval unsupported for it)"
        ));
    }

    Ok(Series {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        bars,
        currency: meta.currency.unwrap_or_default(),
        exchange: meta.exchange.unwrap_or_default(),
        skipped,
        cached: false,
        fetched_ts: chrono::Utc::now().timestamp(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(dt: &str, close: &str) -> RawBar {
        RawBar {
            datetime: dt.into(),
            open: Some("100".into()),
            high: Some("110".into()),
            low: Some("90".into()),
            close: Some(close.into()),
            volume: Some("1000".into()),
        }
    }

    /// A normal 1min bar, 15:59 ET, must land on the right epoch. 2026-09-25 is
    /// EDT (UTC-4), so 15:59 ET is 19:59 UTC.
    #[test]
    fn parses_an_intraday_bar_into_et() {
        let b = parse_bar(&bar("2026-09-25 15:59:00", "225.08")).unwrap();
        assert_eq!(b.c, 225.08);
        assert_eq!(b.t, chrono::DateTime::parse_from_rfc3339("2026-09-25T19:59:00Z").unwrap().timestamp());
    }

    /// The 1day/1week/1month shape has no time part. Reading it as UTC — the
    /// obvious mistake — would put every daily bar on the wrong date for a
    /// negative-offset zone, and silently shift weekly bars by a day.
    #[test]
    fn parses_a_date_only_bar_as_et_midnight() {
        let b = parse_bar(&bar("2026-09-01", "341.07")).unwrap();
        let got = chrono::DateTime::from_timestamp(b.t, 0).unwrap();
        let et = got.with_timezone(&crate::marketclock::ET);
        assert_eq!(et.format("%Y-%m-%d %H:%M").to_string(), "2026-09-01 00:00");
    }

    /// Twelve Data's floats carry binary-representation noise. Parsing must not
    /// surface `225.080002` as a visibly wrong price.
    #[test]
    fn tolerates_float_representation_noise() {
        let b = parse_bar(&bar("2026-09-25 15:59:00", "225.080002")).unwrap();
        assert!((b.c - 225.08).abs() < 1e-4, "got {}", b.c);
    }

    #[test]
    fn a_null_field_drops_the_bar_with_a_reason() {
        let mut b = bar("2026-09-25 15:59:00", "225.08");
        b.close = None;
        let err = parse_bar(&b).unwrap_err();
        assert!(err.contains("close"), "{err}");
    }

    /// The docs say fields may be null when data is unavailable; a missing volume
    /// is documented as normal and must not cost us a good price bar.
    #[test]
    fn a_missing_volume_keeps_the_bar() {
        let mut b = bar("2026-09-25 15:59:00", "225.08");
        b.volume = None;
        let bar = parse_bar(&b).unwrap();
        assert_eq!(bar.v, 0.0);
        assert_eq!(bar.c, 225.08);
    }

    #[test]
    fn an_unparseable_price_is_reported_not_zeroed() {
        let b = parse_bar(&bar("2026-09-25 15:59:00", "n/a")).unwrap_err();
        assert!(b.contains("not a number"), "{b}");
    }

    /// The spring-forward hour does not exist in ET. `single()` returns None and
    /// the bar is skipped rather than silently shifted by an hour.
    #[test]
    fn a_datetime_in_the_dst_gap_is_rejected() {
        let b = parse_bar(&bar("2026-03-08 02:30:00", "100")).unwrap_err();
        assert!(b.contains("DST") || b.contains("ambiguous"), "{b}");
    }

    /// Upstream is asked for asc, but a reversed or cached payload must still
    /// chart correctly rather than drawing right-to-left.
    #[test]
    fn bars_are_sorted_oldest_first_regardless_of_input_order() {
        let raw = RawSeries {
            meta: None,
            values: Some(vec![
                bar("2026-09-25 15:59:00", "3"),
                bar("2026-09-25 15:57:00", "1"),
                bar("2026-09-25 15:58:00", "2"),
            ]),
            status: Some("ok".into()),
            code: None,
            message: None,
            others: Default::default(),
        };
        let (bars, skipped, _) = normalise(raw).unwrap();
        assert_eq!(skipped, 0);
        let closes: Vec<f64> = bars.iter().map(|b| b.c).collect();
        assert_eq!(closes, vec![1.0, 2.0, 3.0]);
    }

    /// A duplicate timestamp means a double-counted bar; the later one wins so the
    /// count matches the raw feed.
    #[test]
    fn duplicate_timestamps_are_collapsed() {
        let raw = RawSeries {
            meta: None,
            values: Some(vec![
                bar("2026-09-25 15:58:00", "2"),
                bar("2026-09-25 15:58:00", "2"),
            ]),
            status: Some("ok".into()),
            code: None,
            message: None,
            others: Default::default(),
        };
        let (bars, _, _) = normalise(raw).unwrap();
        assert_eq!(bars.len(), 1);
    }

    /// Skipped bars are counted, not swallowed: a chart that quietly lost half its
    /// points is worse than one that says so.
    #[test]
    fn skipped_bars_are_counted() {
        let raw = RawSeries {
            meta: None,
            values: Some(vec![
                bar("2026-09-25 15:59:00", "3"),
                bar("2026-09-25 15:58:00", "oops"),
            ]),
            status: Some("ok".into()),
            code: None,
            message: None,
            others: Default::default(),
        };
        let (bars, skipped, _) = normalise(raw).unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn the_api_key_never_appears_in_the_url() {
        let url = series_url("https://api.twelvedata.com", "NVDA", "1min", 300);
        assert!(!url.contains("apikey"), "{url}");
        assert!(!url.contains("key="), "{url}");
        assert!(url.contains("symbol=NVDA"));
        assert!(url.contains("order=asc"));
    }

    /// The symbol arrives from a URL path segment, so it must be encoded rather
    /// than interpolated raw into the query string.
    #[test]
    fn odd_symbols_are_urlencoded() {
        let url = series_url("https://x", "A B&C=d", "1min", 10);
        assert!(url.contains("symbol=A%20B%26C%3Dd"), "{url}");
    }

    #[test]
    fn known_intervals_are_accepted_and_junk_is_not() {
        assert!(is_valid_interval("1min"));
        assert!(is_valid_interval("1month"));
        assert!(!is_valid_interval("0.99min"), "the API's own bad example");
        assert!(!is_valid_interval("1sec"));
        assert!(!is_valid_interval(""));
    }

    /// 1day and up come back date-only, which is the whole reason the parser has
    /// two shapes. Getting this boundary wrong silently mislabels every axis.
    #[test]
    fn intraday_classification_matches_the_two_timestamp_shapes() {
        for i in ["1min", "5min", "15min", "30min", "45min", "1h", "2h", "4h", "8h"] {
            assert!(is_intraday(i), "{i} should be intraday");
        }
        for i in ["1day", "1week", "1month"] {
            assert!(!is_intraday(i), "{i} should not be intraday");
        }
    }

    #[test]
    fn every_interval_has_a_sane_default_depth() {
        for (i, n) in INTERVALS {
            assert!((1..=5000).contains(n), "{i} default {n} is out of range");
        }
        // Finer intervals need more bars to cover any useful span.
        assert!(default_outputsize("1min") > default_outputsize("1day"));
    }
}
