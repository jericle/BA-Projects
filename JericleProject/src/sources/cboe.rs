//! Fallback option chain from Cboe's consolidated OPRA delayed feed.
//!
//! Used only when the Yahoo crumb flow fails. The payload is much larger (0.8-5 MB
//! depending on the name) so this is strictly on-demand and cached by the caller.
//! Expiries come from the OCC contract symbol, e.g. `NVDA260925C00050000` is
//! NVDA, expiring 2026-09-25, call, strike 500.00. Roots shorter than four
//! characters are padded with a filler letter, so the date is located by pattern
//! rather than by a fixed offset.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::options::{build, pick_expiry, OptionRow, OptionSummary};

const ENDPOINT: &str = "https://cdn.cboe.com/api/global/delayed_quotes/options";

#[derive(Debug, Deserialize)]
struct CboeResponse {
    data: CboeData,
}

#[derive(Debug, Deserialize)]
struct CboeData {
    symbol: String,
    #[serde(rename = "current_price", default)]
    current_price: Option<f64>,
    #[serde(default)]
    options: Vec<CboeContract>,
}

#[derive(Debug, Deserialize)]
struct CboeContract {
    option: String,
    // Cboe emits these as JSON floats ("open_interest": 5.0), which serde will not
    // coerce into an integer, so they are read as f64 and rounded.
    #[serde(rename = "open_interest", default)]
    open_interest: Option<f64>,
    volume: Option<f64>,
}

impl CboeContract {
    fn row(&self) -> OptionRow {
        OptionRow {
            strike: 0.0, // filled in by the caller, which knows the OCC symbol
            open_interest: self.open_interest.unwrap_or(0.0).round() as i64,
            volume: self.volume.unwrap_or(0.0).round() as i64,
        }
    }
}

/// `NVDA260925C00050000` -> (expiry 2026-09-25, call, strike 500.00)
/// `CEGG260925P00100000` -> root "CEGG", expiry 2026-09-25, put, strike 100.00
fn parse_occ(symbol: &str) -> Option<(i64, bool, f64)> {
    let bytes = symbol.as_bytes();
    // Locate the 6-digit date immediately followed by C or P. OCC pads a root to an
    // even length, so the date offset varies by symbol: MU (2) starts at index 2,
    // CEG (3) is padded to CEGG (4) and starts at index 4, NVDA (4) at index 4.
    // Roots are letters only, so the first all-digit run in the right position is
    // unambiguously the date.
    // Layout: root(>=2) + YYMMDD(6) + C|P(1) + strike(8) => at least 17 bytes.
    let last_start = bytes.len().checked_sub(15)?;
    for i in 2..=last_start {
        if !bytes[i].is_ascii_digit() {
            continue;
        }
        let date = symbol.get(i..i + 6)?;
        if !date.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let side = *bytes.get(i + 6)?;
        if side != b'C' && side != b'P' {
            continue;
        }
        let strike_digits = symbol.get(i + 7..i + 15)?;
        if !strike_digits.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let yy: i64 = date[0..2].parse().ok()?;
        let mm: i64 = date[2..4].parse().ok()?;
        let dd: i64 = date[4..6].parse().ok()?;
        // The 8-digit strike field is in thousandths: 00050000 -> 50.00.
        let strike: f64 = strike_digits.parse::<f64>().ok()? / 1000.0;
        let expiry_ts = et_epoch_from_occ(yy, mm as u32, dd as u32)?;
        return Some((expiry_ts, side == b'C', strike));
    }
    None
}

/// Two-digit OCC year to four digits. 69-99 are the 1900s, 00-68 the 2000s.
fn et_epoch_from_occ(yy: i64, mm: u32, dd: u32) -> Option<i64> {
    let year = if yy >= 69 { 1900 + yy } else { 2000 + yy };
    crate::marketclock::et_epoch(year as i32, mm, dd)
}

