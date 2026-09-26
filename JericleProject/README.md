# opendash — pre-open US market news and price dashboard

A local Rust daemon that serves a browser dashboard showing the **top 20 US market
headlines** and the **pre-market price action** for a watchlist of AI / memory / power
tickers, built to be looked at about an hour before the 09:30 ET cash open.

No API keys. No npm. No build step for the frontend.

```
┌────────────────────────────────────────────────────────────────┐
│  high vol buy │ high vol sell │ top gainer │ top loser             │  KPI row 1
│  call wall   │ put wall     │ expiry · put/call OI             │  KPI row 2
├──────────────────────────┬─────────────────────────────────────┤
│  TOP 20 HEADLINES        │  PRE-MARKET                        │
│  ranked, sentiment,      │  gap %, sparkline,       │
│  tagged tickers,         │  grouped by theme,       │
│  score breakdown         │  click for detail        │
├──────────────────────────┴──────────────────────────┤
│  drawer: 04:00→now ET price chart with headline     │
│           markers plotted at their publish time     │
└─────────────────────────────────────────────────────┘
```

**Row 1** are session extremes across the watchlist. Every card is clickable and opens
that symbol's drawer.

* **High Volume Buy** — the *heaviest-traded name among those that are up*. Directional,
  not "highest volume": a high-volume decliner does not belong here.
* **Highest Volume Sell** — the mirror image, heaviest traded among the names that are down.
* **Top gainer / top loser** — the extremes of the move itself.

Direction comes from the pre-market gap before 09:30 ET and the last change afterwards.
If nothing on the watchlist moved that way, the card says `none up today` rather than
crowning a name from the wrong side.

On volume: **Yahoo publishes no pre-market volume** — every 1-minute bar before 09:30 ET
reports `0`, at every interval. So pre-open the cards show the last completed session's
volume (`meta.regularMarketVolume`, labelled `last sess`) with the pre-market print count
as the "today" activity signal, and switch to real session volume labelled `session` once
the cash market is running. They never show a bare `0`.

**Row 2** are the option walls for the selected ticker (the High Volume Buy name until you
pick one). The two wall cards each span two of the four columns, so they sit directly
beneath the four session cards. Call wall and put wall are the strikes with the highest open interest, matching
`Finance/price_hist.py`. The expiry shown is the current ET month; if the month has no
listed expiry left the card says `nearest (none left this month)` rather than quietly
showing the wrong month.

## Quick start

```sh
cargo build --release
./target/release/opendash serve      # then open http://127.0.0.1:8787
```

Other subcommands:

| Command | What it does |
|---|---|
| `opendash serve` | run the daemon and web server (default) |
| `opendash snapshot` | one refresh, print a text summary of prices + headlines, exit |
| `opendash prices` | one refresh, print only the price table |
| `opendash alert-once` | one refresh, fire the macOS notification, exit |
| `opendash next-alert` | print the next 08:25 ET briefing in ET **and** local time |

## What "pre-market" means here

* Session window **04:00 → 09:30 ET**; cash open 09:30 ET, close 16:00 ET, extended hours to 20:00 ET.
* **gap %** — the headline number at 08:30 ET — is the pre-market last price versus the previous close.
* Yahoo only emits 1-minute pre-market bars where a trade actually printed, so thinly traded
  names can legitimately show one or two points. The UI says "thin pre-market" rather than
  drawing a misleading chart.
* All market reasoning happens in `America/New_York` with a real NYSE holiday table
  (fixed-date holidays shift to the nearest weekday; Good Friday is tabulated).
  The host here is AEST, so **08:30 ET is 22:30 Sydney** — and that offset is +13h or +14h
  depending on which side is in DST, which is why nothing is hardcoded in local time.

## Data sources (all keyless)

| Used for | Endpoint |
|---|---|
| Pre-market + intraday prices | `query1.finance.yahoo.com/v8/finance/chart/{sym}?interval=1m&range=1d&includePrePost=true` |
| Per-ticker news | `query1.finance.yahoo.com/v1/finance/search?q={sym}&newsCount=8` |
| Market-wide news | `news.google.com/rss/search?q=…&hl=en-US&gl=US&ceid=US:en` |
| Also-moving-today | `query1.finance.yahoo.com/v1/finance/trending/US` |
| Option chains | `query2.finance.yahoo.com/v7/finance/options/{sym}` (needs a cookie + crumb, handled for you) |
| Option fallback | `cdn.cboe.com/api/global/delayed_quotes/options/{sym}.json` (OPRA) |

These Yahoo endpoints are unofficial. If one breaks or throttles, the affected rows show an
error, the warnings line appears in the footer, and the last stored snapshot keeps serving.

## How the ranking works

Salience is 0–100 and every component is shown on hover in the UI:

| Component | Max | Rule |
|---|---|---|
| Recency | 30 | exponential decay, 2 h half-life |
| Cluster | 25 | distinct outlets carrying the same story, log-scaled |
| Impact keywords | 25 | earnings, guidance, FDA, merger, downgrade, SEC, tariff, Fed, … |
| Source tier | 10 | Reuters/Bloomberg/CNBC/WSJ… above aggregators |
| Watchlist relevance | 10 | tagged ticker is on your watchlist |

Then:

* near-duplicate headlines are **clustered**, and only the best-scoring member of each cluster
  is shown — you get the top 20 *stories*, not 20 syndicated copies of 5 stories
* boilerplate ("Stock Price, News, Quote", "Is It Too Late to Buy") is dropped
* stories that are neither US-market nor watchlist-related are damped to 60%, not deleted
* sentiment is a finance-tuned lexicon with negation handling, in [-1, 1]

