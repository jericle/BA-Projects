//! Option wall derivation: pick the current-month expiry, then find the strikes
//! carrying the most open interest.
//!
//! The wall definition matches the existing `Finance/price_hist.py`: the strike
//! with the highest open interest, calls and puts separately. All the logic here
//! is pure so it can be tested without a network call, and both upstream feeds
//! (Yahoo options, CBOE OPRA) reduce to the same `OptionSummary`.

use serde::{Deserialize, Serialize};

use crate::marketclock::Et;
use chrono::Datelike;

/// One row of an option chain, normalised across feeds.
#[derive(Debug, Clone, Default)]
pub struct OptionRow {
    pub strike: f64,
    pub open_interest: i64,
    pub volume: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionSummary {
    pub symbol: String,
    pub spot: f64,
    /// Unix seconds of the expiry.
    pub expiry_ts: i64,
    /// `YYYY-MM-DD` in Eastern Time.
    pub expiry: String,
    /// False when no listed expiry fell in the current ET month and we fell back
    /// to the nearest one. Surfaced in the UI so the date is never ambiguous.
    pub expiry_is_current_month: bool,
    pub call_wall: Option<f64>,
    pub call_wall_oi: i64,
    pub put_wall: Option<f64>,
    pub put_wall_oi: i64,
    pub total_call_oi: i64,
    pub total_put_oi: i64,
    pub total_call_volume: i64,
    pub total_put_volume: i64,
    pub call_strikes: usize,
    pub put_strikes: usize,
    /// Which feed produced this, for transparency.
    pub source: String,
    pub fetched_ts: i64,
}

impl OptionSummary {
    /// Total open interest puts / calls. Above 1 means more put support.
    pub fn put_call_ratio(&self) -> Option<f64> {
        if self.total_call_oi > 0 {
            Some(self.total_put_oi as f64 / self.total_call_oi as f64)
        } else {
            None
        }
    }

    /// Distance from spot to a wall, in percent. Positive means above spot.
    pub fn distance_pct(&self, wall: Option<f64>) -> Option<f64> {
        match (wall, self.spot) {
            (Some(w), s) if s > 0.0 => Some((w / s - 1.0) * 100.0),
            _ => None,
        }
    }
}

/// Yahoo encodes `expirationDates` as midnight **UTC**, and the chain's
/// `expirationDate` as 09:30 ET. Converting either to Eastern Time can shift the
/// calendar day backwards (midnight UTC on the 28th is 20:00 ET on the 27th), which
/// made a live monthly expiry look like it had already passed. Both comparisons and
/// the displayed date therefore use UTC, where both encodings land on the right day.
fn ts_date(ts: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp(ts, 0).map(|utc| utc.date_naive())
}

/// Today in the same frame used for expiry dates.
fn today(now_et: &Et) -> chrono::NaiveDate {
    now_et.with_timezone(&chrono::Utc).date_naive()
}

/// Choose the expiry to display: the earliest listed expiry that is still
/// tradeable *and* falls in the current Eastern Time month. Two subtleties:
/// a monthly expiry earlier in the current month may already have passed (on the
/// 28th, the 25th's contract is dead), and some months have no listed expiry
/// left at all. In that case fall back to the nearest upcoming expiry and report
/// it via the flag, so the UI can say so rather than quietly showing the wrong month.
pub fn pick_expiry(expirations: &[i64], now_et: &Et) -> Option<(i64, bool)> {
    if expirations.is_empty() {
        return None;
    }
    let mut sorted = expirations.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let today_utc = today(now_et);
    let (year, month) = (now_et.year(), now_et.month());

    // Never show a contract that has already expired.
    let upcoming: Vec<i64> = sorted
        .into_iter()
        .filter(|ts| ts_date(*ts).map(|d| d >= today_utc).unwrap_or(false))
        .collect();
    if upcoming.is_empty() {
        return None;
    }

    if let Some(ts) = upcoming.iter().find(|ts| {
        let Some(d) = ts_date(**ts) else { return false };
        (d.year(), d.month()) == (year, month)
    }) {
        return Some((*ts, true));
    }
    Some((upcoming[0], false))
}

/// Strike with the greatest open interest, ignoring rows with no OI.
pub fn wall(rows: &[OptionRow]) -> Option<(f64, i64)> {
    rows.iter()
        .filter(|r| r.open_interest > 0 && r.strike > 0.0)
        .max_by_key(|r| r.open_interest)
        .map(|r| (r.strike, r.open_interest))
}

pub fn total_oi(rows: &[OptionRow]) -> i64 {
    rows.iter().map(|r| r.open_interest).sum()
}

pub fn total_volume(rows: &[OptionRow]) -> i64 {
    rows.iter().map(|r| r.volume).sum()
}

pub fn build(
    symbol: &str,
    spot: f64,
    expiry_ts: i64,
    expiry_is_current_month: bool,
    calls: &[OptionRow],
    puts: &[OptionRow],
    source: &str,
) -> OptionSummary {
    let (call_wall, call_wall_oi) = wall(calls).map(|(s, oi)| (Some(s), oi)).unwrap_or((None, 0));
    let (put_wall, put_wall_oi) = wall(puts).map(|(s, oi)| (Some(s), oi)).unwrap_or((None, 0));
    let expiry = ts_date(expiry_ts)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default();

    OptionSummary {
        symbol: symbol.to_ascii_uppercase(),
        spot,
        expiry_ts,
        expiry,
        expiry_is_current_month,
        call_wall,
        call_wall_oi,
        put_wall,
        put_wall_oi,
        total_call_oi: total_oi(calls),
        total_put_oi: total_oi(puts),
        total_call_volume: total_volume(calls),
        total_put_volume: total_volume(puts),
        call_strikes: calls.len(),
        put_strikes: puts.len(),
        source: source.to_string(),
        fetched_ts: chrono::Utc::now().timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};

    fn et(y: i32, m: u32, d: u32, h: u32, min: u32) -> Et {
        let naive = NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap();
        crate::marketclock::ET.from_local_datetime(&naive).single().unwrap()
    }

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        et(y, m, d, 9, 30).timestamp()
    }

