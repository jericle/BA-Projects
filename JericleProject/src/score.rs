//! News scoring: de-duplication, clustering, ticker tagging, sentiment and salience.
//!
//! The salience score is intentionally simple and fully explainable — every
//! component is surfaced in the UI so the ranking can be audited rather than trusted.

use std::collections::{HashMap, HashSet};

use crate::model::{NewsItem, ScoreBreakdown};

/// A news item before scoring.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub title: String,
    pub source: String,
    pub url: String,
    pub published_ts: i64,
    /// Tickers asserted by the source feed itself (e.g. Yahoo `relatedTickers`).
    pub feed_tickers: Vec<String>,
    /// Query that surfaced this item, for debugging.
    pub origin: String,
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "for", "of", "to", "in", "on", "at", "by", "with", "from",
    "as", "is", "are", "was", "were", "be", "been", "it", "its", "this", "that", "these", "those",
    "will", "has", "have", "had", "not", "up", "down", "over", "into", "after", "before", "says",
    "say", "said", "new", "more", "most", "than", "then", "they", "their", "we", "you", "he", "she",
    "his", "her", "our", "your", "who", "what", "how", "why", "when", "which", "amid", "could",
    "would", "should", "may", "might", "must", "can", "also", "just", "now", "here",
];

/// Terms that mark a headline as potentially market-moving.
const IMPACT_TERMS: &[&str] = &[
    "earnings", "results", "guidance", "forecast", "preannounce", "pre-announce", "outlook",
    "revenue", "profit", "margin", "eps", "dividend", "buyback", "guidance cut", "cuts outlook",
    "fda", "approval", "approved", "cleared", "clearance", "antitrust", "merger", "acquisition",
    "acquire", "acquired", "merger", "takeover", "spin-off", "spinoff", "ipo", "downgrade",
    "upgrade", "downgraded", "upgraded", "sec", "investigation", "probe", "lawsuit", "sue", "sued",
    "recall", "warning", "fraud", "accounting", "restatement", "bankruptcy", "default", "layoffs",
    "tariff", "export", "export control", "sanction", "ban", "blacklist", "antitrust", "doj", "ftc",
    "fed", "fomc", "rate hike", "rate cut", "powell", "ecb", "cpi", "inflation", "jobs report",
    "payrolls", "strike", "shutdown", "cyber", "hack", "breach", "outage", "short squeeze", "bankrupt",
    "ceo", "steps down", "resigns", "board", "dividend cut", "writedown", "impairment", "trial",
    "verdict", "ruling", "ban", "crackdown", "crisis", "emergency", "bailout", "credit", "default",
];

