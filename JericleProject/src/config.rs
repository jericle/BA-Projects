//! Configuration loading. Every field has a default, so a partial `config.toml` is valid.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub port: u16,
    pub host: String,
    pub db_path: String,
    /// Refresh cadence outside the hot window.
    pub refresh_seconds: u64,
    /// Refresh cadence between `hot_window_start` and `hot_window_end` ET.
    pub refresh_seconds_hot: u64,
    pub hot_window_start: String,
    pub hot_window_end: String,
    pub top_news: usize,
    /// Drop news items older than this many hours.
    pub max_age_hours: i64,
    /// How long to keep raw history rows in SQLite.
    pub keep_days: i64,
    /// Simultaneous outbound requests.
    pub max_concurrency: usize,
    /// Delay injected before each request to stay polite.
    pub request_spacing_ms: u64,
    pub groups: Vec<Group>,
    /// Google News queries for market-wide coverage. `:6h`/`:1d` suffixes are honoured.
    pub market_queries: Vec<String>,
    /// Directory the config was loaded from; relative `db_path` resolves against it.
    #[serde(skip)]
    pub base_dir: PathBuf,
    /// How long a fetched option chain stays valid. Open interest is end-of-day
    /// data and the upstream payloads are large, so this is generous.
    pub options_cache_seconds: i64,
    /// Extra company names used for ticker tagging beyond the built-in table.
    #[serde(default)]
    pub extra_aliases: HashMap<String, Vec<String>>,
    pub llm: LlmConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Group {
    pub name: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmConfig {
    pub enabled: bool,
    pub url: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8787,
            host: "localhost".to_string(),
            db_path: "data/dashboard.db".to_string(),
            base_dir: PathBuf::from("."),
            refresh_seconds: 120,
            refresh_seconds_hot: 30,
            hot_window_start: "08:00".to_string(),
            hot_window_end: "09:35".to_string(),
            top_news: 20,
            max_age_hours: 30,
            keep_days: 45,
            max_concurrency: 4,
            request_spacing_ms: 150,
            options_cache_seconds: 1800,
            groups: default_groups(),
            market_queries: default_market_queries(),
            extra_aliases: HashMap::new(),
            llm: LlmConfig::default(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://10.0.0.2:52415".to_string(),
            model: "mlx-community/Qwen3.6-35B-A3B-4bit".to_string(),
            timeout_ms: 5000,
        }
    }
}

pub fn default_groups() -> Vec<Group> {
    vec![
        Group {
            name: "AI Core".into(),
            symbols: ["NVDA", "AMD", "AVGO", "TSM", "ARM", "MRVL", "INTC", "ORCL", "MSFT", "GOOGL"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
        Group {
            name: "Memory & Storage".into(),
            symbols: ["MU", "WDC", "STX", "SNDK"].iter().map(|s| s.to_string()).collect(),
        },
        Group {
            name: "Power & Grid".into(),
            symbols: ["CEG", "VST", "NRG", "TLN", "GEV", "ETN", "PWR", "EME"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        },
    ]
}

pub fn default_market_queries() -> Vec<String> {
    vec![
        "stock market when:1d".into(),
        "\"S&P 500\" when:1d".into(),
        "premarket movers stocks when:1d".into(),
        "stock market today when:1d".into(),
    ]
}

impl Config {
    /// Load a config file, falling back to defaults for anything absent.
    ///
    /// Resolution order: explicit `--config`, then `$OPENDASH_CONFIG`, then
    /// `$OPENDASH_DIR/config.toml`, then `./config.toml`.
    ///
    /// The launchd job deliberately keeps this file off external volumes: launchd's
    /// spawn context blocks indefinitely on `open()` under `/Volumes/*`, so a config
    /// on an external SSD hangs the daemon at startup with no output. Hence
    /// `OPENDASH_CONFIG` pointing somewhere launchd can reach.
    pub fn load(explicit: Option<&str>) -> Result<(Self, PathBuf)> {
        let candidates: Vec<PathBuf> = [
            explicit.map(PathBuf::from),
            std::env::var("OPENDASH_CONFIG").ok().map(PathBuf::from),
            std::env::var("OPENDASH_DIR").ok().map(|d| PathBuf::from(d).join("config.toml")),
            Some(PathBuf::from("config.toml")),
        ]
        .into_iter()
        .flatten()
        .collect();

        let path = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone());

        if !path.exists() {
            eprintln!(
                "opendash  no config.toml found (looked in {}); using defaults",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return Ok((Config::default(), path));
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        // A relative db_path is resolved against the config's own directory, so the
        // database follows the config rather than the working directory.
        cfg.base_dir = path
            .parent()
            .map(|p| if p.as_os_str().is_empty() { PathBuf::from(".") } else { p.to_path_buf() })
            .unwrap_or_else(|| PathBuf::from("."));
        Ok((cfg, path))
    }

    /// Absolute path to the database, honouring `base_dir` for relative settings.
    pub fn db_path(&self) -> PathBuf {
        let p = Path::new(&self.db_path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base_dir.join(p)
        }
    }

    /// Every symbol on the watchlist, in group order, de-duplicated, upper-cased.
    pub fn symbols(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for g in &self.groups {
            for s in &g.symbols {
                let s = s.trim().to_ascii_uppercase();
                if !s.is_empty() && seen.insert(s.clone()) {
                    out.push(s);
                }
            }
        }
        out
    }

    pub fn group_of(&self, symbol: &str) -> String {
        for g in &self.groups {
            if g.symbols.iter().any(|s| s.eq_ignore_ascii_case(symbol)) {
                return g.name.clone();
            }
        }
        "Other".to_string()
    }
}