    #[test]
    fn picks_expiry_in_the_current_month() {
        // Friday 25 Sep 2026, 08:25 ET. Expiries: 25th (this month), 28th (Oct), ...
        let now = et(2026, 9, 25, 8, 25);
        let list = vec![ts(2026, 9, 25), ts(2026, 9, 28), ts(2026, 10, 2), ts(2026, 10, 16)];
        let (chosen, is_current) = pick_expiry(&list, &now).unwrap();
        assert_eq!(chosen, ts(2026, 9, 25));
        assert!(is_current);
    }

    #[test]
    fn skips_a_past_expiry_in_the_same_month() {
        // Monday the 28th: the 25th monthly expiry has already passed.
        let now = et(2026, 9, 28, 8, 0);
        let list = vec![ts(2026, 9, 25), ts(2026, 10, 2), ts(2026, 10, 16)];
        let (chosen, is_current) = pick_expiry(&list, &now).unwrap();
        assert_eq!(chosen, ts(2026, 10, 2), "must not show an expired contract");
        assert!(!is_current, "October is not the current month, and we say so");
    }

    #[test]
    fn falls_back_to_nearest_when_month_has_none() {
        // 1 Oct, and the only listed expiries are in November and December.
        let now = et(2026, 10, 1, 8, 0);
        let list = vec![ts(2026, 11, 20), ts(2026, 12, 18)];
        let (chosen, is_current) = pick_expiry(&list, &now).unwrap();
        assert_eq!(chosen, ts(2026, 11, 20));
        assert!(!is_current);
    }

    #[test]
    fn midnight_utc_expiry_does_not_look_already_expired() {
        // Yahoo lists expirations as midnight UTC: "2026-09-25" arrives as
        // 1790294400, which is 20:00 ET on the 24th. Read in ET that looks like
        // yesterday and the live monthly expiry gets discarded.
        let midnight_utc = |y: i32, m: u32, d: u32| {
            chrono::Utc
                .with_ymd_and_hms(y, m, d, 0, 0, 0)
                .unwrap()
                .timestamp()
        };
        assert_eq!(midnight_utc(2026, 9, 25), 1_790_294_400);
        assert_eq!(midnight_utc(2026, 9, 28), 1_790_553_600);

        // 08:25 ET on the 25th, with the 25th and 28th both listed.
        let now = et(2026, 9, 25, 8, 25);
        let (chosen, is_current) =
            pick_expiry(&[midnight_utc(2026, 9, 25), midnight_utc(2026, 9, 28)], &now).unwrap();
        assert_eq!(chosen, midnight_utc(2026, 9, 25), "today's expiry must be selectable");
        assert!(is_current);
        assert_eq!(ts_date(chosen).unwrap().to_string(), "2026-09-25");
    }

    #[test]
    fn handles_empty_and_single_expiry_lists() {
        let now = et(2026, 9, 25, 8, 0);
        assert!(pick_expiry(&[], &now).is_none());
        let (chosen, is_current) = pick_expiry(&[ts(2026, 9, 25)], &now).unwrap();
        assert_eq!(chosen, ts(2026, 9, 25));
        assert!(is_current);
    }

    #[test]
    fn wall_is_the_max_open_interest_strike() {
        let rows = vec![
            OptionRow { strike: 100.0, open_interest: 500, volume: 10 },
            OptionRow { strike: 110.0, open_interest: 9_000, volume: 20 },
            OptionRow { strike: 120.0, open_interest: 1_200, volume: 5 },
        ];
        assert_eq!(wall(&rows), Some((110.0, 9_000)));
    }

    #[test]
    fn wall_ignores_zero_oi_rows() {
        let rows = vec![
            OptionRow { strike: 50.0, open_interest: 0, volume: 0 },
            OptionRow { strike: 60.0, open_interest: 0, volume: 0 },
        ];
        assert_eq!(wall(&rows), None, "no open interest means no wall");
    }

    #[test]
    fn build_produces_a_complete_summary() {
        let calls = vec![
            OptionRow { strike: 200.0, open_interest: 100, volume: 5 },
            OptionRow { strike: 220.0, open_interest: 4_000, volume: 50 },
        ];
        let puts = vec![
            OptionRow { strike: 180.0, open_interest: 7_500, volume: 90 },
            OptionRow { strike: 160.0, open_interest: 200, volume: 10 },
        ];
        let s = build("NVDA", 210.0, ts(2026, 9, 25), true, &calls, &puts, "test");
        assert_eq!(s.expiry, "2026-09-25");
        assert_eq!(s.call_wall, Some(220.0));
        assert_eq!(s.call_wall_oi, 4_000);
        assert_eq!(s.put_wall, Some(180.0));
        assert_eq!(s.put_wall_oi, 7_500);
        assert_eq!(s.total_call_oi, 4_100);
        assert_eq!(s.total_put_oi, 7_700);
        assert_eq!(s.total_put_volume, 100);
        assert_eq!(s.put_call_ratio().unwrap().round(), 2.0);
        assert_eq!(s.distance_pct(s.call_wall).unwrap().round(), 5.0);
        assert_eq!(s.distance_pct(s.put_wall).unwrap().round(), -14.0);
    }
}