/// Finance-tuned sentiment lexicon. Positive words raise the score.
const SENTIMENT: &[(&str, f64)] = &[
    // positive
    ("beat", 0.7), ("beats", 0.7), ("surge", 0.8), ("surges", 0.8), ("surged", 0.8), ("soar", 0.9),
    ("soars", 0.9), ("soared", 0.9), ("jump", 0.6), ("jumps", 0.6), ("jumped", 0.6), ("rally", 0.7),
    ("rallies", 0.7), ("rallied", 0.7), ("gain", 0.4), ("gains", 0.4), ("gained", 0.4), ("rise", 0.4),
    ("rises", 0.4), ("rose", 0.4), ("climb", 0.5), ("climbs", 0.5), ("climbed", 0.5), ("higher", 0.3),
    ("record", 0.5), ("high", 0.3), ("profit", 0.5), ("profits", 0.5), ("upgrade", 0.8),
    ("upgraded", 0.8), ("outperform", 0.7), ("bullish", 0.8), ("boost", 0.6), ("boosts", 0.6),
    ("boosted", 0.6), ("strong", 0.5), ("stronger", 0.6), ("robust", 0.5), ("growth", 0.4),
    ("growing", 0.3), ("expand", 0.4), ("expands", 0.4), ("approval", 0.6), ("approved", 0.6),
    ("wins", 0.6), ("won", 0.6), ("win", 0.5), ("breakthrough", 0.7), ("partnership", 0.4),
    ("buyback", 0.5), ("dividend", 0.3), ("optimistic", 0.5), ("recovery", 0.5), ("rebound", 0.6),
    ("rebounding", 0.6), ("tops", 0.6), ("topped", 0.6), ("exceeds", 0.6), ("raise", 0.4),
    ("raised", 0.4), ("hike", 0.3), ("hikes", 0.3), ("hiked", 0.3), ("orders", 0.3), ("demand", 0.3),
    ("demand surges", 0.8), ("resilient", 0.4), ("punches", 0.5), ("stellar", 0.7), ("soaring", 0.9),
    // negative
    ("miss", -0.7), ("misses", -0.7), ("missed", -0.7), ("plunge", -0.9), ("plunges", -0.9),
    ("plunged", -0.9), ("slump", -0.8), ("slumps", -0.8), ("slumped", -0.8), ("tumble", -0.8),
    ("tumbles", -0.8), ("tumbled", -0.8), ("crash", -0.9), ("crashes", -0.9), ("crashed", -0.9),
    ("fall", -0.5), ("falls", -0.5), ("fell", -0.5), ("drop", -0.5), ("drops", -0.5), ("dropped", -0.5),
    ("decline", -0.5), ("declines", -0.5), ("declined", -0.5), ("sink", -0.6), ("sinks", -0.6),
    ("sank", -0.6), ("slide", -0.6), ("slides", -0.6), ("slid", -0.6), ("lower", -0.3),
    ("loss", -0.6), ("losses", -0.6), ("downgrade", -0.8), ("downgraded", -0.8), ("bearish", -0.8),
    ("warn", -0.6), ("warns", -0.6), ("warned", -0.6), ("warning", -0.6), ("risk", -0.4),
    ("risks", -0.4), ("concern", -0.4), ("concerns", -0.4), ("fear", -0.6), ("fears", -0.6),
    ("lawsuit", -0.6), ("sue", -0.5), ("sued", -0.6), ("probe", -0.6), ("investigation", -0.5),
    ("recall", -0.7), ("fraud", -0.9), ("bankruptcy", -0.9), ("default", -0.8), ("layoffs", -0.6),
    ("layoff", -0.6), ("cuts", -0.5), ("cut", -0.4), ("slash", -0.6), ("slashes", -0.6), ("slashed", -0.6),
    ("downturn", -0.7), ("recession", -0.8), ("weak", -0.5), ("weaker", -0.6), ("weakness", -0.6),
    ("slow", -0.4), ("slows", -0.4), ("slowed", -0.4), ("halt", -0.5), ("halts", -0.5), ("halted", -0.5),
    ("suspend", -0.5), ("suspended", -0.5), ("ban", -0.6), ("banned", -0.6), ("sanction", -0.6),
    ("sanctions", -0.6), ("tariff", -0.4), ("tax", -0.3), ("laws", -0.3), ("crackdown", -0.6),
    ("crisis", -0.7), ("plunging", -0.9), ("falling", -0.5), ("sink", -0.6), ("negative", -0.4),
    ("underperform", -0.7), ("cuts outlook", -0.9), ("writedown", -0.7), ("impairment", -0.6),
    ("resign", -0.5), ("resigns", -0.5), ("steps down", -0.5), ("outage", -0.6), ("breach", -0.7),
    ("hack", -0.7), ("short", -0.3), ("shorts", -0.3), ("overvalued", -0.5), ("bubble", -0.5),
    ("selloff", -0.8), ("sell-off", -0.8), ("rout", -0.9), ("slumps", -0.8), ("stumbles", -0.5),
];

const NEGATIONS: &[&str] = &["not", "no", "never", "without", "fails", "fail", "failed", "avoids", "avoided", "denies", "denied", "halts", "less"];

