//! The refresh pipeline: quotes + news in, one `Dashboard` out.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use crate::config::Config;
use crate::marketclock::{self, Et};
use crate::model::{Dashboard, Mover, PhaseInfo};
use crate::score::{self, Candidate};
use crate::sources::{gnews, trending, yahoo_chart, yahoo_news};
use crate::store::Store;

pub struct AppState {
    pub config: Config,
    pub client: reqwest::Client,
    pub store: Arc<Store>,
    pub dashboard: std::sync::RwLock<Arc<Dashboard>>,
    pub refresh_flag: AtomicBool,
    /// Guards against two refreshes running at once.
    pub refreshing: AtomicBool,
    /// Cookie + crumb session for Yahoo's options endpoint.
    pub options: Arc<crate::sources::yahoo_options::YahooOptions>,
    /// Cached option walls. Open interest is end-of-day data, so a long TTL is fine.
    pub options_cache: RwLock<HashMap<String, (i64, Arc<crate::options::OptionSummary>)>>,
}

impl AppState {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        let client = crate::http::client(20)?;
        let store = Arc::new(Store::open(&config.db_path())?);

        // Restore the last snapshot so the page is never blank. Its own warnings refer
        // to the fetch that produced it, so label them as historical.
        let restored = store.load_last().map(|mut d| {
            if !d.warnings.is_empty() {
                let mut w = vec!["showing the last saved snapshot; a refresh is in progress".into()];
                w.append(&mut d.warnings);
                d.warnings = w;
            }
            d
        });
        let placeholder = Dashboard {
            generated_ts: 0,
            generated_et: "never".into(),
            generated_local: "never".into(),
            phase: phase_info(&marketclock::now_et()),
            watchlist: vec![],
            news: vec![],
            all_news_count: 0,
            gainers: vec![],
            losers: vec![],
            trending: vec![],
            warnings: vec!["No refresh has run yet.".into()],
            news_queries: config.market_queries.clone(),
        };

