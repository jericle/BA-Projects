//! opendash — a pre-open US market news and price dashboard served locally.
//!
//! Subcommands:
//!   serve        start the daemon and the web server (default)
//!   snapshot     run one refresh, print a summary, exit
//!   alert-once   run one refresh and fire the macOS pre-open notification
//!   prices       print the pre-market table only

mod config;
mod http;
mod llm;
mod marketclock;
mod model;
mod options;
mod pipeline;
mod score;
mod sources;
mod store;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Timelike;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::pipeline::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut config_arg: Option<String> = None;
    let mut cmd = "serve".to_string();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                config_arg = raw.get(i).cloned();
            }
            other if other.starts_with("--config=") => {
                config_arg = Some(other["--config=".len()..].to_string());
            }
            other if !other.starts_with('-') && cmd == "serve" => cmd = other.to_string(),
            _ => {}
        }
        i += 1;
    }

    let (cfg, cfg_path) = Config::load(config_arg.as_deref())?;

    match cmd.as_str() {
        "serve" => serve(cfg, cfg_path).await,
        "snapshot" | "prices" => {
            let state = AppState::new(cfg)?;
            let d = pipeline::refresh(&state).await?;
            print_summary(&d, cmd == "prices");
            Ok(())
        }
        "alert-once" => {
            let state = AppState::new(cfg)?;
            let d = pipeline::refresh(&state).await?;
            let body = pipeline::briefing(&d);
            notify(&body);
            println!("{body}");
            Ok(())
        }
        "next-alert" => {
            // Consumed by launchd/preopen_alert.sh. The ET->local offset changes with
            // both sides' DST, so this must be computed, never hardcoded.
            let now = marketclock::now_et();
            let next = marketclock::next_alert_instant(&now);
            let local = next.with_timezone(&chrono::Local);
            println!("next_alert_epoch={}", next.timestamp());
            println!("next_alert_et={}", next.format("%Y-%m-%d %H:%M:%S %Z"));
            println!("next_alert_local={}", local.format("%Y-%m-%d %H:%M:%S %Z"));
            eprintln!(
                "next pre-open briefing: {} ET = {} (now {} ET / {} local)",
                next.format("%Y-%m-%d %H:%M %Z"),
                local.format("%Y-%m-%d %H:%M %Z"),
                now.format("%H:%M %Z"),
                chrono::Local::now().format("%H:%M %Z"),
            );
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            Ok(())
        }
    }
}

/// Addresses to listen on.
///
/// `localhost` binds **both** loopback families. This matters in practice: Safari
/// resolves `localhost` to `::1` first on macOS, so an IPv4-only bind looks dead in
/// Safari while `curl 127.0.0.1` still works. Binding only loopback keeps the
/// dashboard off the network.
fn bind_addrs(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
    let host = host.trim();
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return Ok(vec![
            SocketAddr::from(([127, 0, 0, 1], port)),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port)),
        ]);
    }
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("bad bind address {host}:{port}"))?;
    Ok(vec![addr])
}

async fn serve(cfg: Config, cfg_path: PathBuf) -> Result<()> {
    let addrs = bind_addrs(&cfg.host, cfg.port)?;

    let state = AppState::new(cfg)?;
    println!("opendash  config: {}", cfg_path.display());
    println!("opendash  market phase: {}", marketclock::phase(&marketclock::now_et()).as_str());

    tokio::spawn(refresh_loop(Arc::clone(&state)));

    let app = web::router(state);
    let app = Arc::new(app);
    let mut listeners = Vec::new();
    for addr in &addrs {
        match TcpListener::bind(addr).await {
            Ok(l) => {
                println!("opendash  listening on http://{addr}");
                listeners.push(l);
            }
            // One family may be unavailable; carry on as long as one bound.
            Err(e) => eprintln!("opendash  could not bind {addr}: {e}"),
        }
    }
    if listeners.is_empty() {
        anyhow::bail!("could not bind any of {addrs:?} — is another instance running?");
    }
    if addrs.len() > 1 && listeners.len() < addrs.len() {
        println!("opendash  open http://localhost:{}", addrs[0].port());
    } else {
        println!("opendash  open http://localhost:{}", addrs[0].port());
    }

    // One Ctrl-C fans out to every listener via a watch channel; a bare future
    // cannot be shared across tasks.
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\nopendash shutting down");
        let _ = tx.send(true);
    });

    let mut set = tokio::task::JoinSet::new();
    for listener in listeners {
        let mut rx = rx.clone();
        let app = Arc::clone(&app);
        set.spawn(async move {
            axum::serve(listener, (*app).clone())
                .with_graceful_shutdown(async move {
                    let _ = rx.changed().await;
                })
                .await
        });
    }
    while let Some(joined) = set.join_next().await {
        if let Err(e) = joined {
            eprintln!("opendash  server task ended: {e:?}");
        }
    }

    Ok(())
}