/// Boilerplate and clickbait that the broad queries inevitably surface.
const JUNK_PATTERNS: &[&str] = &[
    "stock price, news, quote",
    "stock price, price, news",
    "our pick of",
    "is it too late to buy",
    "should you buy",
    "here's what you need to know",
    "here's why",
    "can ai deepen",
    "here's how",
    "best stocks to buy",
    "stocks to watch",
    "portfolio strategy",
    "price target and rating",
    "q2 earnings call transcript",
];

/// Markers that a story is actually about the US equity market. Note "S&P 500"
/// rather than bare "S&P", so S&P Global the ratings agency does not match.
const US_MARKERS: &[&str] = &[
    "s&p 500", "s&p500", "s&p midcap", "nasdaq", "dow jones", "dow ", "wall street", "wall st",
    "u.s.", "us stocks", "us market", "federal reserve", "fed", "fomc", "nasdaq 100",
    "russell 2000", "nyse", "premarket", "pre-market", "after hours", "stock market", "treasury",
    "10-year", "white house", "congress", "sec ", "nasdaq composite", "ibit ",
];

pub const DEFAULT_ALIASES: &[(&str, &[&str])] = &[
    ("NVDA", &["nvidia"]),
    ("AMD", &["advanced micro devices", "amd"]),
    ("AVGO", &["broadcom"]),
    ("TSM", &["taiwan semiconductor", "tsmc"]),
    ("ARM", &["arm holdings", "arm"]),
    ("MRVL", &["marvell", "marvell technology"]),
    ("INTC", &["intel"]),
    ("ORCL", &["oracle"]),
    ("MSFT", &["microsoft"]),
    ("GOOGL", &["google", "alphabet"]),
    ("MU", &["micron", "micron technology"]),
    ("WDC", &["western digital"]),
    ("STX", &["seagate"]),
    ("SNDK", &["sandisk", "san disk"]),
    ("CEG", &["constellation energy"]),
    ("VST", &["vistra"]),
    ("NRG", &["nrg"]),
    ("TLN", &["talen", "talen energy"]),
    ("GEV", &["ge vernova", "vernova"]),
    ("ETN", &["eaton"]),
    ("PWR", &["quanta services"]),
    ("EME", &["emcor"]),
    ("DELL", &["dell"]),
    ("ANET", &["arista", "arista networks"]),
    ("VRT", &["vertiv"]),
    ("COHR", &["coherent"]),
    ("LITE", &["lumentum"]),
    ("PLTR", &["palantir"]),
    ("SMCI", &["super micro", "supermicro"]),
    ("AMAT", &["applied materials"]),
    ("LRCX", &["lam research"]),
    ("KLAC", &["kla"]),
    ("ASML", &["asml"]),
    ("QCOM", &["qualcomm"]),
    ("MRNA", &["moderna"]),
    ("CCJ", &["cameco"]),
    ("NEE", &["nextera energy"]),
    ("AES", &["aes"]),
    ("SO", &["southern company"]),
    ("DUK", &["duke energy"]),
    ("AAPL", &["apple"]),
    ("AMZN", &["amazon"]),
    ("META", &["meta platforms"]),
    ("TSLA", &["tesla"]),
    ("NFLX", &["netflix"]),
    ("JPM", &["jpmorgan"]),
    ("BAC", &["bank of america"]),
    ("GS", &["goldman sachs"]),
    ("XOM", &["exxon"]),
    ("CVX", &["chevron"]),
    ("LLY", &["eli lilly", "lilly"]),
    ("UNH", &["unitedhealth"]),
];

pub fn alias_map(extra: &HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (ticker, names) in DEFAULT_ALIASES {
        map.insert(ticker.to_string(), names.iter().map(|s| s.to_string()).collect());
    }
    for (ticker, names) in extra {
        map.insert(ticker.to_ascii_uppercase(), names.iter().map(|s| s.to_lowercase()).collect());
    }
    map
}

fn is_stopword(t: &str) -> bool {
    STOPWORDS.contains(&t)
}

/// Lowercase alphanumeric tokens, stopwords removed.
pub fn tokens(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric() && c != '$')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 1 && !is_stopword(t))
        .collect()
}