        Ok(Arc::new(Self {
            config,
            client,
            store,
            dashboard: std::sync::RwLock::new(Arc::new(restored.unwrap_or(placeholder))),
            refresh_flag: AtomicBool::new(false),
            refreshing: AtomicBool::new(false),
            options: Arc::new(crate::sources::yahoo_options::YahooOptions::new()?),
            options_cache: RwLock::new(HashMap::new()),
        }))
    }

    /// Option walls for one symbol, cached. Yahoo first, CBOE as a fallback so a
    /// crumb failure does not leave the card permanently empty.
    pub async fn option_walls(
        &self,
        symbol: &str,
        ttl_secs: i64,
    ) -> Result<Arc<crate::options::OptionSummary>> {
        let symbol = symbol.trim().to_ascii_uppercase();
        let now = chrono::Utc::now().timestamp();

        {
            let cache = self.options_cache.read().await;
            if let Some((at, summary)) = cache.get(&symbol) {
                if now - at < ttl_secs {
                    return Ok(Arc::clone(summary));
                }
            }
        }

        let et = marketclock::now_et();
        // OPENDASH_OPTIONS_SOURCE=cboe forces the fallback feed, which is how the
        // fallback path gets exercised without waiting for Yahoo to break.
        let forced = std::env::var("OPENDASH_OPTIONS_SOURCE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let resolved = match self.options.current_month_summary(&symbol, &et).await {
            Ok(_) if forced == "cboe" => {
                match crate::sources::cboe::current_month_summary(&self.client, &symbol, &et).await {
                    Ok(mut c) => {
                        c.source = format!("{} (forced)", c.source);
                        c
                    }
                    Err(e) => anyhow::bail!("forced cboe: {e}"),
                }
            }
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string();
                match crate::sources::cboe::current_month_summary(&self.client, &symbol, &et).await {
                    Ok(mut s) => {
                        // Make the fallback visible rather than silently swapping feeds.
                        s.source = format!("cboe (yahoo: {})", truncate_mid(&msg, 48));
                        s
                    }
                    Err(cboe_err) => {
                        anyhow::bail!("yahoo: {msg}; cboe: {cboe_err}")
                    }
                }
            }
        };

        let summary = Arc::new(resolved);
        {
            let mut cache = self.options_cache.write().await;
            cache.insert(symbol, (now, Arc::clone(&summary)));
        }
        Ok(summary)
    }

    /// Current snapshot. The lock is only ever held to clone an `Arc`, so a
    /// std RwLock is the right tool and keeps this callable from sync code.
    pub fn snapshot(&self) -> Arc<Dashboard> {
        match self.dashboard.read() {
            Ok(d) => d.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn request_refresh(&self) {
        self.refresh_flag.store(true, Ordering::SeqCst);
    }
}

fn phase_info(et: &Et) -> PhaseInfo {
    let open = marketclock::next_cash_open(et);
    PhaseInfo {
        phase: marketclock::phase(et).as_str().to_string(),
        is_trading_day: marketclock::is_trading_day(marketclock::et_date(et)),
        session_date: marketclock::session_date(et).to_string(),
        market_open_et: open.format("%Y-%m-%d %H:%M ET").to_string(),
        countdown_seconds: (open - *et).num_seconds().max(0),
        et_now: et.format("%Y-%m-%d %H:%M:%S ET").to_string(),
        local_now: Local::now().format("%Y-%m-%d %H:%M:%S %Z").to_string(),
        local_tz: Local::now().format("%Z").to_string(),
    }
}

/// One full refresh. Safe to call from the API while the loop is idle.
pub async fn refresh(state: &Arc<AppState>) -> Result<Dashboard> {
    if state.refreshing.swap(true, Ordering::SeqCst) {
        // Another refresh is in flight; hand back what we have.
        return Ok((*state.snapshot()).clone());
    }
    let result = refresh_inner(state).await;
    state.refreshing.store(false, Ordering::SeqCst);
    let d = result?;
    match state.dashboard.write() {
        Ok(mut slot) => *slot = Arc::new(d.clone()),
        Err(poisoned) => *poisoned.into_inner() = Arc::new(d.clone()),
    }
    if let Err(e) = state.store.save(&d) {
        eprintln!("[warn] could not persist snapshot: {e}");
    }
    Ok(d)
}

async fn refresh_inner(state: &Arc<AppState>) -> Result<Dashboard> {
    let cfg = &state.config;
    let et = marketclock::now_et();
    let now_ts = et.timestamp();
    let mut warnings: Vec<String> = Vec::new();

    let symbols = cfg.symbols();
    let watchlist: HashSet<String> = symbols.iter().cloned().collect();
    let aliases = score::alias_map(&cfg.extra_aliases);
    let spacing = Duration::from_millis(cfg.request_spacing_ms);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(cfg.max_concurrency.max(1)));

    // ---- quotes ---------------------------------------------------------
    let mut set = JoinSet::new();
    for symbol in &symbols {
        let state = Arc::clone(state);
        let sem = Arc::clone(&sem);
        let symbol = symbol.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            tokio::time::sleep(spacing).await;
            let quote = yahoo_chart::fetch_quote(&state.client, &symbol, &state.config).await;
            (symbol, quote)
        });
    }

    // A transient network failure must not replace good prices with zeros, so a
    // failed quote inherits the last known values and is flagged as stale.
    let previous = state.snapshot();
    let mut quotes = Vec::new();
    let mut stale_quotes = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_symbol, Ok(q))) => quotes.push(q),
            Ok((symbol, Err(e))) => {
                warnings.push(format!("quote {symbol}: {e}"));
                let carried = previous
                    .watchlist
                    .iter()
                    .find(|q| q.symbol == symbol)
                    .cloned();
                match carried {
                    Some(mut stale) if stale.last > 0.0 => {
                        stale.error = Some(format!("stale: {e}"));
                        quotes.push(stale);
                        stale_quotes += 1;
                    }
                    _ => quotes.push(placeholder_quote(&symbol, state)),
                }
            }
            Err(e) => warnings.push(format!("quote task: {e}")),
        }
    }
    if stale_quotes > 0 {
        warnings.push(format!("{stale_quotes} quote(s) carried over from the last good fetch"));
    }
    quotes.sort_by(|a, b| a.group.cmp(&b.group).then(a.symbol.cmp(&b.symbol)));

    // ---- news -----------------------------------------------------------
    let mut candidates: Vec<Candidate> = Vec::new();

    // Market-wide Google News queries, narrowed as the open approaches.
    let hours_to_open = ((marketclock::next_cash_open(&et) - et).num_seconds() as f64) / 3600.0;
    let mut queries: Vec<String> = Vec::new();
    for q in &cfg.market_queries {
        if hours_to_open < 6.0 {
            queries.push(q.replace(":1d", ":6h"));
        } else {
            queries.push(q.clone());
        }
    }

    // Each task returns already-normalised candidates so the set stays homogeneous.
    let mut news_set = JoinSet::new();
    for q in queries.clone() {
        let client = state.client.clone();
        let origin = q.clone();
        news_set.spawn(async move {
            let outcome = gnews::fetch_query(&client, &q).await.map(|items| {
                items
                    .into_iter()
                    .map(|n| Candidate {
                        title: n.title,
                        source: n.publisher,
                        url: n.link,
                        published_ts: n.published_ts,
                        feed_tickers: vec![],
                        origin: origin.clone(),
                    })
                    .collect::<Vec<_>>()
            });
            (origin, outcome)
        });
    }
    for symbol in &symbols {
        let client = state.client.clone();
        let symbol = symbol.clone();
        let origin = format!("yahoo:{symbol}");
        news_set.spawn(async move {
            let outcome = yahoo_news::fetch_ticker_news(&client, &symbol, 8).await.map(|items| {
                items
                    .into_iter()
                    .map(|n| Candidate {
                        title: n.title,
                        source: n.publisher.unwrap_or_else(|| "Yahoo Finance".into()),
                        url: n.link,
                        published_ts: n.published_ts.unwrap_or_else(yahoo_news::fallback_ts),
                        feed_tickers: n.related_tickers,
                        origin: origin.clone(),
                    })
                    .collect::<Vec<_>>()
            });
            (origin, outcome)
        });
    }

    let mut market_items = 0usize;
    while let Some(joined) = news_set.join_next().await {
        match joined {
            Ok((origin, Ok(items))) => {
                if !origin.starts_with("yahoo:") {
                    market_items += items.len();
                }
                candidates.extend(items);
            }
            Ok((origin, Err(e))) => warnings.push(format!("news {origin}: {e}")),
            Err(e) => warnings.push(format!("news task: {e}")),
        }
    }

    if market_items == 0 {
        warnings.push("market-wide news feed returned nothing".into());
    }
    if std::env::var("OPENDASH_VERBOSE").is_ok() {
        let mut per_origin: std::collections::BTreeMap<&str, usize> = Default::default();
        for c in &candidates {
            *per_origin.entry(c.origin.as_str()).or_default() += 1;
        }
        for (origin, n) in per_origin {
            eprintln!("[news] {n:>3} candidate(s) from {origin}");
        }
    }

    let (mut news, mut all_news_count) = score::rank(
        candidates,
        &aliases,
        &watchlist,
        now_ts,
        120.0,
        cfg.top_news,
        cfg.max_age_hours,
    );

    if cfg.llm.enabled {
        news = crate::llm::rerank(&state.client, &cfg.llm, &news).await;
        news.truncate(cfg.top_news);
    }

    // Same principle for headlines: an empty fetch means the feeds are down, not
    // that the market had no news, so keep the previous list rather than blanking it.
    if news.is_empty() && !previous.news.is_empty() {
        warnings.push(format!(
            "keeping {} headline(s) from the last good fetch",
            previous.news.len()
        ));
        news = previous.news.clone();
        all_news_count = previous.all_news_count;
    }

    // ---- fold news into quotes ----------------------------------------
    let mut per_ticker: HashMap<String, (usize, f64)> = HashMap::new();
    for n in &news {
        for t in &n.tickers {
            let entry = per_ticker.entry(t.clone()).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += n.sentiment;
        }
    }
    for q in &mut quotes {
        if let Some((count, sum)) = per_ticker.get(&q.symbol) {
            q.news_count = *count;
            q.sentiment_avg = Some(sum / *count as f64);
        }
    }

    // ---- what else is moving -------------------------------------------
    let trending: Vec<String> = match trending::fetch_trending(&state.client, 10).await {
        Ok(list) => list.into_iter().map(|(sym, _)| sym).collect(),
        Err(e) => {
            warnings.push(format!("trending: {e}"));
            Vec::new()
        }
    };

    // ---- movers ---------------------------------------------------------
    let mut movers: Vec<Mover> = quotes
        .iter()
        .filter(|q| q.gap_pct.is_some() && q.error.is_none())
        .map(|q| Mover { symbol: q.symbol.clone(), gap_pct: q.gap_pct.unwrap(), last: q.last })
        .collect();
    movers.sort_by(|a, b| b.gap_pct.partial_cmp(&a.gap_pct).unwrap_or(std::cmp::Ordering::Equal));
    let gainers: Vec<Mover> = movers.iter().filter(|m| m.gap_pct > 0.0).take(5).cloned().collect();
    let losers: Vec<Mover> = movers
        .iter()
        .rev()
        .filter(|m| m.gap_pct < 0.0)
        .take(5)
        .cloned()
        .collect();

    let local = Local::now();

    Ok(Dashboard {
        generated_ts: now_ts,
        generated_et: et.format("%Y-%m-%d %H:%M:%S ET").to_string(),
        generated_local: local.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
        phase: phase_info(&et),
        watchlist: quotes,
        news,
        all_news_count,
        gainers,
        losers,
        trending,
        warnings,
        news_queries: queries,
    })
}

