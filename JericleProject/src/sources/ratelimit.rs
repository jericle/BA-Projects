//! A credit counter shaped like the upstream's own quota window.
//!
//! Twelve Data's free tier allows 8 credits per minute and bills one credit per
//! symbol, and it reports a quota breach as **HTTP 200 with an error body** — so
//! detecting the overrun after the fact is not enough. The counter has to refuse
//! before the request goes out.
//!
//! **The window is a fixed wall-clock minute, not a rolling bucket.** This was a
//! real bug caught against the live API: a continuously-refilling token bucket
//! looked correct on its own terms but let 10 requests through into a limit of 8,
//! because a slow sequence of requests trickles extra tokens in mid-window. The
//! upstream's own error said "11 API credits were used, with the current limit
//! being 8". A rolling bucket is the right shape for smoothing a client's own
//! bursts; matching a *provider's* quota means reproducing the provider's window
//! arithmetic exactly, or the limiter causes the very throttling it exists to
//! prevent. So: count within the current clock minute, reset on the minute.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Credits allowed per clock minute, and how many have been spent in this one.
#[derive(Debug)]
pub struct RateLimit {
    pub capacity: u32,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    /// Which wall-clock minute `spent` belongs to, as seconds since the epoch
    /// divided by 60. A counter tagged with the wrong minute is discarded.
    minute: u64,
    spent: u32,
}

impl RateLimit {
    pub fn per_minute(per_minute: u32) -> Self {
        let capacity = per_minute.max(1);
        Self {
            capacity,
            state: Mutex::new(State {
                minute: current_minute(),
                spent: 0,
            }),
        }
    }

    /// Take one credit if the current minute has room.
    ///
    /// Returns how long until the next minute begins, so the caller can tell the
    /// user when to retry instead of just refusing.
    pub fn acquire(&self) -> Result<(), Duration> {
        self.acquire_n(1)
    }

    /// Take `n` credits, all or nothing. A partial spend would leave the caller
    /// with a half-result and no way to tell what was dropped.
    pub fn acquire_n(&self, n: u32) -> Result<(), Duration> {
        if n > self.capacity {
            // Asking for more than a whole minute can hold would otherwise wait
            // out several resets for credits that still would not fit.
            return Err(seconds_to_next_minute());
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_minute();
        if now != st.minute {
            st.minute = now;
            st.spent = 0;
        }
        if st.spent + n <= self.capacity {
            st.spent += n;
            return Ok(());
        }
        Err(seconds_to_next_minute())
    }

    /// Credits left in the current minute. Reported by `/api/series-meta` so the
    /// UI can warn before the last one is spent, not only after a refusal.
    pub fn available(&self) -> u32 {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = current_minute();
        if now != st.minute {
            st.minute = now;
            st.spent = 0;
        }
        self.capacity.saturating_sub(st.spent)
    }
}

/// Seconds since the Unix epoch, divided by 60: the identity of the current
/// clock minute. A clock that jumps backwards resets the counter, which errs
/// towards spending slightly more in one window — the same direction as the
/// upstream's own clock, so the two stay in step.
fn current_minute() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

fn seconds_to_next_minute() -> Duration {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| 60 - (d.as_secs() % 60))
        .unwrap_or(60);
    Duration::from_secs(secs.clamp(1, 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_counter_starts_empty_of_spends() {
        let rl = RateLimit::per_minute(8);
        assert_eq!(rl.available(), 8);
        for _ in 0..8 {
            assert!(rl.acquire().is_ok());
        }
        assert!(rl.acquire().is_err(), "the 9th credit in a minute should be refused");
    }

    /// The exact failure the live API exposed. A rolling bucket would have let a
    /// 9th, 10th and 11th request through as tokens trickled back; a per-minute
    /// counter must refuse all of them until the minute rolls over.
    #[test]
    fn no_more_than_the_allowance_can_be_spent_in_one_minute() {
        let rl = RateLimit::per_minute(8);
        let mut granted = 0;
        // Stand in for a slow sequence of requests spanning most of a minute.
        for _ in 0..40 {
            if rl.acquire().is_ok() {
                granted += 1;
            }
        }
        assert_eq!(granted, 8, "a minute may not yield more than the allowance");
    }

    /// The behaviour the whole module exists for: the refusal is returned before
    /// the request is sent, and carries when to retry.
    #[test]
    fn a_refusal_says_when_to_retry() {
        let rl = RateLimit::per_minute(8);
        for _ in 0..8 {
            rl.acquire().unwrap();
        }
        let wait = rl.acquire().unwrap_err();
        assert!(wait >= Duration::from_secs(1) && wait <= Duration::from_secs(60),
            "retry hint should be within this minute, got {wait:?}");
    }

    #[test]
    fn a_batch_is_all_or_nothing() {
        let rl = RateLimit::per_minute(8);
        for _ in 0..6 {
            rl.acquire().unwrap();
        }
        // 3 do not fit in the 2 remaining; spending 2 and failing would leave the
        // cache cold and the caller with a half-result.
        assert!(rl.acquire_n(3).is_err());
        assert_eq!(rl.available(), 2, "a refused batch must not spend credits");
        assert!(rl.acquire_n(2).is_ok());
    }

    #[test]
    fn a_batch_larger_than_the_allowance_fails_fast() {
        let rl = RateLimit::per_minute(8);
        assert!(rl.acquire_n(9).is_err());
        assert_eq!(rl.available(), 8, "and must not have spent anything");
    }

    /// Crossing a minute boundary must reset the count, and it must do so by
    /// inspecting the clock rather than by elapsed time, so a test can drive it.
    #[test]
    fn crossing_into_the_next_minute_resets_the_count() {
        let rl = RateLimit::per_minute(8);
        for _ in 0..8 {
            rl.acquire().unwrap();
        }
        assert!(rl.acquire().is_err());
        {
            let mut st = rl.state.lock().unwrap();
            st.minute = current_minute().wrapping_sub(1);
        }
        assert!(rl.acquire().is_ok(), "a new minute restores the allowance");
        assert_eq!(rl.available(), 7);
    }

    #[test]
    fn available_does_not_count_a_stale_minute() {
        let rl = RateLimit::per_minute(8);
        {
            let mut st = rl.state.lock().unwrap();
            st.minute = current_minute().wrapping_sub(3);
            st.spent = 8;
        }
        assert_eq!(rl.available(), 8, "last minute's spend is not this minute's");
    }

    /// Misconfiguration should degrade to "very slow", not to a dead tab.
    #[test]
    fn a_zero_limit_still_allows_progress() {
        let rl = RateLimit::per_minute(0);
        assert!(rl.acquire().is_ok());
        assert!(rl.acquire().is_err());
    }
}