/// Ticker tag detection.
///
/// * `$AAPL` and bare uppercase `AAPL` always match.
/// * Company aliases match on whole tokens, or n-grams for multi-word aliases.
/// * Aliases shorter than 5 characters (e.g. `ARM`, `MU`, `NRG`) only match when the
///   token is uppercase in the original text or `$`-prefixed, so "arm" and "muscle"
///   do not tag the wrong ticker.
pub fn tag_tickers(title: &str, aliases: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // Case-preserving tokens, so uppercase detection is possible.
    let raw: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric() && c != '$')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    let lower: Vec<String> = raw.iter().map(|t| t.to_lowercase()).collect();

    for i in 0..lower.len() {
        let bare = lower[i].trim_start_matches('$');
        let was_dollar = lower[i].starts_with('$');
        let is_upper = raw[i].chars().all(|c| !c.is_lowercase()) && raw[i].chars().any(|c| c.is_alphabetic());
        let clean = bare.trim_end_matches('s'); // crude plural handling

        // Direct ticker hit.
        if (was_dollar || is_upper) && bare.len() <= 5 {
            let t = bare.to_ascii_uppercase();
            if aliases.contains_key(&t) {
                push_unique(&mut out, t);
                continue;
            }
        }

        // Company alias: single token.
        if bare.len() >= 5 {
            for (ticker, names) in aliases {
                if names.iter().any(|n| n.split_whitespace().count() == 1 && n == clean) {
                    push_unique(&mut out, ticker.clone());
                }
            }
        }
    }

    // Company alias: multi-word n-grams.
    for (ticker, names) in aliases {
        for name in names {
            let parts: Vec<&str> = name.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            for window in lower.windows(parts.len()) {
                if window == parts {
                    push_unique(&mut out, ticker.clone());
                    break;
                }
            }
        }
    }

    out.sort();
    out
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// Finance-lexicon sentiment in [-1, 1]. Positive is bullish.
pub fn sentiment(title: &str) -> f64 {
    let lower = title.to_lowercase();
    let mut total = 0.0;
    let mut hits = 0usize;

    for (term, weight) in SENTIMENT {
        if let Some(pos) = lower.find(term) {
            // Ignore bare "cut" when it is part of "cut outlook" (handled separately)
            // and ignore "rate cut/hike" false friend for "cut" via negation window.
            let before = &lower[..pos];
            let negated = NEGATIONS.iter().any(|n| {
                before
                    .rsplit(|c: char| !c.is_alphabetic())
                    .find(|w| !w.is_empty())
                    .map(|w| w == *n)
                    .unwrap_or(false)
            });
            let effective = if negated { -weight * 0.8 } else { *weight };
            total += effective;
            hits += 1;
        }
    }

    if hits == 0 {
        return 0.0;
    }
    (total / (hits as f64).sqrt()).clamp(-1.0, 1.0)
}

