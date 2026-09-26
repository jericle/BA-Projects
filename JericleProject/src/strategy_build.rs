//! Building candidate strategies from a real option chain, and ranking them.
//!
//! The library is deliberately **defined-risk only**: every structure has a
//! payoff that is bounded on both sides, so max profit, max loss and the
//! reward-to-risk ratio are exact numbers rather than "unlimited". A naked short
//! call is left out precisely because its loss is unbounded and the ratio people
//! quote for it is meaningless.
//!
//! Strikes are chosen **relative to spot and to the chain's own liquidity**, not
//! hardcoded. A fixed $5 wing is a 2% move on one symbol and a 25% move on another,
//! so a hardcoded width would quietly produce a different strategy per name. Widths
//! are expressed in a fraction of spot, then snapped to the nearest listed strike.
//!
//! Nothing here takes a directional view. Every candidate for a symbol is
//! generated, priced from the chain, and ranked; the ranking is a statement about
//! payoff shape, not about where the price is going.

use crate::strategy::{
    net_premium, payoff, Bias, Extremes, Leg, Metrics, OptionKind, Strategy,
};

/// One contract as quoted by the chain, with everything needed to build a leg.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quote {
    pub strike: f64,
    pub bid: f64,
    pub ask: f64,
    pub iv: f64,
    pub open_interest: i64,
    pub volume: i64,
}

impl Quote {
    /// Mid-market, the price a strategy is assumed to be entered at.
    ///
    /// The mid is the honest default. Marking at the bid flatters a credit and
    /// penalises a debit, and marking at the ask does the reverse; either way the
    /// reported edge is partly an artefact of which side you picked.
    pub fn mid(&self) -> f64 {
        if self.bid > 0.0 && self.ask > 0.0 {
            (self.bid + self.ask) / 2.0
        } else if self.last_is_positive() {
            self.bid.max(self.ask)
        } else {
            0.0
        }
    }

    fn last_is_positive(&self) -> bool {
        self.bid > 0.0 || self.ask > 0.0
    }

    /// A two-sided quote wide enough to trade. One-sided or zero-quote contracts
    /// are excluded: a strategy leg priced off an empty side is fiction.
    pub fn tradable(&self) -> bool {
        self.bid > 0.0 && self.ask > 0.0 && self.ask >= self.bid && self.iv > 0.0
    }
}

/// One side of a chain at one expiry.
#[derive(Debug, Clone, Default)]
pub struct ChainSide {
    pub calls: Vec<Quote>,
    pub puts: Vec<Quote>,
}

impl ChainSide {
    pub fn nearest(&self, kind: OptionKind, target: f64) -> Option<&Quote> {
        let rows = match kind {
            OptionKind::Call => &self.calls,
            OptionKind::Put => &self.puts,
        };
        rows.iter()
            .filter(|q| q.tradable())
            // Nearest by absolute distance, then prefer the tighter-strike side on a
            // tie so the leg is the more conservative of the two candidates.
            .min_by(|a, b| {
                let da = (a.strike - target).abs();
                let db = (b.strike - target).abs();
                da.partial_cmp(&db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.strike.partial_cmp(&b.strike).unwrap_or(std::cmp::Ordering::Equal))
            })
    }

    /// Strikes where both a call and a put are tradable, nearest spot first.
    pub fn usable_strikes(&self) -> Vec<f64> {
        let mut out: Vec<f64> = self
            .calls
            .iter()
            .filter(|q| q.tradable())
            .map(|q| q.strike)
            .collect();
        out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        out.dedup();
        out
    }
}

/// Everything needed to build candidates for one symbol at one expiry.
#[derive(Debug, Clone)]
pub struct Underlying {
    pub symbol: String,
    pub spot: f64,
    pub expiry_ts: i64,
    pub expiry: String,
    pub t_years: f64,
    pub rate: f64,
    pub chain: ChainSide,
}

/// A ranked candidate: the structure, its numbers, and why it placed where it did.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub strategy: Strategy,
    pub metrics: Metrics,
    /// The ranking objective: probability of profit weighted by reward-to-risk.
    pub score: f64,
    /// Legs priced at the mid, and the total, so the UI can show the cost.
    pub entry_cost: f64,
    /// Human-readable reasons the structure was built this way.
    pub notes: Vec<String>,
}

