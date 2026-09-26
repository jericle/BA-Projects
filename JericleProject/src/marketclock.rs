//! US market calendar: Eastern Time, DST, NYSE holidays, session phase.
//!
//! Everything in the dashboard is reasoned about in `America/New_York`. The host
//! may be in any timezone (this one is AEST), so nothing may be hardcoded in local time.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};
pub use chrono_tz::Tz;
use std::collections::BTreeSet;

pub type Et = DateTime<Tz>;

pub const ET: Tz = chrono_tz::America::New_York;

/// Pre-market open.
pub const PREMARKET_OPEN: (u32, u32) = (4, 0);
/// Cash session open.
pub const CASH_OPEN: (u32, u32) = (9, 30);
/// Cash session close.
pub const CASH_CLOSE: (u32, u32) = (16, 0);
/// Extended-hours end.
pub const AFTERHOURS_END: (u32, u32) = (20, 0);
/// Time of the pre-open alert.
pub const ALERT_AT: (u32, u32) = (8, 25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before 04:00 ET on a trading day.
    Early,
    /// 04:00 to 09:30 ET on a trading day.
    PreMarket,
    /// 09:30 to 16:00 ET on a trading day.
    Open,
    /// 16:00 to 20:00 ET on a trading day.
    AfterHours,
    /// Weekend or NYSE holiday.
    Closed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Early => "early",
            Phase::PreMarket => "pre-market",
            Phase::Open => "open",
            Phase::AfterHours => "after-hours",
            Phase::Closed => "closed",
        }
    }

    /// True while data is worth refreshing every cycle.
    pub fn is_live(self) -> bool {
        matches!(self, Phase::PreMarket | Phase::Open | Phase::AfterHours)
    }
}

pub fn now_et() -> Et {
    Utc::now().with_timezone(&ET)
}

pub fn et_date(t: &Et) -> NaiveDate {
    t.date_naive()
}

pub fn et_time(t: &Et) -> NaiveTime {
    t.time()
}

/// Unix seconds for 09:30 ET on a calendar date. Used to turn OCC contract
/// symbols (which carry only YYMMDD) into comparable expiry timestamps.
pub fn et_epoch(year: i32, month: u32, day: u32) -> Option<i64> {
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    at_et(date, CASH_OPEN).map(|t| t.timestamp())
}

pub fn at_et(date: NaiveDate, hhmm: (u32, u32)) -> Option<Et> {
    let naive = date.and_hms_opt(hhmm.0, hhmm.1, 0)?;
    ET.from_local_datetime(&naive).single()
}

/// nth weekday of a month, e.g. the 3rd Monday of January. `n` is 1-based.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month");
    let offset = (weekday.num_days_from_monday() as i32
        + 7
        - first.weekday().num_days_from_monday() as i32)
        % 7;
    first + Duration::days(offset as i64) + Duration::days((n as i64) - 1) * 7
}

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (y, m) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).expect("valid month") - Duration::days(1)
}

fn last_weekday(year: i32, month: u32, weekday: Weekday) -> NaiveDate {
    let last = last_day_of_month(year, month);
    let back = (last.weekday().num_days_from_monday() as i32
        + 7
        - weekday.num_days_from_monday() as i32)
        % 7;
    last - Duration::days(back as i64)
}

/// Good Friday is not derivable without an Easter algorithm, so it is tabulated.
const GOOD_FRIDAY: &[NaiveDate] = &[
    NaiveDate::from_ymd_opt(2026, 4, 3).unwrap(),
    NaiveDate::from_ymd_opt(2027, 3, 26).unwrap(),
    NaiveDate::from_ymd_opt(2028, 4, 14).unwrap(),
    NaiveDate::from_ymd_opt(2029, 3, 30).unwrap(),
];

/// NYSE holidays for a year, with weekend observance shifting applied.
pub fn holidays(year: i32) -> BTreeSet<NaiveDate> {
    let mut set = BTreeSet::new();

    // Fixed-date holidays, observed on the nearest weekday.
    for (month, day) in [(1, 1), (6, 19), (7, 4), (12, 25)] {
        let Some(actual) = NaiveDate::from_ymd_opt(year, month, day) else {
            continue;
        };
        set.insert(observe(actual));
    }

    // Floating holidays.
    set.insert(nth_weekday(year, 1, Weekday::Mon, 3)); // MLK Day
    set.insert(nth_weekday(year, 2, Weekday::Mon, 3)); // Presidents' Day
    set.insert(last_weekday(year, 5, Weekday::Mon)); // Memorial Day
    set.insert(nth_weekday(year, 9, Weekday::Mon, 1)); // Labor Day
    set.insert(nth_weekday(year, 11, Weekday::Thu, 4)); // Thanksgiving
    for gf in GOOD_FRIDAY {
        if gf.year() == year {
            set.insert(*gf);
        }
    }
    set
}