/// Impact keyword hits, used both for scoring and for the UI's "why".
pub fn impact_hits(title: &str) -> Vec<String> {
    let lower = title.to_lowercase();
    let mut hits = Vec::new();
    for term in IMPACT_TERMS {
        if lower.contains(term) {
            hits.push((*term).to_string());
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

/// True for boilerplate and listicle filler that the broad queries drag in.
pub fn is_junk(title: &str) -> bool {
    let lower = title.to_lowercase();
    JUNK_PATTERNS.iter().any(|p| lower.contains(p))
}

/// True when the headline is plausibly about the US equity market.
pub fn is_us_market(title: &str) -> bool {
    let lower = title.to_lowercase();
    US_MARKERS.iter().any(|m| lower.contains(m))
}

/// Publisher credibility weight in [0, 1].
pub fn source_weight(publisher: &str) -> f64 {
    let p = publisher.to_lowercase();
    if p.contains("reuters") || p.contains("bloomberg") {
        1.0
    } else if p.contains("cnbc") || p.contains("wsj") || p.contains("wall street journal")
        || p.contains("financial times") || p.contains("barrons") || p.contains("bloomberg")
        || p.contains("investor's business daily") || p.contains("marketwatch") || p.contains("reuters")
    {
        0.85
    } else if p.contains("yahoo finance") || p.contains("yahoo") {
        0.7
    } else if p.contains("benzinga") || p.contains("investing.com") || p.contains("motley")
    {
        0.5
    } else if p.contains("google news") || p.is_empty() {
        0.3
    } else {
        0.6
    }
}

/// A group of near-identical headlines about the same story.
struct Cluster {
    tokens: HashSet<String>,
    outlets: HashSet<String>,
    members: Vec<usize>,
}

/// Cluster candidates by token overlap, then score everything.
pub fn rank(
    candidates: Vec<Candidate>,
    aliases: &HashMap<String, Vec<String>>,
    watchlist: &HashSet<String>,
    now_ts: i64,
    half_life_min: f64,
    top_n: usize,
    max_age_hours: i64,
) -> (Vec<NewsItem>, usize) {
    // De-duplicate by URL first, then by normalised title.
    let mut seen_url: HashSet<String> = HashSet::new();
    let mut seen_title: HashSet<String> = HashSet::new();
    // Junk is dropped outright: there is no version of a "Stock Price, News, Quote"
    // page that a trader wants at 08:25am.
    let mut junk_dropped = 0usize;
    let mut unique: Vec<Candidate> = Vec::new();
    for c in candidates {
        let url_key = c.url.trim().to_lowercase();
        let title_key = normalise_title(&c.title);
        if !url_key.is_empty() && !seen_url.insert(url_key) {
            continue;
        }
        if !title_key.is_empty() && !seen_title.insert(title_key) {
            continue;
        }
        if c.published_ts < now_ts - max_age_hours * 3600 {
            continue;
        }
        if is_junk(&c.title) {
            junk_dropped += 1;
            continue;
        }
        unique.push(c);
    }
    let total = unique.len();

    // Cluster by Jaccard overlap of significant tokens.
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut item_cluster: Vec<usize> = Vec::with_capacity(unique.len());
    for c in &unique {
        let toks: HashSet<String> = tokens(&c.title).into_iter().collect();
        let mut best: Option<(usize, f64)> = None;
        for (i, cl) in clusters.iter().enumerate() {
            let inter = toks.intersection(&cl.tokens).count();
            if inter == 0 {
                continue;
            }
            let union = toks.union(&cl.tokens).count();
            let j = inter as f64 / union as f64;
            if (j >= 0.5 || inter >= 4) && best.map(|(_, bj)| j > bj).unwrap_or(true) {
                best = Some((i, j));
            }
        }
        match best {
            Some((i, _)) => {
                clusters[i].tokens.extend(toks);
                clusters[i].outlets.insert(c.source.to_lowercase());
                clusters[i].members.push(item_cluster.len());
                item_cluster.push(i);
            }
            None => {
                let mut outlets = HashSet::new();
                outlets.insert(c.source.to_lowercase());
                item_cluster.push(clusters.len());
                clusters.push(Cluster { tokens: toks, outlets, members: vec![item_cluster.len() - 1] });
            }
        }
    }

    let mut items: Vec<NewsItem> = Vec::with_capacity(unique.len());
    for (c, &ci) in unique.iter().zip(item_cluster.iter()) {
        let cluster = &clusters[ci];
        let mut tickers = tag_tickers(&c.title, aliases);
        for t in &c.feed_tickers {
            let t = t.to_ascii_uppercase();
            if watchlist.contains(&t) {
                push_unique(&mut tickers, t);
            }
        }
        tickers.sort();
        tickers.dedup();

        let age_min = ((now_ts - c.published_ts).max(0) as f64) / 60.0;
        let recency = 30.0 * (-(std::f64::consts::LN_2) * age_min / half_life_min).exp();
        let n_outlets = cluster.outlets.len() as f64;
        let cluster_score = 25.0 * (1.0 + n_outlets).min(9.0f64).ln() / (1.0 + 8.0f64).ln();
        let hits = impact_hits(&c.title);
        let impact = 25.0 * (hits.len() as f64 / 2.0).min(1.0);
        let source = 10.0 * source_weight(&c.source);
        let watch_rel = if tickers.iter().any(|t| watchlist.contains(t)) { 10.0 } else { 0.0 };

        // Stories that are neither about the US market nor about a watchlist name are
        // damped rather than dropped, so an unusual but real story is still visible.
        let us_relevant = is_us_market(&c.title) || watch_rel > 0.0;
        let mut salience = (recency + cluster_score + impact + source + watch_rel).min(100.0);
        if !us_relevant {
            salience *= 0.6;
        }

        let mut outlets: Vec<String> = cluster.outlets.iter().cloned().collect();
        outlets.sort();
        outlets.truncate(6);

        items.push(NewsItem {
            id: fnv1a(&normalise_title(&c.title)),
            title: c.title.clone(),
            source: c.source.clone(),
            url: c.url.clone(),
            published_ts: c.published_ts,
            published_et: format_ts(c.published_ts),
            age_minutes: age_min.round() as i64,
            tickers,
            sentiment: sentiment(&c.title),
            salience,
            cluster_id: ci,
            breakdown: ScoreBreakdown {
                recency,
                cluster: cluster_score,
                impact,
                source,
                watchlist: watch_rel,
                impact_hits: hits,
                cluster_size: cluster.members.len(),
                outlets,
            },
        });
    }

    items.sort_by(|a, b| {
        b.salience
            .partial_cmp(&a.salience)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.published_ts.cmp(&a.published_ts))
    });

    // A trader wants the top 20 *stories*, not the top 20 articles. Keep the
    // highest-scoring member of each cluster; the rest are represented by
    // `breakdown.outlets`, which lists who else carried it.
    let mut seen_clusters: HashSet<usize> = HashSet::new();
    let mut collapsed: Vec<NewsItem> = Vec::with_capacity(items.len());
    for item in items {
        if seen_clusters.insert(item.cluster_id) {
            collapsed.push(item);
        }
    }

    collapsed.truncate(top_n);
    if std::env::var("OPENDASH_VERBOSE").is_ok() && junk_dropped > 0 {
        eprintln!("[news] dropped {junk_dropped} boilerplate item(s)");
    }
    (collapsed, total)
}

/// Lowercase, strip punctuation and whitespace, for dedup hashing.
fn normalise_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fnv1a(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn format_ts(ts: i64) -> String {
    match chrono::DateTime::from_timestamp(ts, 0) {
        Some(utc) => utc.with_timezone(&crate::marketclock::ET).format("%Y-%m-%d %H:%M ET").to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> HashMap<String, Vec<String>> {
        alias_map(&HashMap::new())
    }

    #[test]
    fn tags_ticker_and_company() {
        let m = map();
        let t = tag_tickers("NVIDIA beats on revenue, $NVDA rallies", &m);
        assert!(t.contains(&"NVDA".to_string()), "got {t:?}");
        let t = tag_tickers("Micron guides higher on HBM demand", &m);
        assert!(t.contains(&"MU".to_string()), "got {t:?}");
    }

    #[test]
    fn short_ticker_needs_uppercase() {
        let m = map();
        // lowercase "arm" must NOT tag ARM
        let t = tag_tickers("Regulators raise concerns about the arm of the law", &m);
        assert!(!t.contains(&"ARM".to_string()), "false positive: {t:?}");
        // uppercase ARM must tag
        let t = tag_tickers("ARM Holdings raises guidance", &m);
        assert!(t.contains(&"ARM".to_string()), "missed: {t:?}");
    }

    #[test]
    fn sentiment_polarity() {
        assert!(sentiment("Nvidia surges on earnings beat") > 0.3);
        assert!(sentiment("Nvidia plunges after guidance cut") < -0.3);
        assert_eq!(sentiment("Company schedules investor day"), 0.0);
    }

    #[test]
    fn impact_terms_detected() {
        let hits = impact_hits("Apple wins FDA approval for new drug");
        assert!(hits.iter().any(|h| h == "fda"));
        assert!(hits.iter().any(|h| h == "approval"));
    }

    #[test]
    fn junk_and_relevance_filters() {
        assert!(is_junk("Alphabet Inc. (GOOG) Stock Price, News, Quote & History"));
        assert!(is_junk("Is It Too Late to Buy IREN Stock After Its Monster Run?"));
        assert!(!is_junk("Nvidia beats on revenue"));
        assert!(is_us_market("S&P 500, Dow, Nasdaq End Week Higher"));
        assert!(is_us_market("Fed rate hike signals independence under strain"));
        assert!(!is_us_market("S&P shifts Czech outlook to positive as economy improves"));
    }

    #[test]
    fn off_topic_story_is_damped_not_dropped() {
        let wl: HashSet<String> = ["NVDA".to_string()].into_iter().collect();
        let now = 1_790_353_489;
        let cands = vec![
            // On-topic, on the watchlist, very fresh.
            Candidate { title: "Nvidia surges as S&P 500 futures climb".into(), source: "Reuters".into(), url: "a".into(), published_ts: now - 120, feed_tickers: vec![], origin: "q".into() },
            // Off-topic: no US marker, no watchlist ticker.
            Candidate { title: "Czech factory output rises on resilient exports".into(), source: "Reuters".into(), url: "b".into(), published_ts: now - 60, feed_tickers: vec![], origin: "q".into() },
        ];
        let (items, _) = rank(cands, &map(), &wl, now, 120.0, 20, 48);
        assert_eq!(items.len(), 2, "off-topic item is damped, not deleted");
        assert!(items[0].title.contains("Nvidia"), "on-topic story ranks first");
    }

    #[test]
    fn source_weights() {
        assert!(source_weight("Reuters") > source_weight("Yahoo Finance"));
        assert!(source_weight("Yahoo Finance") > source_weight("Google News"));
    }

    #[test]
    fn rank_dedupes_clusters_and_filters_by_age() {
        let wl: HashSet<String> = ["NVDA".to_string()].into_iter().collect();
        let now = 1_790_353_489;
        let cands = vec![
            // Same story, different outlet and wording: should cluster, not duplicate.
            Candidate { title: "Nvidia beats on revenue".into(), source: "Reuters".into(), url: "a".into(), published_ts: now - 600, feed_tickers: vec![], origin: "q".into() },
            Candidate { title: "Nvidia beats revenue, shares climb".into(), source: "Benzinga".into(), url: "b".into(), published_ts: now - 500, feed_tickers: vec![], origin: "q".into() },
            // Exact title duplicate: dropped.
            Candidate { title: "Nvidia beats on revenue".into(), source: "Yahoo Finance".into(), url: "c".into(), published_ts: now - 400, feed_tickers: vec![], origin: "q".into() },
            // 60 hours old, outside max_age_hours: dropped.
            Candidate { title: "Weather turns mild in the midwest".into(), source: "Reuters".into(), url: "d".into(), published_ts: now - 60 * 3600, feed_tickers: vec![], origin: "q".into() },
        ];
        let (items, total) = rank(cands, &map(), &wl, now, 120.0, 20, 48);
        assert_eq!(total, 2, "exact dup and stale item both dropped");
        // Two near-identical headlines are one story, so only one survives.
        assert_eq!(items.len(), 1, "cluster collapsed to a single story");
        assert!(items[0].title.contains("Nvidia"), "got {:?}", items[0].title);
        assert_eq!(items[0].breakdown.cluster_size, 2, "same story clustered");
        assert!(items[0].tickers.contains(&"NVDA".to_string()), "tagged: {:?}", items[0].tickers);
        assert_eq!(items[0].breakdown.outlets.len(), 2, "both outlets recorded");
    }
}