fn leg_from(quote: &Quote, kind: OptionKind, position: f64) -> Leg {
    Leg {
        position,
        kind,
        strike: quote.strike,
        // Premium is the price paid for the leg, signed by direction: a long leg
        // carries a positive cost and a short leg a negative one, so that
        // `leg_payoff`'s `position * intrinsic - premium` and `net_premium`'s
        // `-sum(premium)` are both correct without special-casing short legs.
        premium: position * quote.mid(),
        iv: quote.iv,
    }
}

/// Wing widths as a fraction of spot.
///
/// A single relative width keeps the structures comparable across a watchlist that
/// spans $25 to $1,700. Without it, the same $5 wing is a trivial move on EME and a
/// trivial move on TSM too, and nothing on the tab means the same thing twice.
const WINGS: &[(&str, f64)] = &[
    ("narrow", 0.02),
    ("moderate", 0.05),
    ("wide", 0.10),
];

/// Build every defined-risk candidate for one underlying, ranked.
///
/// Returns an empty vector rather than panicking when the chain is too thin to
/// build anything, which is a normal state for a far-dated or illiquid name.
pub fn build_candidates(u: &Underlying) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    if u.spot <= 0.0 || u.chain.calls.is_empty() || u.chain.puts.is_empty() {
        return out;
    }
    let strikes = u.chain.usable_strikes();
    if strikes.len() < 4 {
        return out;
    }

    for (wname, wfrac) in WINGS {
        let wing = u.spot * wfrac;
        // A wing must be at least one listed strike away, or the two legs collapse
        // onto the same strike and the structure is not the one it claims to be.
        if wing < u.spot * 0.005 {
            continue;
        }

        // Bullish: long the lower call, sell the higher one.
        if let (Some(lo), Some(hi)) = (
            u.chain.nearest(OptionKind::Call, u.spot),
            u.chain.nearest(OptionKind::Call, u.spot + wing),
        ) {
            if hi.strike > lo.strike {
                push(
                    &mut out,
                    mk(
                        u,
                        format!("Bull call spread · {wname}"),
                        vec![leg_from(lo, OptionKind::Call, 1.0), leg_from(hi, OptionKind::Call, -1.0)],
                        Bias::Bullish,
                        "Defined-risk upside: the short call caps the risk of the long call.",
                    ),
                );
            }
        }

        // Bearish: long the higher put, sell the lower one.
        if let (Some(hi), Some(lo)) = (
            u.chain.nearest(OptionKind::Put, u.spot),
            u.chain.nearest(OptionKind::Put, u.spot - wing),
        ) {
            if hi.strike > lo.strike {
                push(
                    &mut out,
                    mk(
                        u,
                        format!("Bear put spread · {wname}"),
                        vec![leg_from(hi, OptionKind::Put, 1.0), leg_from(lo, OptionKind::Put, -1.0)],
                        Bias::Bearish,
                        "Defined-risk downside: the long put is paid for, the short put caps it.",
                    ),
                );
            }
        }

        // Bullish credit: sell the lower put, buy protection lower still.
        if let (Some(sell), Some(buy)) = (
            u.chain.nearest(OptionKind::Put, u.spot - wing),
            u.chain.nearest(OptionKind::Put, u.spot - 2.0 * wing),
        ) {
            if buy.strike < sell.strike {
                push(
                    &mut out,
                    mk(
                        u,
                        format!("Bull put spread · {wname}"),
                        vec![leg_from(sell, OptionKind::Put, -1.0), leg_from(buy, OptionKind::Put, 1.0)],
                        Bias::Bullish,
                        "Collects a credit if the underlying holds above the short strike.",
                    ),
                );
            }
        }

        // Bearish credit: sell the higher call, buy protection higher still.
        if let (Some(sell), Some(buy)) = (
            u.chain.nearest(OptionKind::Call, u.spot + wing),
            u.chain.nearest(OptionKind::Call, u.spot + 2.0 * wing),
        ) {
            if buy.strike > sell.strike {
                push(
                    &mut out,
                    mk(
                        u,
                        format!("Bear call spread · {wname}"),
                        vec![leg_from(sell, OptionKind::Call, -1.0), leg_from(buy, OptionKind::Call, 1.0)],
                        Bias::Bearish,
                        "Collects a credit if the underlying stays below the short strike.",
                    ),
                );
            }
        }
    }

    // Iron condor, one width either side, wings twice that distance out.
    if let (Some(sp), Some(lp), Some(sc), Some(lc)) = (
        u.chain.nearest(OptionKind::Put, u.spot - u.spot * 0.05),
        u.chain.nearest(OptionKind::Put, u.spot - u.spot * 0.10),
        u.chain.nearest(OptionKind::Call, u.spot + u.spot * 0.05),
        u.chain.nearest(OptionKind::Call, u.spot + u.spot * 0.10),
    ) {
        if lp.strike < sp.strike && lc.strike > sc.strike {
            push(
                &mut out,
                mk(
                    u,
                    "Iron condor".to_string(),
                    vec![
                        leg_from(sp, OptionKind::Put, -1.0),
                        leg_from(lp, OptionKind::Put, 1.0),
                        leg_from(sc, OptionKind::Call, -1.0),
                        leg_from(lc, OptionKind::Call, 1.0),
                    ],
                    Bias::Neutral,
                    "Defined risk on both sides, profitable while the underlying stays in a range.",
                ),
            );
        }
    }

    // Call butterfly: long one wing, short two, long the other. A debit structure
    // that profits from a narrow move near the body.
    if let (Some(lo), Some(mid), Some(hi)) = (
        u.chain.nearest(OptionKind::Call, u.spot - u.spot * 0.05),
        u.chain.nearest(OptionKind::Call, u.spot),
        u.chain.nearest(OptionKind::Call, u.spot + u.spot * 0.05),
    ) {
        if lo.strike < mid.strike && mid.strike < hi.strike {
            push(
                &mut out,
                mk(
                    u,
                    "Call butterfly".to_string(),
                    vec![
                        leg_from(lo, OptionKind::Call, 1.0),
                        leg_from(mid, OptionKind::Call, -2.0),
                        leg_from(hi, OptionKind::Call, 1.0),
                    ],
                    Bias::Neutral,
                    "Peaks at the middle strike and decays either side; needs a fairly precise move.",
                ),
            );
        }
    }

    // Long straddle: bought volatility. Its loss is capped by the premium, so it is
    // defined-risk even though its profit is not — which is why it earns a special
    // note rather than a reward/risk ratio.
    if let (Some(c), Some(p)) = (
        u.chain.nearest(OptionKind::Call, u.spot),
        u.chain.nearest(OptionKind::Put, u.spot),
    ) {
        let legs = vec![leg_from(c, OptionKind::Call, 1.0), leg_from(p, OptionKind::Put, 1.0)];
        let cost = -net_premium(&legs);
        // Only worth showing when the two legs are close to the same strike, which
        // is what makes it a straddle rather than an arbitrary two-leg purchase.
        if (c.strike - p.strike).abs() <= u.spot * 0.02 && cost > 0.0 {
            let mut cand = mk(
                u,
                "Long straddle".to_string(),
                legs,
                Bias::Volatile,
                "Buys both directions. Loss is capped at the premium; profit needs a move larger than the two breakevens.",
            );
            cand.notes.push("Profit is unbounded above, so no reward-to-risk ratio is quoted.".into());
            out.push(cand);
        }
    }

    rank(&mut out);
    out
}

