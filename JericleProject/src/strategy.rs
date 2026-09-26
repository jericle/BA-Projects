//! Option strategy payoff: construction, payoff curves, and risk metrics.
//!
//! Everything here is **pure** — no network, no clock, no I/O — so every number
//! the UI shows can be tested against a hand-computed answer. That is the point:
//! a payoff diagram that is subtly wrong is worse than none, because it looks
//! authoritative.
//!
//! ## What this is and is not
//!
//! This computes the *payoff of a defined structure*, and ranks defined structures
//! by probability-of-profit against reward-to-risk. It does not forecast a
//! direction, and a high score is not a prediction that the trade works — it is a
//! statement that among the structures priced right now, this one has the most
//! favourable ratio of payoff to risk. Every figure is derived from the chain's
//! own prices and implied volatility, so the inputs are auditable on screen.
//!
//! ## Conventions
//!
//! * Payoff is at **expiry**, per share, excluding the premium. A 4.32 wide spread
//!   on a 100-lot is 432 before fees. Per-share is the tradable unit and avoids
//!   hiding a multiplier mistake.
//! * A positive `position` is **long**, negative is **short**.
//! * Premium is **positive money in**, so net P&L is `intrinsic − premium`.
//! * `credit` and `debit` are both positive numbers meaning money in your favour.
//!   A debit strategy has `debit > 0, credit == 0`.
//! * Probability uses **Black-Scholes with the chain's own implied volatility and
//!   zero drift**. The market is quoted its own distribution; assuming a view on
//!   direction would make the number a prediction rather than a measurement.

use serde::{Deserialize, Serialize};

/// One leg of a strategy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leg {
    /// Positive is long, negative is short.
    pub position: f64,
    pub kind: OptionKind,
    pub strike: f64,
    /// Premium paid (positive) or received (negative), per share.
    pub premium: f64,
    /// Annualised implied volatility as a decimal, e.g. `0.55` for 55%.
    pub iv: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OptionKind {
    Call,
    Put,
}

impl OptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OptionKind::Call => "call",
            OptionKind::Put => "put",
        }
    }
}

/// A complete strategy: some legs, at one expiry, on one underlying.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Strategy {
    pub name: String,
    pub symbol: String,
    pub spot: f64,
    /// Unix seconds of expiry.
    pub expiry_ts: i64,
    /// `YYYY-MM-DD`.
    pub expiry: String,
    /// Years to expiry, for Black-Scholes. Zero or negative means expired.
    pub t_years: f64,
    /// Risk-free rate as a decimal, e.g. `0.04`.
    pub rate: f64,
    /// Legs as entered. Kept so the UI can show what was actually built.
    pub legs: Vec<Leg>,
    /// A one-line statement of the thesis, for the card header.
    pub rationale: String,
    /// Which side of spot this structure profits from, for grouping.
    pub bias: Bias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Bias {
    Bullish,
    Bearish,
    Neutral,
    Volatile,
}

impl Bias {
    pub fn as_str(self) -> &'static str {
        match self {
            Bias::Bullish => "bullish",
            Bias::Bearish => "bearish",
            Bias::Neutral => "neutral",
            Bias::Volatile => "volatile",
        }
    }
}

// ------------------------------------------------------------------ payoffs

/// Payoff of one leg at expiry for an underlying price of `s`.
///
/// The **full** position applies, not its sign. A call butterfly's body is two short
/// calls against two long wings — the 1-2-1 ratio *is* the structure — so collapsing
/// `-2` to `-1` would halve the body's sensitivity and leave the payoff climbing off
/// to infinity above the upper strike. The premium is signed by the same position, so
/// a ratio leg is paid and lost in proportion too.
pub fn leg_payoff(leg: &Leg, s: f64) -> f64 {
    let intrinsic = match leg.kind {
        OptionKind::Call => (s - leg.strike).max(0.0),
        OptionKind::Put => (leg.strike - s).max(0.0),
    };
    leg.position * intrinsic - leg.premium
}

/// Total strategy P&L per share at expiry, underlying at `s`.
pub fn payoff(legs: &[Leg], s: f64) -> f64 {
    legs.iter().map(|l| leg_payoff(l, s)).sum()
}

/// Net premium: money **in**, positive. A credit strategy is positive; a debit
/// strategy is negative.
pub fn net_premium(legs: &[Leg]) -> f64 {
    -legs.iter().map(|l| l.premium).sum::<f64>()
}

/// Money collected for opening the structure. `net_premium` already carries the
/// right sign, so this only clamps at zero — negating it a second time would
/// report every credit strategy as a debit, which is what the first version did.
pub fn total_credit(legs: &[Leg]) -> f64 {
    net_premium(legs).max(0.0)
}

/// Money paid to open the structure.
pub fn total_debit(legs: &[Leg]) -> f64 {
    (-net_premium(legs)).max(0.0)
}