/// Background refresh. Idles outside the live session, tightens near the open.
async fn refresh_loop(state: Arc<AppState>) {
    let mut last_prune = 0i64;
    loop {
        let et = marketclock::now_et();
        let phase = marketclock::phase(&et);
        let now_ts = et.timestamp();

        let manual = state.refresh_flag.swap(false, Ordering::SeqCst);
        let never_ran = state.snapshot().generated_ts == 0;
        // Refresh when the session is live, or on first run, or when poked.
        let should = manual || never_ran || (marketclock::is_trading_day(marketclock::et_date(&et))
            && phase.is_live());

        if should {
            match pipeline::refresh(&state).await {
                Ok(d) => {
                    let n = d.news.len();
                    let w = d.watchlist.len();
                    eprintln!(
                        "[{}] refreshed: {w} quotes, {n} headlines, {} warning(s)",
                        d.generated_et,
                        d.warnings.len()
                    );
                }
                Err(e) => eprintln!("[refresh error] {e}"),
            }

            if now_ts - last_prune > 86_400 {
                last_prune = now_ts;
                if let Ok(n) = state.store.prune(state.config.keep_days) {
                    if n > 0 {
                        eprintln!("[prune] removed {n} rows older than {} days", state.config.keep_days);
                    }
                }
            }
        }

        let sleep_for = if state.in_hot_window(&et) {
            state.config.refresh_seconds_hot
        } else if should {
            state.config.refresh_seconds
        } else {
            // Idle: wake up every 5 minutes to notice the session starting.
            300
        };
        tokio::time::sleep(Duration::from_secs(sleep_for.max(5))).await;
    }
}

impl AppState {
    /// True when we are inside the tighter refresh window, e.g. 08:00-09:35 ET.
    fn in_hot_window(&self, et: &marketclock::Et) -> bool {
        let t = marketclock::et_time(et);
        let hm = (t.hour(), t.minute());
        hm >= parse_hm(&self.config.hot_window_start) && hm <= parse_hm(&self.config.hot_window_end)
    }
}

fn parse_hm(s: &str) -> (u32, u32) {
    let mut it = s.split(':');
    let h = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let m = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (h, m)
}

fn print_summary(d: &model::Dashboard, prices_only: bool) {
    println!();
    println!("  generated   {}  ({})", d.generated_et, d.generated_local);
    println!("  phase       {}  (session {})", d.phase.phase, d.phase.session_date);
    println!("  next open   {}  in {} min", d.phase.market_open_et, d.phase.countdown_seconds / 60);
    println!();

    println!("  {:<6} {:<22} {:>10} {:>9} {:>8}  SENT", "SYM", "GROUP", "LAST", "CHG%", "GAP%");
    println!("  {}", "-".repeat(76));
    for q in &d.watchlist {
        let last = if q.last > 0.0 { format!("{:.2}", q.last) } else { "—".into() };
        let chg = if q.last > 0.0 { format!("{:+.2}%", q.change_pct) } else { "—".into() };
        let gap = match q.gap_pct {
            Some(g) => format!("{g:+.2}%"),
            None => "—".into(),
        };
        let sent = match q.sentiment_avg {
            Some(s) => format!("{s:+.2} ({} news)", q.news_count),
            None => format!("{} news", q.news_count),
        };
        let flag = if q.error.is_some() { "!" } else { " " };
        println!(
            "  {flag}{:<5} {:<22} {:>10} {:>9} {:>8}  {}",
            q.symbol, truncate(&q.group, 22), last, chg, gap, sent
        );
    }
    println!();

    if prices_only {
        return;
    }

    println!("  TOP {} HEADLINES", d.news.len());
    println!("  {}", "-".repeat(76));
    for (i, n) in d.news.iter().enumerate() {
        let tickers = if n.tickers.is_empty() {
            String::new()
        } else {
            format!(" [{}]", n.tickers.join(" "))
        };
        println!(
            "  {:>2}. [{:>5.1}] {:<26} {:>5}m  {}{}",
            i + 1,
            n.salience,
            truncate(&n.source, 26),
            n.age_minutes,
            truncate(&n.title, 62),
            tickers
        );
    }
    println!();

    if !d.gainers.is_empty() || !d.losers.is_empty() {
        let fmt = |m: &model::Mover| format!("{} {:+.2}%", m.symbol, m.gap_pct);
        println!("  gainers  {}", d.gainers.iter().map(fmt).collect::<Vec<_>>().join("  "));
        println!("  losers   {}", d.losers.iter().map(fmt).collect::<Vec<_>>().join("  "));
        println!();
    }

    if !d.warnings.is_empty() {
        println!("  warnings:");
        for w in &d.warnings {
            println!("    - {w}");
        }
        println!();
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// macOS notification. Failure is non-fatal — the text is always printed.
fn notify(body: &str) {
    let script = format!(
        "display notification {} with title \"US Pre-Open 8:25am ET\" sound name \"Glass\"",
        applescript_quote(body)
    );
    match std::process::Command::new("osascript").arg("-e").arg(&script).status() {
        Ok(s) if s.success() => eprintln!("[alert] notification sent"),
        Ok(s) => eprintln!("[alert] osascript exited {s}"),
        Err(e) => eprintln!("[alert] could not run osascript: {e}"),
    }
}

fn applescript_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars().take(400) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_usage() {
    eprintln!("usage: opendash [serve|snapshot|prices|alert-once|next-alert] [--config PATH]");
    eprintln!();
    eprintln!("  serve        run the web dashboard (default)");
    eprintln!("  snapshot     refresh once, print a text summary, exit");
    eprintln!("  prices       print only the pre-market price table");
    eprintln!("  alert-once   refresh once and fire the macOS notification");
    eprintln!("  next-alert   print the next 08:25 ET briefing time in ET and local time");
}
