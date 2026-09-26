//! Snapshot history in SQLite.
//!
//! Two layers: a `snapshots` table holding the exact JSON served to the UI (so the
//! dashboard can be restored after a restart), and normalised `news` / `prices`
//! tables that a future event-study can query without re-fetching anything.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

use crate::model::Dashboard;

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite at {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS snapshots (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts          INTEGER NOT NULL,
                 session_date TEXT NOT NULL,
                 payload     TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS news (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts          INTEGER NOT NULL,
                 session_date TEXT NOT NULL,
                 item_id     TEXT NOT NULL,
                 title       TEXT NOT NULL,
                 source      TEXT,
                 url         TEXT,
                 published_ts INTEGER,
                 tickers     TEXT,
                 sentiment   REAL,
                 salience    REAL,
                 cluster_size INTEGER
             );
             CREATE TABLE IF NOT EXISTS prices (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts          INTEGER NOT NULL,
                 session_date TEXT NOT NULL,
                 symbol      TEXT NOT NULL,
                 last        REAL,
                 prior_close REAL,
                 change_pct  REAL,
                 gap_pct     REAL
             );
             CREATE INDEX IF NOT EXISTS news_ts ON news(ts);
             CREATE INDEX IF NOT EXISTS prices_ts_ts_sym ON prices(ts, symbol);
             CREATE INDEX IF NOT EXISTS prices_sym ON prices(symbol);",
        )
        .context("creating schema")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Persist a full dashboard plus its normalised rows.
    pub fn save(&self, d: &Dashboard) -> Result<()> {
        let payload = serde_json::to_string(d)?;
        let date = d.phase.session_date.clone();
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO snapshots (ts, session_date, payload) VALUES (?1, ?2, ?3)",
            rusqlite::params![d.generated_ts, date, payload],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO news (ts, session_date, item_id, title, source, url, published_ts, tickers, sentiment, salience, cluster_size)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            for n in &d.news {
                stmt.execute(rusqlite::params![
                    d.generated_ts,
                    date,
                    n.id,
                    n.title,
                    n.source,
                    n.url,
                    n.published_ts,
                    n.tickers.join(","),
                    n.sentiment,
                    n.salience,
                    n.breakdown.cluster_size as i64,
                ])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO prices (ts, session_date, symbol, last, prior_close, change_pct, gap_pct)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )?;
            for q in &d.watchlist {
                stmt.execute(rusqlite::params![
                    d.generated_ts,
                    date,
                    q.symbol,
                    q.last,
                    q.prior_close,
                    q.change_pct,
                    q.gap_pct,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Most recent stored dashboard, so the page is never blank after a restart.
    pub fn load_last(&self) -> Option<Dashboard> {
        let conn = self.conn.lock().ok()?;
        let payload: String = conn
            .query_row("SELECT payload FROM snapshots ORDER BY ts DESC LIMIT 1", [], |r| r.get(0))
            .ok()?;
        serde_json::from_str::<Dashboard>(&payload).ok()
    }

    /// Drop history older than `keep_days`.
    pub fn prune(&self, keep_days: i64) -> Result<usize> {
        let cutoff = chrono::Utc::now().timestamp() - keep_days * 86_400;
        let conn = self.conn.lock().unwrap();
        let mut total = 0;
        for table in ["snapshots", "news", "prices"] {
            total += conn.execute(&format!("DELETE FROM {table} WHERE ts < ?1"), [cutoff])?;
        }
        Ok(total)
    }

    pub fn counts(&self) -> (i64, i64, i64) {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return (0, 0, 0),
        };
        let get = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
        (
            get("SELECT COUNT(*) FROM snapshots"),
            get("SELECT COUNT(*) FROM news"),
            get("SELECT COUNT(*) FROM prices"),
        )
    }

    /// Historical gap for one symbol, newest first. Reserved for event-study work.
    pub fn symbol_history(&self, symbol: &str, limit: i64) -> Result<Vec<Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ts, session_date, last, prior_close, change_pct, gap_pct
             FROM prices WHERE symbol = ?1 ORDER BY ts DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![symbol.to_ascii_uppercase(), limit], |r| {
            Ok(serde_json::json!({
                "ts": r.get::<_, i64>(0)?,
                "sessionDate": r.get::<_, String>(1)?,
                "last": r.get::<_, Option<f64>>(2)?,
                "priorClose": r.get::<_, Option<f64>>(3)?,
                "changePct": r.get::<_, Option<f64>>(4)?,
                "gapPct": r.get::<_, Option<f64>>(5)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