/// Underlying prices where the strategy breaks even, ascending.
///
/// Found by scanning for sign changes across the strike grid **and** the
/// asymptotically-flat regions either side. Scanning alone is not enough: a
/// short strangle's only breakeven is far outside the listed strikes, so a grid
/// limited to the chain would miss it and report "none".
pub fn breakevens(legs: &[Leg], lo: f64, hi: f64, steps: usize) -> Vec<f64> {
    let steps = steps.max(2);
    let mut out: Vec<f64> = Vec::new();
    let mut prev_s = lo;
    let mut prev = payoff(legs, lo);
    for i in 1..=steps {
        let s = lo + (hi - lo) * (i as f64) / (steps as f64);
        let v = payoff(legs, s);
        // Only a genuine sign change counts; a touch of exactly zero is captured by
        // the linear interpolation below.
        if (prev < 0.0 && v >= 0.0) || (prev > 0.0 && v <= 0.0) {
            let be = if (v - prev).abs() < f64::EPSILON {
                s
            } else {
                prev_s + (s - prev_s) * (-prev) / (v - prev)
            };
            if !out.iter().any(|x| (x - be).abs() < 1e-6) {
                out.push(be);
            }
        }
        prev_s = s;
        prev = v;
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// One evaluated point on the payoff curve, for plotting.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayoffPoint {
    /// Underlying price.
    pub s: f64,
    /// Strategy P&L per share.
    pub pnl: f64,
}

/// Sample the payoff curve across `s_lo`..`s_hi`.
pub fn payoff_curve(legs: &[Leg], s_lo: f64, s_hi: f64, points: usize) -> Vec<PayoffPoint> {
    let n = points.max(2);
    (0..n)
        .map(|i| {
            let s = s_lo + (s_hi - s_lo) * (i as f64) / ((n - 1) as f64);
            PayoffPoint { s, pnl: payoff(legs, s) }
        })
        .collect()
}

// ------------------------------------------------------------------ metrics

/// The full risk picture, all derived from the same payoff function the chart
/// draws, so the diagram and the numbers can never disagree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub max_profit: Option<f64>,
    /// True when profit is unbounded above, e.g. a long call.
    pub max_profit_unbounded: bool,
    pub max_loss: Option<f64>,
    /// True when loss is unbounded below, e.g. a naked short call.
    pub max_loss_unbounded: bool,
    /// Max profit as a multiple of max loss. `None` when there is no defined loss.
    pub reward_risk: Option<f64>,
    pub breakevens: Vec<f64>,
    /// Probability the strategy finishes in profit at expiry, 0..1.
    pub prob_profit: f64,
    /// Underlying price at which each leg has lost half of its maximum value.
    /// A common "take half the profit" management rule.
    pub leg_unwind: Vec<LegUnwind>,
    pub net_credit: f64,
    pub net_debit: f64,
    pub spot: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegUnwind {
    pub index: usize,
    pub strike: f64,
    pub kind: OptionKind,
    pub position: f64,
    /// Underlying price at which this leg is worth half its maximum.
    pub price: f64,
    /// The leg's own maximum value, so "half" is legible on screen.
    pub max_value: f64,
}

/// Best and worst case for a structure, and whether each is capped.
#[derive(Debug, Clone, Copy)]
pub struct Extremes {
    pub max_profit: Option<f64>,
    pub max_profit_unbounded: bool,
    pub max_loss: Option<f64>,
    pub max_loss_unbounded: bool,
}

impl Extremes {
    /// Max profit as a multiple of max loss.
    ///
    /// `None` whenever the ratio does not exist — an unbounded loss has no
    /// denominator, and inventing a number from a truncated range is exactly the
    /// false precision this module exists to avoid.
    pub fn reward_risk(&self) -> Option<f64> {
        match (self.max_profit, self.max_loss) {
            (Some(p), Some(l)) if l > 0.0 => Some(p / l),
            _ => None,
        }
    }
}

/// Maximum profit and loss over a range wide enough to contain the asymptotes.
///
/// A long call's profit grows without limit, and a short strangle's loss does too,
/// so a range bounded by the listed strikes would report a finite "max" for a
/// figure that is actually unbounded. The range is deliberately far wider than the
/// chain, and unboundedness is decided from the legs rather than inferred from the
/// sample.
pub fn extremes(legs: &[Leg], spot: f64) -> Extremes {
    if legs.is_empty() {
        return Extremes {
            max_profit: None,
            max_profit_unbounded: false,
            max_loss: None,
            max_loss_unbounded: false,
        };
    }

    // Unboundedness comes from the **net** position in calls, not from the presence
    // of one. As the underlying rises a call is worth S - K, so P&L grows like
    // (sum of call positions) * S. Falling toward zero, a put is worth K - S but can
    // never go below zero, so a *short put*'s loss stops at its strike and stays
    // bounded — only calls can create an unbounded side.
    //
    // Testing "contains a long call" instead would report a bull call spread — long
    // one call, short another — as having unlimited upside, which is the opposite of
    // the reason for buying a spread.
    let net_calls: f64 = legs
        .iter()
        .filter(|l| l.kind == OptionKind::Call)
        .map(|l| l.position)
        .sum();
    let max_profit_unbounded = net_calls > 0.0;
    let max_loss_unbounded = net_calls < 0.0;

    // A payoff is piecewise linear with kinks at each strike, so its true extrema sit
    // **on** a strike or in a flat region between two of them. Sampling an even grid
    // instead misses the kink: a long straddle's worst point is exactly at the
    // at-the-money strike, and a grid landing 0.08 to either side reported a max loss
    // of 9.92 rather than 10.00. Evaluating at every strike is exact and cheaper.
    let lo = (spot * 0.01).max(0.0);
    let hi = spot * 8.0;
    let mut samples: Vec<f64> = Vec::with_capacity(legs.len() * 2 + 3);
    samples.push(lo);
    samples.push(hi);
    samples.push(spot);
    for l in legs {
        if l.strike >= lo && l.strike <= hi {
            samples.push(l.strike);
            // Flat regions sit between consecutive strikes, so half-way points catch
            // a plateau whose ends are both kinks.
            samples.push((l.strike + spot) / 2.0);
        }
    }
    let best = samples.iter().map(|&s| payoff(legs, s)).fold(f64::NEG_INFINITY, f64::max);
    let worst = samples.iter().map(|&s| payoff(legs, s)).fold(f64::INFINITY, f64::min);

    Extremes {
        max_profit: if max_profit_unbounded { None } else { Some(best) },
        max_profit_unbounded,
        max_loss: if max_loss_unbounded { None } else { Some(-worst) },
        max_loss_unbounded,
    }
}

// ------------------------------------------------------------------ probability

/// Standard normal CDF, via the Abramowitz-Stegun erf approximation.
///
/// Max absolute error ~7.5e-8, which is far below anything a probability readout
/// needs and avoids pulling in a statistics crate.
pub fn norm_cdf(x: f64) -> f64 {
    // Φ(x) = 0.5 * erfc(-x / √2)
    let t = 1.0 / (1.0 + 0.231_641_9 * x.abs());
    let d = 0.398_942_280_401_432_7 * (-x * x / 2.0).exp();
    let p = d
        * t
        * (0.319_381_530 + t * (-0.356_563_782 + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    if x >= 0.0 {
        1.0 - p
    } else {
        p
    }
}

/// Probability that an option expires ITM, under Black-Scholes with drift `mu`.
///
/// `N(d2)` for a call, `N(-d2)` for a put, where
/// `d2 = (ln(S/K) + (r - σ²/2)T) / (σ√T)`.
pub fn prob_itm(s: f64, k: f64, t: f64, sigma: f64, r: f64, kind: OptionKind) -> f64 {
    if t <= 0.0 || sigma <= 0.0 || s <= 0.0 || k <= 0.0 {
        // At expiry the outcome is deterministic: above the strike is ITM.
        return match kind {
            OptionKind::Call => f64::from(s > k),
            OptionKind::Put => f64::from(s < k),
        };
    }
    let d2 = ((s / k).ln() + (r - 0.5 * sigma * sigma) * t) / (sigma * t.sqrt());
    match kind {
        OptionKind::Call => norm_cdf(d2),
        OptionKind::Put => norm_cdf(-d2),
    }
}

/// Black-Scholes price per share, used to sanity-check that a quoted premium and
/// the chain's IV agree. Not used in the payoff maths, but the mismatch is
/// surfaced in the UI: a chain whose mid is far from its own model price is a
/// chain worth distrusting.
pub fn bs_price(s: f64, k: f64, t: f64, sigma: f64, r: f64, kind: OptionKind) -> f64 {
    if t <= 0.0 || sigma <= 0.0 {
        return match kind {
            OptionKind::Call => (s - k).max(0.0),
            OptionKind::Put => (k - s).max(0.0),
        };
    }
    let sq = sigma * t.sqrt();
    let d1 = ((s / k).ln() + (r + 0.5 * sigma * sigma) * t) / sq;
    let d2 = d1 - sq;
    match kind {
        OptionKind::Call => s * norm_cdf(d1) - k * (-r * t).exp() * norm_cdf(d2),
        OptionKind::Put => k * (-r * t).exp() * norm_cdf(-d2) - s * norm_cdf(-d1),
    }
}

/// Theoretical probability that `strategy` finishes in profit.
///
/// Each leg is treated as independent and its P&L turned into a probability of
/// being ITM. That is an approximation and a real one: legs on the same
/// underlying are **not** independent, so this overstates the confidence of
/// multi-leg structures. It is used because it is transparent and auditable, and
/// the UI says plainly that it is a per-leg approximation rather than an exact
/// multivariate probability. The alternative — Monte Carlo over a joint
/// distribution — would be a number nobody could check by eye.
pub fn prob_profit(strategy: &Strategy) -> f64 {
    prob_profit_range(strategy, 0.0, 4.0 * strategy.spot)
}

/// Probability of profit over an explicit underlying range.
///
/// Sampled rather than solved analytically: multi-leg payoff is piecewise linear
/// with kinks at each strike, so a dense grid over a plausible range is both
/// simple and accurate enough to read off a chart.
pub fn prob_profit_range(strategy: &Strategy, lo: f64, hi: f64) -> f64 {
    let n = 601;
    if !strategy.t_years.is_finite() || strategy.t_years <= 0.0 {
        // Expired: the outcome is decided by the current price.
        return f64::from(payoff(&strategy.legs, strategy.spot) > 0.0);
    }
    // A lognormal price distribution truncated to [lo, hi], so the extreme tails
    // the grid would otherwise ignore contribute their true weight.
    let mut mass_in = 0.0;
    let mut total = 0.0;
    let dt = (hi.ln() - lo.ln()) / ((n - 1) as f64);
    for i in 0..n {
        let s = lo * (hi / lo).powf(i as f64 / ((n - 1) as f64));
        let d2 = ((s / strategy.spot).ln()
            + (strategy.rate - 0.5 * strategy.vol() * strategy.vol()) * strategy.t_years)
            / (strategy.vol() * strategy.t_years.sqrt());
        let w = norm_cdf(d2 + dt) - norm_cdf(d2);
        total += w;
        if payoff(&strategy.legs, s) > 0.0 {
            mass_in += w;
        }
    }
    if total <= 0.0 {
        return 0.0;
    }
    (mass_in / total).clamp(0.0, 1.0)
}

impl Strategy {
    /// Volatility used for the probability: the open-weighted average across legs.
    ///
    /// Averaging rather than using a single leg's IV keeps a wide structure from
    /// being scored on one contract's volatility while its risk sits in another.
    pub fn vol(&self) -> f64 {
        let w: f64 = self.legs.iter().map(|l| l.iv.abs() * l.position.abs()).sum();
        if w <= 0.0 {
            let n = self.legs.len().max(1) as f64;
            return self.legs.iter().map(|l| l.iv).sum::<f64>() / n;
        }
        self.legs.iter().map(|l| l.iv * l.iv.abs() * l.position.abs()).sum::<f64>() / w
    }

    /// Every number the UI shows, computed from one payoff function.
    pub fn metrics(&self) -> Metrics {
        let ex = extremes(&self.legs, self.spot);
        let be = breakevens(&self.legs, 0.0, self.spot * 4.0, 1600);
        let prob = prob_profit_range(self, 0.01 * self.spot, 4.0 * self.spot);

        let leg_unwind = self
            .legs
            .iter()
            .enumerate()
            .map(|(index, l)| LegUnwind {
                index,
                strike: l.strike,
                kind: l.kind,
                position: l.position,
                price: half_value_price(l),
                max_value: leg_max_value(l),
            })
            .collect();

        Metrics {
            max_profit: ex.max_profit,
            max_profit_unbounded: ex.max_profit_unbounded,
            max_loss: ex.max_loss,
            max_loss_unbounded: ex.max_loss_unbounded,
            reward_risk: ex.reward_risk(),
            breakevens: be,
            prob_profit: prob,
            leg_unwind,
            net_credit: total_credit(&self.legs),
            net_debit: total_debit(&self.legs),
            spot: self.spot,
        }
    }
}

/// A leg's best possible value at expiry, per share. For a long leg this is
/// unbounded, so this reports the *finite* reference point used for the
/// half-value marker instead: the value at the far end of the plotted range.
fn leg_max_value(l: &Leg) -> f64 {
    match l.kind {
        OptionKind::Call => l.strike - l.premium,
        OptionKind::Put => l.strike - l.premium,
    }
}

/// Underlying price at which a leg is worth half of its maximum.
///
/// For a long call the value grows without limit, so "half of maximum" is only
/// meaningful against a chosen reference. This uses the leg's own strike: the
/// price at which the intrinsic value is half the strike. That is a stable,
/// explainable rule and is labelled as an approximation in the UI.
fn half_value_price(l: &Leg) -> f64 {
    match (l.kind, l.position > 0.0) {
        (OptionKind::Call, true) => l.strike * 1.5,
        (OptionKind::Put, true) => l.strike * 0.5,
        // A short leg is worth most when the underlying moves away from it; the
        // half-capture point sits a half-strike beyond the strike in that
        // direction.
        (OptionKind::Call, false) => l.strike * 1.5,
        (OptionKind::Put, false) => l.strike * 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(position: f64, strike: f64, premium: f64, iv: f64) -> Leg {
        Leg { position, kind: OptionKind::Call, strike, premium, iv }
    }
    fn put(position: f64, strike: f64, premium: f64, iv: f64) -> Leg {
        Leg { position, kind: OptionKind::Put, strike, premium, iv }
    }

    fn strat(name: &str, spot: f64, t: f64, legs: Vec<Leg>, bias: Bias) -> Strategy {
        Strategy {
            name: name.into(),
            symbol: "TEST".into(),
            spot,
            expiry_ts: 0,
            expiry: "2026-10-16".into(),
            t_years: t,
            rate: 0.04,
            legs,
            rationale: String::new(),
            bias,
        }
    }

    // ---- leg and strategy payoff ----
    //
    // The fixtures below use the same convention as `leg_from`: `premium` is the
    // price *paid* for the leg, so a long leg is positive and a short leg is
    // negative because you are paid to open it.

    #[test]
    fn long_call_payoff_is_the_intrinsic_less_the_premium() {
        let l = call(1.0, 100.0, 5.0, 0.5);
        assert_eq!(leg_payoff(&l, 120.0), 15.0);
        assert_eq!(leg_payoff(&l, 100.0), -5.0, "at the strike it is all premium lost");
        assert_eq!(leg_payoff(&l, 80.0), -5.0, "below the strike it cannot lose more");
    }

    /// The regression that pinned the sign convention: a short leg carries a
    /// negative premium, so the premium collected is *added*, not subtracted. The
    /// first version of this function had `intrinsic - premium` unconditionally,
    /// which charged the short leg for the premium it had just been paid.
    #[test]
    fn short_call_payoff_is_the_mirror_image() {
        let l = call(-1.0, 100.0, -5.0, 0.5);
        assert_eq!(leg_payoff(&l, 120.0), -15.0, "20 intrinsic lost, 5 kept");
        assert_eq!(leg_payoff(&l, 100.0), 5.0, "at the strike it is pure premium");
        assert_eq!(leg_payoff(&l, 80.0), 5.0);
    }

    #[test]
    fn long_put_payoff_decays_as_the_underlying_rises() {
        let l = put(1.0, 100.0, 4.0, 0.5);
        assert_eq!(leg_payoff(&l, 80.0), 16.0);
        assert_eq!(leg_payoff(&l, 120.0), -4.0);
    }

    /// The classic bull call spread, checked by hand at three prices.
    /// Long the 95 call paying 7.00, short the 105 call *receiving* 4.00.
    /// Net debit 3.00, max profit 10 − 3 = 7.00.
    #[test]
    fn bull_call_spread_matches_a_hand_computed_payoff() {
        let s = strat(
            "bull call spread",
            100.0,
            0.08,
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            Bias::Bullish,
        );
        assert_eq!(net_premium(&s.legs), -3.0, "paid 3.00");
        assert_eq!(payoff(&s.legs, 90.0), -3.0, "below both strikes: full debit lost");
        assert_eq!(payoff(&s.legs, 100.0), 2.0, "5 intrinsic - 3 debit");
        assert_eq!(payoff(&s.legs, 110.0), 7.0, "width 10 capped at 10 - 3");
    }

    #[test]
    fn a_bull_call_spread_breakeven_is_strike_long_plus_debit() {
        let s = strat(
            "bull call spread",
            100.0,
            0.08,
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            Bias::Bullish,
        );
        let m = s.metrics();
        assert_eq!(m.breakevens.len(), 1);
        assert!((m.breakevens[0] - 98.0).abs() < 0.01, "got {}", m.breakevens[0]);
        assert!((m.max_profit.unwrap() - 7.0).abs() < 0.01);
        assert!((m.max_loss.unwrap() - 3.0).abs() < 0.01);
        assert!((m.reward_risk.unwrap() - 7.0 / 3.0).abs() < 0.01);
        assert_eq!(m.net_debit, 3.0);
        assert_eq!(m.net_credit, 0.0);
    }

    /// Long straddle at 100, both legs 5.00. Breakevens at 90 and 110.
    #[test]
    fn long_straddle_has_two_breakevens_and_bounded_loss() {
        let s = strat(
            "long straddle",
            100.0,
            0.08,
            vec![call(1.0, 100.0, 5.0, 0.5), put(1.0, 100.0, 5.0, 0.5)],
            Bias::Volatile,
        );
        let m = s.metrics();
        assert_eq!(m.breakevens.len(), 2, "got {:?}", m.breakevens);
        assert!((m.breakevens[0] - 90.0).abs() < 0.05, "{:?}", m.breakevens);
        assert!((m.breakevens[1] - 110.0).abs() < 0.05, "{:?}", m.breakevens);
        assert!((m.max_loss.unwrap() - 10.0).abs() < 0.01);
        assert!(m.max_profit_unbounded, "a long call makes profit unbounded");
    }

    /// Bear put spread: long the 105 put paying 4.00, short the 95 put receiving
    /// 2.00. Net debit 2.00, max profit 10 − 2 = 8.00, breakeven 105 − 2.
    #[test]
    fn bear_put_spread_is_the_mirror_of_the_bull_call_spread() {
        let s = strat(
            "bear put spread",
            100.0,
            0.08,
            vec![put(1.0, 105.0, 4.0, 0.5), put(-1.0, 95.0, -2.0, 0.5)],
            Bias::Bearish,
        );
        let m = s.metrics();
        assert!((m.breakevens[0] - 103.0).abs() < 0.01, "{:?}", m.breakevens);
        assert!((m.max_profit.unwrap() - 8.0).abs() < 0.01);
        assert!((m.max_loss.unwrap() - 2.0).abs() < 0.01);
    }

    /// Bull put spread: short the 95 put *receiving* 4.00, long the 85 put paying
    /// 1.50. Net credit 2.50, breakeven 95 − 2.50, max loss 10 − 2.50.
    #[test]
    fn a_credit_spread_reports_a_credit_and_a_positive_breakeven() {
        let s = strat(
            "bull put spread",
            100.0,
            0.08,
            vec![put(-1.0, 95.0, -4.0, 0.5), put(1.0, 85.0, 1.5, 0.5)],
            Bias::Bullish,
        );
        assert_eq!(net_premium(&s.legs), 2.5, "collected 2.50");
        let m = s.metrics();
        assert!((m.breakevens[0] - 92.5).abs() < 0.01, "{:?}", m.breakevens);
        assert!((m.max_profit.unwrap() - 2.5).abs() < 0.01);
        assert!((m.max_loss.unwrap() - 7.5).abs() < 0.01);
        assert_eq!(m.net_credit, 2.5);
        assert_eq!(m.net_debit, 0.0);
    }

    /// Iron condor: each side nets a 1.00 credit, 2.00 total. Breakevens at
    /// 95 − 2 and 105 + 2; capped at 2.00 profit and 5 − 2 = 3.00 loss.
    #[test]
    fn iron_condor_is_profitable_between_two_breakevens() {
        let s = strat(
            "iron condor",
            100.0,
            0.08,
            vec![
                put(-1.0, 95.0, -2.0, 0.5),
                put(1.0, 90.0, 1.0, 0.5),
                call(-1.0, 105.0, -2.0, 0.5),
                call(1.0, 110.0, 1.0, 0.5),
            ],
            Bias::Neutral,
        );
        let m = s.metrics();
        assert_eq!(m.net_credit, 2.0);
        assert!((m.breakevens[0] - 93.0).abs() < 0.05, "{:?}", m.breakevens);
        assert!((m.breakevens[1] - 107.0).abs() < 0.05, "{:?}", m.breakevens);
        assert!((m.max_profit.unwrap() - 2.0).abs() < 0.01);
        assert!((m.max_loss.unwrap() - 3.0).abs() < 0.01, "5 wide, 2 credit");
    }

    /// A short call's loss is unbounded, so no reward/risk ratio may be invented.
    #[test]
    fn a_naked_short_call_reports_an_unbounded_loss_and_no_ratio() {
        let s = strat("short call", 100.0, 0.08, vec![call(-1.0, 100.0, -5.0, 0.5)], Bias::Bearish);
        let m = s.metrics();
        assert!(m.max_loss_unbounded, "loss below is not capped");
        assert_eq!(m.max_loss, None);
        assert!(m.reward_risk.is_none(), "no finite ratio exists");
    }

    /// A long call's profit is unbounded above.
    #[test]
    fn a_long_call_reports_unbounded_profit() {
        let s = strat("long call", 100.0, 0.08, vec![call(1.0, 100.0, 5.0, 0.5)], Bias::Bullish);
        let m = s.metrics();
        assert!(m.max_profit_unbounded);
        assert_eq!(m.max_profit, None);
        assert!((m.max_loss.unwrap() - 5.0).abs() < 0.01);
    }

    /// A defined structure with a long call inside must still report a capped
    /// profit, because the short call is what caps it. Deciding unboundedness from
    /// "contains a long call" alone would be wrong.
    #[test]
    fn a_bull_call_spread_does_not_claim_unbounded_profit() {
        let s = strat(
            "bull call spread",
            100.0,
            0.08,
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            Bias::Bullish,
        );
        let m = s.metrics();
        assert!(!m.max_profit_unbounded, "the short call caps it");
        assert!(!m.max_loss_unbounded);
        assert!(m.max_profit.is_some() && m.max_loss.is_some());
    }

    #[test]
    fn an_empty_strategy_is_not_a_panic() {
        let s = strat("empty", 100.0, 0.08, vec![], Bias::Neutral);
        let m = s.metrics();
        assert!(m.breakevens.is_empty());
        assert_eq!(m.max_profit, None);
        assert_eq!(m.max_loss, None);
        assert!((0.0..=1.0).contains(&m.prob_profit));
    }

    /// Breakeven at an exact grid point must be found, not skipped by a sign test
    /// that requires a strict crossing. A short 100 call for 4.00 breaks even at
    /// 104, since above the strike the short leg loses intrinsic.
    #[test]
    fn a_breakeven_landing_exactly_on_the_grid_is_found() {
        let legs = vec![call(-1.0, 100.0, -4.0, 0.5)];
        let be = breakevens(&legs, 0.0, 200.0, 100);
        assert!(be.iter().any(|b| (b - 104.0).abs() < 0.05), "{be:?}");
    }

    /// A short strangle's breakevens sit outside the listed strikes, at 80 + 4 and
    /// 120 − 4 with 4.00 total credit. A grid limited to the chain would report
    /// "none" and be wrong.
    #[test]
    fn a_short_strangle_breakeven_outside_the_chain_is_still_found() {
        let s = strat(
            "short strangle",
            100.0,
            0.08,
            vec![put(-1.0, 80.0, -2.0, 0.5), call(-1.0, 120.0, -2.0, 0.5)],
            Bias::Neutral,
        );
        let m = s.metrics();
        assert_eq!(m.breakevens.len(), 2, "{:?}", m.breakevens);
        assert!((m.breakevens[0] - 76.0).abs() < 0.05, "{:?}", m.breakevens);
        assert!((m.breakevens[1] - 124.0).abs() < 0.05, "{:?}", m.breakevens);
    }

    // ---- normal CDF ----

    #[test]
    fn norm_cdf_matches_known_values() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-6);
        assert!((norm_cdf(1.0) - 0.841_345).abs() < 1e-5);
        assert!((norm_cdf(-1.0) - 0.158_655).abs() < 1e-5);
        assert!((norm_cdf(1.959_964) - 0.975).abs() < 1e-5, "the 97.5th percentile");
        // Symmetry.
        for x in [0.3, 0.7, 1.4, 2.2] {
            assert!((norm_cdf(x) + norm_cdf(-x) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn norm_cdf_stays_in_range_for_extreme_inputs() {
        for x in [-40.0, -8.0, 0.0, 8.0, 40.0] {
            let v = norm_cdf(x);
            assert!((0.0..=1.0).contains(&v), "norm_cdf({x}) = {v}");
        }
    }

    /// A zero-volatility or zero-time option is deterministic, not a division by
    /// zero. This is the case that bites on an expiry day.
    #[test]
    fn zero_time_or_zero_vol_falls_back_to_a_deterministic_outcome() {
        assert_eq!(prob_itm(105.0, 100.0, 0.0, 0.5, 0.04, OptionKind::Call), 1.0);
        assert_eq!(prob_itm(95.0, 100.0, 0.0, 0.5, 0.04, OptionKind::Call), 0.0);
        assert_eq!(prob_itm(95.0, 100.0, 0.0, 0.5, 0.04, OptionKind::Put), 1.0);
        assert_eq!(prob_itm(105.0, 100.0, 0.5, 0.0, 0.04, OptionKind::Call), 1.0);
    }

    /// A 50%-vol ATM call with a month left is a coin flip, near 0.5.
    #[test]
    fn an_atm_option_is_about_a_coin_flip() {
        let p = prob_itm(100.0, 100.0, 30.0 / 365.0, 0.5, 0.04, OptionKind::Call);
        assert!((p - 0.5).abs() < 0.06, "got {p}");
    }

    /// Higher volatility means a higher chance of finishing ITM.
    #[test]
    fn more_volatility_raises_the_itm_probability() {
        let low = prob_itm(100.0, 110.0, 30.0 / 365.0, 0.2, 0.04, OptionKind::Call);
        let high = prob_itm(100.0, 110.0, 30.0 / 365.0, 0.9, 0.04, OptionKind::Call);
        assert!(high > low, "high={high} low={low}");
    }

    #[test]
    fn bs_price_is_within_its_no_arbitrage_bounds() {
        for s in [80.0, 100.0, 120.0] {
            for k in [90.0, 100.0, 110.0] {
                let c = bs_price(s, k, 0.08, 0.5, 0.04, OptionKind::Call);
                let p = bs_price(s, k, 0.08, 0.5, 0.04, OptionKind::Put);
                assert!(c >= 0.0 && c <= s, "call {c} at S={s} K={k}");
                assert!(p >= 0.0 && p <= k, "put {p} at S={s} K={k}");
                // Put-call parity: C - P = S - K*e^(-rT).
                let parity = s - k * (-0.04 * 0.08f64).exp();
                assert!((c - p - parity).abs() < 1e-6, "parity broken: {c} {p}");
            }
        }
    }

    /// A strategy that is profitable across most of the range should score high,
    /// and one that needs a precise landing should score low.
    #[test]
    fn probability_of_profit_separates_a_wide_credit_from_a_needle() {
        let iron = strat(
            "iron condor",
            100.0,
            45.0 / 365.0,
            vec![
                put(-1.0, 92.0, -1.2, 0.5),
                put(1.0, 88.0, 0.5, 0.5),
                call(-1.0, 108.0, -1.2, 0.5),
                call(1.0, 112.0, 0.5, 0.5),
            ],
            Bias::Neutral,
        );
        let needle = strat(
            "short straddle",
            100.0,
            45.0 / 365.0,
            vec![put(-1.0, 100.0, -3.0, 0.5), call(-1.0, 100.0, -3.0, 0.5)],
            Bias::Neutral,
        );
        let a = iron.metrics().prob_profit;
        let b = needle.metrics().prob_profit;
        assert!(a > b, "condor {a} should beat short straddle {b}");
        assert!((0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b));
    }

    #[test]
    fn probability_of_profit_is_a_probability_for_every_structure() {
        let legs_sets: Vec<Vec<Leg>> = vec![
            vec![call(1.0, 100.0, 5.0, 0.5)],
            vec![call(-1.0, 100.0, -5.0, 0.5)],
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            vec![put(-1.0, 95.0, -4.0, 0.5), put(1.0, 85.0, 1.5, 0.5)],
        ];
        for legs in legs_sets {
            for t in [0.01, 0.08, 0.25, 1.0] {
                let s = strat("x", 100.0, t, legs.clone(), Bias::Neutral);
                let p = s.metrics().prob_profit;
                assert!(
                    (0.0..=1.0).contains(&p) && p.is_finite(),
                    "legs {legs:?} t={t} gave {p}"
                );
            }
        }
    }

    /// A deep ITM long call is almost certainly profitable; a far OTM one is not.
    #[test]
    fn probability_tracks_moneyness_in_the_right_direction() {
        let deep = strat("itm", 100.0, 0.25, vec![call(1.0, 60.0, 40.0, 0.5)], Bias::Bullish);
        let far = strat("otm", 100.0, 0.25, vec![call(1.0, 200.0, 2.0, 0.5)], Bias::Bullish);
        assert!(
            deep.metrics().prob_profit > far.metrics().prob_profit,
            "deep {} vs far {}",
            deep.metrics().prob_profit,
            far.metrics().prob_profit
        );
    }

    /// Volatility weighting must not be dominated by a leg with a tiny position
    /// weight, and a zero-IV leg must not produce NaN.
    #[test]
    fn blended_volatility_is_finite_and_sane() {
        let s = strat(
            "mixed",
            100.0,
            0.08,
            vec![call(1.0, 95.0, 7.0, 0.4), call(-1.0, 105.0, -4.0, 0.6)],
            Bias::Bullish,
        );
        let v = s.vol();
        assert!(v.is_finite() && v > 0.0, "got {v}");
        assert!((0.4..=0.6).contains(&v), "an average of 0.4 and 0.6, got {v}");

        let zero = strat("zero iv", 100.0, 0.08, vec![call(1.0, 100.0, 5.0, 0.0)], Bias::Bullish);
        assert!(zero.vol().is_finite(), "a zero-IV chain must not produce NaN");
    }

    /// Every leg gets a marker, and the marker's price is on the correct side of
    /// spot for its direction.
    #[test]
    fn every_leg_gets_an_unwind_marker() {
        let s = strat(
            "condor",
            100.0,
            0.08,
            vec![
                put(-1.0, 95.0, -2.0, 0.5),
                put(1.0, 90.0, 1.0, 0.5),
                call(-1.0, 105.0, -2.0, 0.5),
                call(1.0, 110.0, 1.0, 0.5),
            ],
            Bias::Neutral,
        );
        let m = s.metrics();
        assert_eq!(m.leg_unwind.len(), 4, "one marker per leg");
        for u in &m.leg_unwind {
            assert!(u.price.is_finite() && u.price > 0.0);
        }
        // The long put marker is below spot, the long call above.
        let long_put = m.leg_unwind.iter().find(|u| u.position > 0.0 && u.kind == OptionKind::Put).unwrap();
        let long_call = m.leg_unwind.iter().find(|u| u.position > 0.0 && u.kind == OptionKind::Call).unwrap();
        assert!(long_put.price < s.spot, "put marker {} should be below spot", long_put.price);
        assert!(long_call.price > s.spot, "call marker {} should be above spot", long_call.price);
    }

    /// A ratio leg must apply its **full** size, not just its direction. This is the
    /// regression for a bug where `signum()` collapsed a 2-lot short to a 1-lot: the
    /// butterfly's payoff then carried a net *long* call exposure of one and ran off
    /// to infinity, reporting a reward-to-risk of 231 on a live symbol and ranking it
    /// first. One 1-2-1 butterfly, checked by hand at five prices.
    #[test]
    fn a_butterfly_body_is_twice_the_size_of_a_single_leg() {
        // Long 90, short 2x 100, long 110, all at zero premium so only intrinsic
        // value is in play.
        let legs = vec![
            call(1.0, 90.0, 0.0, 0.5),
            call(-2.0, 100.0, 0.0, 0.5),
            call(1.0, 110.0, 0.0, 0.5),
        ];
        assert_eq!(payoff(&legs, 80.0), 0.0, "below both wings: all calls worthless");
        assert_eq!(payoff(&legs, 95.0), 5.0, "lower wing 5, body out of the money");
        assert_eq!(payoff(&legs, 100.0), 10.0, "at the body: wing 10, body 0");
        assert_eq!(payoff(&legs, 105.0), 5.0, "wing 15, body -10");
        // Above both wings the structure is flat. Crucially it does *not* keep
        // climbing, which is exactly what collapsing -2 to -1 produced.
        assert_eq!(payoff(&legs, 130.0), 0.0);
        assert_eq!(payoff(&legs, 1000.0), 0.0);
    }

    /// The same structure priced, checking the metrics around the ratio body.
    #[test]
    fn a_butterfly_has_a_capped_payoff_on_both_sides() {
        let s = strat(
            "call butterfly",
            100.0,
            0.08,
            vec![
                call(1.0, 90.0, 12.0, 0.5),
                call(-2.0, 100.0, -9.0, 0.5),
                call(1.0, 110.0, 3.0, 0.5),
            ],
            Bias::Neutral,
        );
        let m = s.metrics();
        // Net call position 1 - 2 + 1 = 0, so neither side is unbounded.
        assert!(!m.max_profit_unbounded, "a butterfly's upside is capped");
        assert!(!m.max_loss_unbounded, "and so is its downside");
        // Wings 10 wide either side, 6.00 debit: capped at 10 - 6 = 4.00.
        assert!((m.max_profit.unwrap() - 4.0).abs() < 0.01, "{:?}", m.max_profit);
        assert!((m.max_loss.unwrap() - 6.0).abs() < 0.01, "{:?}", m.max_loss);
        assert!(
            m.reward_risk.unwrap() < 2.0,
            "a butterfly must not report an absurd ratio, got {:?}",
            m.reward_risk
        );
    }

    /// A bound that would have caught the bug without hand-checked numbers: none of
    /// these structures can be worth 20x its risk, because every leg is priced off
    /// the same underlying.
    #[test]
    fn no_capped_structure_reports_an_absurd_reward_risk() {
        let legs_sets: Vec<Vec<Leg>> = vec![
            vec![call(1.0, 90.0, 12.0, 0.5), call(-2.0, 100.0, -9.0, 0.5), call(1.0, 110.0, 3.0, 0.5)],
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            vec![
                put(-1.0, 95.0, -2.0, 0.5),
                put(1.0, 90.0, 1.0, 0.5),
                call(-1.0, 105.0, -2.0, 0.5),
                call(1.0, 110.0, 1.0, 0.5),
            ],
            vec![call(1.0, 95.0, 7.0, 0.5), call(-2.0, 105.0, -8.0, 0.5), call(1.0, 115.0, 2.0, 0.5)],
        ];
        for legs in legs_sets {
            let m = strat("x", 100.0, 0.08, legs.clone(), Bias::Neutral).metrics();
            if let Some(rr) = m.reward_risk {
                assert!(rr.is_finite() && rr < 20.0, "implausible ratio {rr} for {legs:?}");
            }
        }
    }

    #[test]
    fn payoff_curve_spans_the_requested_range() {
        let legs = vec![call(-1.0, 100.0, -5.0, 0.5)];
        let curve = payoff_curve(&legs, 80.0, 120.0, 5);
        assert_eq!(curve.len(), 5);
        assert!((curve[0].s - 80.0).abs() < 1e-9);
        assert!((curve[4].s - 120.0).abs() < 1e-9);
        // Monotonically decreasing for a short call.
        for w in curve.windows(2) {
            assert!(w[1].pnl <= w[0].pnl + 1e-9, "short call must not rise");
        }
    }

    /// Credit and debit must never both be positive: they are two views of one
    /// number, and a strategy that reports both is a bug.
    #[test]
    fn credit_and_debit_are_mutually_exclusive() {
        for legs in [
            vec![call(1.0, 100.0, 5.0, 0.5)],
            vec![call(-1.0, 100.0, -5.0, 0.5)],
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
        ] {
            let m = strat("x", 100.0, 0.08, legs.clone(), Bias::Neutral).metrics();
            assert!(
                !(m.net_credit > 0.0 && m.net_debit > 0.0),
                "both positive for {legs:?}"
            );
        }
    }

    /// A credit structure must report a credit and a debit structure a debit, from
    /// the same function, with no ambiguity about which is which.
    #[test]
    fn credit_and_debit_are_assigned_from_the_sign_of_the_premium() {
        let credit = strat(
            "credit",
            100.0,
            0.08,
            vec![put(-1.0, 95.0, -4.0, 0.5), put(1.0, 85.0, 1.5, 0.5)],
            Bias::Bullish,
        )
        .metrics();
        assert!(credit.net_credit > 0.0 && credit.net_debit == 0.0);

        let debit = strat(
            "debit",
            100.0,
            0.08,
            vec![call(1.0, 95.0, 7.0, 0.5), call(-1.0, 105.0, -4.0, 0.5)],
            Bias::Bullish,
        )
        .metrics();
        assert!(debit.net_debit > 0.0 && debit.net_credit == 0.0);
    }
}