fn placeholder_quote(symbol: &str, state: &Arc<AppState>) -> crate::model::Quote {
    crate::model::Quote {
        symbol: symbol.to_string(),
        name: symbol.to_string(),
        group: state.config.group_of(symbol),
        currency: "USD".into(),
        prior_close: 0.0,
        last: 0.0,
        change_pct: 0.0,
        gap_pct: None,
        session_pct: None,
        premarket_last: None,
        premarket_vwap: None,
        premarket_high: None,
        premarket_low: None,
        premarket: false,
        session_date: String::new(),
        spark: vec![],
        volume: 0.0,
        regular_volume: 0.0,
        premarket_prints: 0,
        news_count: 0,
        sentiment_avg: None,
        error: Some("quote unavailable".into()),
    }
}

fn truncate_mid(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n / 2).collect();
    let tail: String = s.chars().skip(s.chars().count() - n / 2).collect();
    format!("{head}…{tail}")
}

/// Summary text used by `alert-once` and the launchd notification.
pub fn briefing(d: &Dashboard) -> String {
    let mut lines = Vec::new();
    for (i, n) in d.news.iter().take(3).enumerate() {
        lines.push(format!("{}. {}", i + 1, n.title));
    }
    if lines.is_empty() {
        lines.push("No headlines captured yet.".into());
    }
    let mut mover_bits: Vec<String> = Vec::new();
    for m in d.gainers.iter().take(2) {
        mover_bits.push(format!("{} +{:.2}%", m.symbol, m.gap_pct));
    }
    for m in d.losers.iter().take(2) {
        mover_bits.push(format!("{} {:.2}%", m.symbol, m.gap_pct));
    }
    if !mover_bits.is_empty() {
        lines.push(String::new());
        lines.push(format!("Gaps: {}", mover_bits.join("  ")));
    }
    lines.join("\n")
}

/// ISO-ish timestamp helper reused by the API for cache headers.
pub fn now_iso() -> String {
    DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}