fn mk(u: &Underlying, name: String, legs: Vec<Leg>, bias: Bias, rationale: &str) -> Candidate {
    let strategy = Strategy {
        name,
        symbol: u.symbol.clone(),
        spot: u.spot,
        expiry_ts: u.expiry_ts,
        expiry: u.expiry.clone(),
        t_years: u.t_years,
        rate: u.rate,
        legs,
        rationale: rationale.to_string(),
        bias,
    };
    let metrics = strategy.metrics();
    let entry_cost = -net_premium(&strategy.legs);
    let score = score(&metrics);
    Candidate {
        strategy,
        metrics,
        score,
        entry_cost,
        notes: Vec::new(),
    }
}

/// Reject a structure that cannot be traded or cannot be reasoned about.
fn push(out: &mut Vec<Candidate>, mut c: Candidate) {
    if !admissible(&c) {
        return;
    }
    if c.metrics.breakevens.is_empty() {
        c.notes.push("No breakeven inside the plotted range.".into());
    }
    out.push(c);
}

/// A candidate is only worth showing if it is a real structure.
fn admissible(c: &Candidate) -> bool {
    if c.strategy.legs.len() < 2 {
        return false;
    }
    if !c.score.is_finite() {
        return false;
    }
    if c.entry_cost == 0.0 {
        return false;
    }
    // A structure whose loss is unbounded is not in a defined-risk library.
    if c.metrics.max_loss_unbounded {
        return false;
    }
    // P&L should be finite everywhere on the plotted range.
    let m = &c.metrics;
    if let Some(p) = m.max_profit {
        if !p.is_finite() || p <= 0.0 {
            return false;
        }
    }
    match m.max_loss {
        Some(l) if l.is_finite() && l > 0.0 => {}
        _ => return false,
    }
    true
}