pub async fn current_month_summary(
    client: &reqwest::Client,
    symbol: &str,
    now_et: &crate::marketclock::Et,
) -> Result<OptionSummary> {
    let url = format!("{ENDPOINT}/{}.json", symbol.to_ascii_uppercase());
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("cboe request for {symbol}"))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let body = resp.bytes().await.context("reading cboe payload")?;
    if !status.is_success() {
        anyhow::bail!("cboe HTTP {status} for {symbol} (final url {final_url})");
    }
    // A 307 that was not followed, or a Cloudflare interstitial, arrives as HTML
    // and serde's error is opaque. Show what actually came back.
    if body.first() != Some(&b'{') {
        let head: String = String::from_utf8_lossy(&body[..body.len().min(200)])
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        anyhow::bail!("cboe returned non-JSON ({status}, {len} bytes) for {symbol}: {head}", len = body.len());
    }
    let parsed: CboeResponse =
        serde_json::from_slice(&body).context("decoding cboe options payload")?;

    let data = parsed.data;
    let mut by_expiry: std::collections::BTreeMap<i64, (Vec<OptionRow>, Vec<OptionRow>)> =
        std::collections::BTreeMap::new();
    for c in &data.options {
        let Some((expiry, is_call, strike)) = parse_occ(&c.option) else { continue };
        let mut row = c.row();
        row.strike = strike;
        let entry = by_expiry.entry(expiry).or_default();
        if is_call {
            entry.0.push(row);
        } else {
            entry.1.push(row);
        }
    }

    let dates: Vec<i64> = by_expiry.keys().copied().collect();
    let (target, is_current) =
        pick_expiry(&dates, now_et).ok_or_else(|| anyhow::anyhow!("no expiries for {symbol}"))?;
    let (calls, puts) = by_expiry.get(&target).cloned().unwrap_or_default();

    Ok(build(
        &data.symbol,
        data.current_price.unwrap_or(0.0),
        target,
        is_current,
        &calls,
        &puts,
        "cboe",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // OCC encodes the strike in thousandths in an 8-digit field, so "00050000"
    // is 50.00. Cross-checked against Yahoo, which reports the same NVDA contract
    // (NVDA260925C00050000, last traded 173.73, volume 2, OI 5) as strike 50.0.

    #[test]
    fn parses_four_character_root() {
        let (ts, is_call, strike) = parse_occ("NVDA260925C00050000").unwrap();
        assert_eq!(ts, 1_790_343_000, "2026-09-25 09:30 ET");
        assert!(is_call);
        assert_eq!(strike, 50.0);
    }

    #[test]
    fn parses_padded_root_and_put() {
        let (ts, is_call, strike) = parse_occ("CEGG260925P00100000").unwrap();
        assert_eq!(ts, 1_790_343_000);
        assert!(!is_call);
        assert_eq!(strike, 100.0);
    }

    #[test]
    fn parses_low_strike_and_long_dated() {
        let (_, is_call, strike) = parse_occ("MU260925C00022500").unwrap();
        assert!(is_call);
        assert_eq!(strike, 22.5);
        let (ts, _, strike) = parse_occ("MU281215C00100000").unwrap();
        assert_eq!(strike, 100.0);
        assert_eq!(ts, 1_860_503_400, "2028-12-15 09:30 ET");
    }

    #[test]
    fn two_digit_year_century_mapping() {
        // 26 -> 2026, 69 -> 1969. Keeps the OCC convention in one place.
        assert_eq!(et_epoch_from_occ(26, 9, 25), crate::marketclock::et_epoch(2026, 9, 25));
        assert_eq!(et_epoch_from_occ(69, 1, 3), crate::marketclock::et_epoch(1969, 1, 3));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_occ("").is_none());
        assert!(parse_occ("NOTANOPTION").is_none());
        assert!(parse_occ("NVDA26092XC00050000").is_none());
        assert!(parse_occ("SHORT").is_none());
    }
}