## Option walls

Fetched on demand and cached for `options_cache_seconds` (default 1800) because open
interest is end-of-day data and the payloads are large. Yahoo answers first; if its crumb
flow fails, Cboe's consolidated OPRA feed answers instead and the card names which feed
was used. Force the fallback with:

```sh
OPENDASH_OPTIONS_SOURCE=cboe ./target/release/opendash serve
```

Both feeds were cross-checked on NVDA and returned identical walls: call 230.00
(OI 90,075), put 105.00 (OI 47,486), 587,399 total call OI.

Note that Yahoo encodes `expirationDates` as midnight **UTC** while the chain's
`expirationDate` is 09:30 ET. Both land on the right calendar day in UTC and on the wrong
one in ET, so the whole options module reasons in UTC — see `src/options.rs`.

## Configuration

`config.toml` — every key is also the default, so delete anything you do not want to change.
Watchlist groups:

```toml
[[groups]]
name = "AI Core"
symbols = ["NVDA", "AMD", "AVGO", "TSM", "ARM", "MRVL", "INTC", "ORCL", "MSFT", "GOOGL"]

[[groups]]
name = "Memory & Storage"
symbols = ["MU", "WDC", "STX", "SNDK"]

[[groups]]
name = "Power & Grid"
symbols = ["CEG", "VST", "NRG", "TLN", "GEV", "ETN", "PWR", "EME"]
```

Refresh cadence tightens automatically inside `hot_window_start`…`hot_window_end` (08:00–09:35 ET).

### Optional local LLM rerank

Off by default. If your local mlx server is up, the top 25 headlines are re-scored and the
model's verdict is **blended 50/50** with the heuristic score — one bad response cannot
dominate, and if the endpoint is down or slow (5 s timeout) the heuristic ranking is used
unchanged.

```toml
[llm]
enabled = true
url = "http://10.0.0.2:52415"
model = "mlx-community/Qwen3.6-35B-A3B-4bit"
```

## Automation

```sh
./launchd/install.sh            # install + start + health check
./launchd/install.sh --sync     # re-copy config/binary after editing, then restart
./launchd/install.sh --uninstall
```

**Why the installer exists.** This project lives on an external SSD, and launchd's spawn
context blocks indefinitely on `open()` for anything under `/Volumes/*`. A LaunchAgent
pointing straight at the SSD therefore hangs at startup with no output at all — the job
reports `state = running` while the process sits in `open()`. So the installer puts the
supervised binary (`~/.local/bin/opendash`) and its config (`~/.opendash/config.toml`) on
the internal volume, where launchd can reach them, and leaves the repo as the source of
truth. After editing the repo's `config.toml`, run `--sync`.

The runtime directory is configurable and independent of where the source lives:

```sh
opendash serve --config ~/somewhere/config.toml
# or
OPENDASH_CONFIG=~/somewhere/config.toml opendash serve
```

Resolution order: `--config`, `$OPENDASH_CONFIG`, `$OPENDASH_DIR/config.toml`,
`./config.toml`. A relative `db_path` resolves against the config file's own directory, so
the database follows the config rather than the working directory.
```

Pre-open briefing (macOS notification with the top 3 headlines and the biggest gap movers):

```sh
./launchd/preopen_alert.sh loop     # foreground; re-arms each day
./launchd/preopen_alert.sh once     # fire only if 08:25 ET has arrived
./launchd/preopen_alert.sh now      # fire immediately
```

`loop` asks the binary for the next 08:25 ET instant and sleeps until then, rather than using
`StartCalendarInterval`, because the ET→local offset changes with both countries' DST.

## API

| Route | Purpose |
|---|---|
| `GET /` | the dashboard page (`no-store`, so a rebuild is never masked by cache) |
| `GET /api/dashboard` | the full payload the page renders (camelCase) |
| `GET /api/refresh` | force a refresh, returns a short ack |
| `GET /api/options/{symbol}` | option walls for the current-month expiry (cached) |
| `GET /api/history/{symbol}` | stored price history for one symbol |
| `GET /health` | liveness plus row counts |

## History

Every refresh is written to `data/dashboard.db` (SQLite, gitignored):

* `snapshots` — the exact JSON that was served, so the page is never blank after a restart
* `news` — one row per headline with sentiment, salience, tickers, cluster size
* `prices` — one row per symbol per cycle, ready for a future event study

```sql
-- how NVDA's pre-market gap moved on days with NVDA-tagged headlines
SELECT date(ts/86400,'unixepoch') d, round(avg(gap_pct),2) avg_gap, count(*)
FROM prices WHERE symbol='NVDA' AND gap_pct IS NOT NULL
GROUP BY d ORDER BY d DESC LIMIT 20;
```

Rows older than `keep_days` are pruned daily.

## Development

```sh
cargo test                 # 29 unit tests: calendar, DST, scoring, RSS, option walls, OCC
cargo run -- snapshot      # live end-to-end run, prints a text summary
OPENDASH_VERBOSE=1 cargo run -- snapshot   # per-feed candidate counts

# headless smoke test of the page's real JS against the live API payload
./target/release/opendash serve &
curl -s localhost:8787/api/dashboard -o /tmp/dash.json
node scripts/smoke-ui.mjs
```

Keyboard: `1`–`2` switch panels, `r` refresh, `Esc` close the drawer. The active
panel is kept in the URL fragment, so `localhost:8787/#premarket` opens straight
into the pre-market tab.