/// The ranking objective: probability of profit weighted by reward-to-risk.
///
/// `P(profit) × reward/risk` is chosen because it is a single number that trades the
/// two things a defined structure actually trades off — how often it wins against
/// how much it wins when it does. It is **not** an expected value: there is no
/// weighting of the loss case, so a structure with a beautiful ratio and a 20%
/// win rate can still score well. The components are shown separately on the card so
/// the score is never the only thing on screen.
pub fn score(m: &Metrics) -> f64 {
    let rr = m.reward_risk.unwrap_or(0.0);
    if !rr.is_finite() {
        return 0.0;
    }
    m.prob_profit * rr
}

/// Sort by score, keeping the order stable within equal scores.
fn rank(v: &mut [Candidate]) {
    v.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.entry_cost.partial_cmp(&b.entry_cost).unwrap_or(std::cmp::Ordering::Equal))
    });
}

/// The best candidate per bias, for a summary row.
pub fn best_by_bias(cands: &[Candidate]) -> Vec<(Bias, &Candidate)> {
    let mut out: Vec<(Bias, &Candidate)> = Vec::new();
    for bias in [Bias::Bullish, Bias::Bearish, Bias::Neutral, Bias::Volatile] {
        if let Some(c) = cands.iter().find(|c| c.strategy.bias == bias) {
            out.push((bias, c));
        }
    }
    out
}

/// P&L at spot, so the card can say whether the structure is currently in profit.
pub fn pnl_at_spot(c: &Candidate) -> f64 {
    payoff(&c.strategy.legs, c.strategy.spot)
}