/// Saturday holidays are observed Friday, Sunday holidays Monday.
fn observe(date: NaiveDate) -> NaiveDate {
    match date.weekday() {
        Weekday::Sat => date - Duration::days(1),
        Weekday::Sun => date + Duration::days(1),
        _ => date,
    }
}

pub fn is_trading_day(date: NaiveDate) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && !holidays(date.year()).contains(&date)
}

/// The trading session this instant belongs to: today if today is a trading day,
/// otherwise the most recent prior trading day.
pub fn session_date(t: &Et) -> NaiveDate {
    let today = et_date(t);
    if is_trading_day(today) {
        return today;
    }
    let mut d = today;
    for _ in 0..10 {
        d -= Duration::days(1);
        if is_trading_day(d) {
            return d;
        }
    }
    today
}

pub fn phase(t: &Et) -> Phase {
    let today = et_date(t);
    if !is_trading_day(today) {
        return Phase::Closed;
    }
    let time = et_time(t);
    let before = |h: u32, m: u32| time < NaiveTime::from_hms_opt(h, m, 0).unwrap();
    if before(PREMARKET_OPEN.0, PREMARKET_OPEN.1) {
        Phase::Early
    } else if before(CASH_OPEN.0, CASH_OPEN.1) {
        Phase::PreMarket
    } else if before(CASH_CLOSE.0, CASH_CLOSE.1) {
        Phase::Open
    } else if before(AFTERHOURS_END.0, AFTERHOURS_END.1) {
        Phase::AfterHours
    } else {
        Phase::Closed
    }
}

/// Next 09:30 ET cash open at or after `from`.
pub fn next_cash_open(from: &Et) -> Et {
    let mut date = session_date(from);
    for _ in 0..14 {
        if let Some(open) = at_et(date, CASH_OPEN) {
            if open > *from && is_trading_day(date) {
                return open;
            }
        }
        date += Duration::days(1);
    }
    *from + Duration::days(1)
}

/// Next 08:25 ET alert instant at or after `from` (the pre-open briefing).
pub fn next_alert_instant(from: &Et) -> Et {
    let mut date = et_date(from);
    for _ in 0..14 {
        if is_trading_day(date) {
            if let Some(at) = at_et(date, ALERT_AT) {
                if at > *from {
                    return at;
                }
            }
        }
        date += Duration::days(1);
    }
    *from + Duration::days(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn floating_holidays_land_correctly() {
        let h = holidays(2026);
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 1, 19).unwrap()), "MLK");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 2, 16).unwrap()), "Presidents");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 4, 3).unwrap()), "Good Friday");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()), "Memorial");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 7, 3).unwrap()), "July 4 Sat -> Fri 3");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 9, 7).unwrap()), "Labor");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 11, 26).unwrap()), "Thanksgiving");
        assert!(h.contains(&NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()), "Christmas");
    }

    #[test]
    fn phase_respects_dst_boundaries() {
        // 2026-09-25 is a Friday, 08:25 ET.
        let t = at_et(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), (8, 25)).unwrap();
        assert_eq!(phase(&t), Phase::PreMarket);
        // Same day after the close.
        let t = at_et(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), (17, 0)).unwrap();
        assert_eq!(phase(&t), Phase::AfterHours);
        // Thanksgiving is closed.
        let t = at_et(NaiveDate::from_ymd_opt(2026, 11, 26).unwrap(), (10, 0)).unwrap();
        assert_eq!(phase(&t), Phase::Closed);
    }

    #[test]
    fn alert_lands_on_trading_days() {
        // Friday 09:00 ET -> same day 08:25 has passed, so next is Monday.
        let t = at_et(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), (9, 0)).unwrap();
        let next = next_alert_instant(&t);
        assert_eq!(next.date_naive(), NaiveDate::from_ymd_opt(2026, 9, 28).unwrap());
        // 07:00 ET -> still today.
        let t = at_et(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), (7, 0)).unwrap();
        let next = next_alert_instant(&t);
        assert_eq!(next.date_naive(), NaiveDate::from_ymd_opt(2026, 9, 25).unwrap());
        assert_eq!(next.time().hour(), 8);
        assert_eq!(next.time().minute(), 25);
    }

    #[test]
    fn et_offsets_are_correct() {
        let t = at_et(NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(), (9, 30)).unwrap();
        // EDT (UTC-4) -> 13:30 UTC
        assert_eq!(t.with_timezone(&Utc).hour(), 13);
        // EST (UTC-5) in January -> 14:30 UTC
        let t = at_et(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), (9, 30)).unwrap();
        assert_eq!(t.with_timezone(&Utc).hour(), 14);
    }
}