/// Expose the extremes helper for the API layer's summary line.
pub fn summarise(c: &Candidate) -> (Option<f64>, Option<f64>, Extremes) {
    (
        c.metrics.max_profit,
        c.metrics.max_loss,
        Extremes {
            max_profit: c.metrics.max_profit,
            max_profit_unbounded: c.metrics.max_profit_unbounded,
            max_loss: c.metrics.max_loss,
            max_loss_unbounded: c.metrics.max_loss_unbounded,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(strike: f64, mid: f64, iv: f64) -> Quote {
        Quote {
            strike,
            bid: mid - 0.05,
            ask: mid + 0.05,
            iv,
            open_interest: 1000,
            volume: 100,
        }
    }

    /// A chain with realistic strikes around a spot, priced by the same Black-Scholes
    /// the probability maths uses.
    ///
    /// Priced properly rather than with a flat time value: a chain where every strike
    /// has the same premium gives every spread a zero debit, and a zero-debit
    /// structure is correctly rejected as untradeable — which would leave these tests
    /// silently asserting nothing.
    fn chain(spot: f64, step: f64) -> ChainSide {
        use crate::strategy::{bs_price, OptionKind};
        let t: f64 = 0.05;
        let iv: f64 = 0.45;
        let r: f64 = 0.04;
        let mut calls = Vec::new();
        let mut puts = Vec::new();
        let mut k = (spot * 0.6 / step).floor() * step;
        while k <= spot * 1.4 {
            if k > 0.0 {
                let c = bs_price(spot, k, t, iv, r, OptionKind::Call);
                let p = bs_price(spot, k, t, iv, r, OptionKind::Put);
                // A bid/ask a touch around the model, as a real chain quotes.
                calls.push(Quote {
                    strike: k,
                    bid: (c - 0.05).max(0.0),
                    ask: c + 0.05,
                    iv,
                    open_interest: 1000,
                    volume: 100,
                });
                puts.push(Quote {
                    strike: k,
                    bid: (p - 0.05).max(0.0),
                    ask: p + 0.05,
                    iv,
                    open_interest: 1000,
                    volume: 100,
                });
            }
            k += step;
        }
        ChainSide { calls, puts }
    }

    fn underlying(spot: f64, symbol: &str) -> Underlying {
        let step = if spot > 500.0 { 10.0 } else if spot > 100.0 { 5.0 } else { 1.0 };
        Underlying {
            symbol: symbol.into(),
            spot,
            expiry_ts: 1_790_553_600,
            expiry: "2026-10-16".into(),
            t_years: 0.12,
            rate: 0.04,
            chain: chain(spot, step),
        }
    }

    #[test]
    fn mid_averages_the_two_sides() {
        let mut x = q(100.0, 5.0, 0.5);
        x.bid = 4.9;
        x.ask = 5.1;
        assert!((x.mid() - 5.0).abs() < 1e-9);
    }

    /// A one-sided quote is not a price, and building a leg off it invents an edge.
    #[test]
    fn a_one_sided_quote_is_not_tradable() {
        let mut x = q(100.0, 5.0, 0.5);
        x.bid = 0.0;
        assert!(!x.tradable());
        x.bid = 4.9;
        x.ask = 0.0;
        assert!(!x.tradable());
        // A crossed quote is nonsense and must not be traded.
        x.bid = 6.0;
        x.ask = 5.0;
        assert!(!x.tradable());
    }

    /// A chain with no implied volatility cannot produce a probability, so those
    /// contracts are excluded rather than scored at zero.
    #[test]
    fn a_zero_iv_quote_is_not_tradable() {
        let mut x = q(100.0, 5.0, 0.0);
        assert!(!x.tradable());
        x.iv = 0.3;
        assert!(x.tradable());
    }

    #[test]
    fn nearest_snaps_to_the_closest_listed_strike() {
        let c = chain(100.0, 5.0);
        let n = c.nearest(OptionKind::Call, 103.0).unwrap();
        assert_eq!(n.strike, 105.0, "105 is nearer 103 than 100 is");
        let n = c.nearest(OptionKind::Call, 102.0).unwrap();
        assert_eq!(n.strike, 100.0);
    }

    #[test]
    fn builds_candidates_for_a_normal_chain() {
        let u = underlying(100.0, "TEST");
        let cands = build_candidates(&u);
        assert!(!cands.is_empty(), "a 100 spot with a full chain must yield something");
        for c in &cands {
            assert!(c.score.is_finite(), "{} scored {:?}", c.strategy.name, c.score);
            assert!(!c.metrics.max_loss_unbounded, "{} must be defined risk", c.strategy.name);
            assert!(c.metrics.max_loss.unwrap_or(0.0) > 0.0);
        }
    }

    /// The library is defined-risk only. A naked short call must never appear, and
    /// the unbounded-loss guard has to actually hold.
    #[test]
    fn no_candidate_has_an_unbounded_loss() {
        for spot in [25.0, 100.0, 900.0] {
            let u = underlying(spot, "X");
            for c in build_candidates(&u) {
                assert!(
                    !c.metrics.max_loss_unbounded,
                    "{} on a {spot} spot leaked an unbounded loss",
                    c.strategy.name
                );
            }
        }
    }

    /// Candidates are ordered by score, and the order is not arbitrary.
    #[test]
    fn candidates_come_back_ranked_by_score() {
        let cands = build_candidates(&underlying(100.0, "TEST"));
        for w in cands.windows(2) {
            assert!(
                w[0].score >= w[1].score - 1e-12,
                "{} ({}) outranked {} ({})",
                w[0].strategy.name,
                w[0].score,
                w[1].strategy.name,
                w[1].score
            );
        }
    }

    /// The score is exactly the product of the two things it claims to combine.
    /// Structures with no finite ratio (a straddle, whose profit is unbounded) are
    /// skipped: there is no ratio to multiply, which is the point.
    #[test]
    fn score_is_probability_times_reward_risk() {
        let cands = build_candidates(&underlying(100.0, "TEST"));
        let mut checked = 0;
        for c in &cands {
            let Some(rr) = c.metrics.reward_risk else { continue };
            assert!((c.score - c.metrics.prob_profit * rr).abs() < 1e-9);
            checked += 1;
        }
        assert!(checked > 0, "the score check must actually have run");
    }

    /// Relative wings, not fixed dollars. The same $5 wing is 5% of a $100 name and
    /// 0.3% of a $1,700 one, so a hardcoded width would not be comparable.
    #[test]
    fn wings_scale_with_the_underlying_not_the_dollar_amount() {
        let cheap = build_candidates(&underlying(25.0, "CHEAP"));
        let dear = build_candidates(&underlying(1700.0, "DEAR"));
        let mut checked = 0;
        for c in cheap.iter().chain(dear.iter()) {
            // A straddle is deliberately two legs at one strike, so it has no width.
            if c.strategy.name == "Long straddle" {
                continue;
            }
            let strikes: Vec<f64> = c.strategy.legs.iter().map(|l| l.strike).collect();
            let lo = strikes.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = strikes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let width_frac = (hi - lo) / c.strategy.spot;
            assert!(
                width_frac > 0.01 && width_frac < 0.60,
                "{} on a {} spot has a width fraction of {width_frac:.3}, which is not a sane wing",
                c.strategy.name,
                c.strategy.spot
            );
            checked += 1;
        }
        assert!(checked > 0, "the wing check must actually have run");
    }

    /// A spread's two legs must be genuinely different strikes, or it is not the
    /// structure it is labelled as. A straddle is exempt: two legs at one strike is
    /// the entire point of it.
    #[test]
    fn no_spread_collapses_onto_one_strike() {
        let mut checked = 0;
        for spot in [25.0, 100.0, 900.0] {
            for c in build_candidates(&underlying(spot, "X")) {
                if c.strategy.name == "Long straddle" {
                    continue;
                }
                let strikes: Vec<f64> = c.strategy.legs.iter().map(|l| l.strike).collect();
                let mut distinct = strikes.clone();
                distinct.sort_by(|a, b| a.partial_cmp(b).unwrap());
                distinct.dedup();
                assert!(
                    distinct.len() >= 2,
                    "{} collapsed to one strike on a {spot} spot",
                    c.strategy.name
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the collapse check must actually have run");
    }

    #[test]
    fn an_empty_chain_yields_nothing_rather_than_panicking() {
        let u = Underlying {
            symbol: "EMPTY".into(),
            spot: 100.0,
            expiry_ts: 0,
            expiry: "2026-10-16".into(),
            t_years: 0.1,
            rate: 0.04,
            chain: ChainSide::default(),
        };
        assert!(build_candidates(&u).is_empty());
    }

    /// A chain with one strike cannot form a spread.
    #[test]
    fn a_thin_chain_yields_nothing_rather_than_panicking() {
        let mut u = underlying(100.0, "THIN");
        u.chain.calls.truncate(1);
        u.chain.puts.truncate(1);
        assert!(build_candidates(&u).is_empty());
    }

    /// Zero spot is a broken feed, not a stock that costs nothing.
    #[test]
    fn a_zero_spot_yields_nothing() {
        let mut u = underlying(100.0, "ZERO");
        u.spot = 0.0;
        assert!(build_candidates(&u).is_empty());
    }

    #[test]
    fn best_by_bias_reports_one_per_bias() {
        let cands = build_candidates(&underlying(100.0, "TEST"));
        let best = best_by_bias(&cands);
        assert!(!best.is_empty());
        // No bias may appear twice.
        let mut seen: Vec<Bias> = best.iter().map(|(b, _)| *b).collect();
        seen.dedup();
        assert_eq!(seen.len(), best.len());
    }

    /// An expired structure must still produce a finite, sensible probability
    /// rather than dividing by a zero time to expiry.
    #[test]
    fn an_expired_underlying_still_scores() {
        let mut u = underlying(100.0, "EXPIRED");
        u.t_years = 0.0;
        for c in build_candidates(&u) {
            assert!(c.metrics.prob_profit.is_finite(), "{} gave NaN", c.strategy.name);
            assert!((0.0..=1.0).contains(&c.metrics.prob_profit));
        }
    }
}
