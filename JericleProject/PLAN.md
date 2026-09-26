# Pre-Open News & Price Dashboard — Plan

**Status:** built. See `README.md` for how to run it.
**Created:** 2026-09-26
**Host timezone:** AEST (Sydney). All market logic is computed in `America/New_York`.

## Build notes — what changed from the plan

* **Cluster collapse.** The plan ranked articles. In practice the same story arrives from
  several outlets, so near-duplicates are clustered and only the best-scoring member is
  shown. The top 20 is now 20 *stories*, with the other outlets listed on hover.
* **Publisher names come from the `<source>` element text**, not the `url` attribute.
  Deriving from the URL produced "Finance", "Uk" and "Investors".
* **Relevance damping and a boilerplate blocklist** were added after the first live run
  surfaced a Czech macro story and a "Stock Price, News, Quote" page in the top 10.
  Off-topic stories are damped to 60%, not deleted.
* **"S&P 500" not "S&P"** as a US-market marker, so S&P Global the ratings agency does not
  match. A unit test covers this.
* **Stale-data handling.** A failed quote now inherits the last known values and is flagged
  `stale` rather than being zeroed; an empty news fetch keeps the previous headlines.
  Verified by pointing the fetches at a black-hole proxy: 22/22 quotes kept their prices,
  all 20 headlines were retained, 52 warnings surfaced.
* **Honest thin pre-market.** Yahoo only emits 1-minute pre-market bars where a trade
  printed, so EME legitimately has one point. Those rows say "thin pre-market" instead of
  drawing a misleading sparkline.
* **`deny_unknown_fields` on the config.** Appending a key after a `[table]` header silently
  lands it in that table; a misplaced key is now a hard parse error.
* **Verification added:** 61 Rust unit tests (calendar, DST, scoring, RSS, option walls,
  OCC parsing, secrets redaction and permissions, the credit counter, and every Twelve
  Data parsing quirk above) and `scripts/smoke-ui.mjs`, which runs the page's real
  JavaScript against the live API payload under a stub DOM and asserts 95 rendering
  invariants (27 at the time of writing, 66 after the tab milestone, 95 after this one).
* **KPI cards added** (second request): row 1 is the session extremes. The first cut used
  "most traded / least traded", which is high volume vs *low* volume and not what a
  High Volume Buy/Sell read is actually for. Corrected to the directional definition:
  heaviest-traded name among those **up**, and heaviest-traded among those **down**. The
  wall cards in row 2 were also re-laid-out to span two of the four columns each so they
  align directly beneath the four session cards, instead of drifting into a half-width row. Volume is pre-market volume before 09:30 ET and
  session volume after, and is labelled as such, because before the open the two are the
  same number. Row 2 is the call wall and put wall of the current ET month expiry for the
  selected ticker, with the expiry date, distance from spot and the open interest behind
  each wall.
* **Option data needed a new source.** Yahoo's `v7/finance/options` rejects keyless
  requests with "Invalid Crumb", so a cookie + crumb session was added. That yields a
  ~50 KB per-expiry payload, against 0.8-5 MB from Cboe's OPRA feed, which is kept as a
  fallback. Both were cross-checked on NVDA and agree exactly.
* **Two upstream encoding traps, both caught by tests then confirmed live:** Yahoo encodes
  `expirationDates` as midnight UTC (reading it in ET made the live monthly expiry look
  expired and silently showed the following week), and CBOE emits `open_interest` as a
  JSON float, which serde will not coerce into `i64`.
* **OCC strike fields are in thousandths**, confirmed by cross-reference: Yahoo reports
  `NVDA260925C00050000` as strike 50.0, not 500.
* **PRICE HISTORY tab** (v0.3.0). A third tab, next to the other two, showing OHLC bars
  for the watchlist at any of Twelve Data's twelve intervals (`1min` … `1month`), with
  candles or a line and a crosshair readout of O/H/L/C/volume. The list view reuses the
  Yahoo quote already in the dashboard payload and costs **zero** credits; Twelve Data is
  called only when a symbol is opened. That split is what makes the free tier viable at
  all — see the rate-limit note below.
* **This is the project's first keyed upstream, and the key is kept out of the repo.**
  `BA-Projects` is a **public** GitHub repository, so an API key in a tracked file is a key
  anyone can clone. The key lives in `~/.opendash/secrets.toml`, mode 600, outside the repo
  entirely. Four things back that up: the file mode is checked on load and a loose one is
  reported with the `chmod` to fix it; the key travels in an `Authorization` header, never
  a query string, so it cannot surface in an access log, a `Referer`, or an error that
  echoes the URL; `Debug` is hand-written to print `***` (and not the length, which is
  small enough to brute-force the rest); and there is deliberately no `Serialize`.
  `.gitignore` now carries a secrets pattern as a second line of defence, and
  `launchd/install.sh` creates the file from a committed template at mode 600 rather than
  letting a umask decide. Verified by sweeping every endpoint for the key: clean.
* **Providers are a map, not a struct field.** `secrets.toml` is `[providers.<name>]`, so a
  second service needs no code change — which was the stated requirement, since more keys
  are coming. A misspelled provider name resolves to `None` and reports itself, rather than
  quietly matching nothing; a misspelled *field* inside a provider is a parse error, so a
  typo cannot present as "no key configured".
* **A rate limiter that caused the throttling it was meant to prevent.** The first version
  was a continuously-refilling token bucket, which is right for smoothing your own bursts
  and wrong for matching a provider's quota. Live against the free tier it let **11
  requests into an 8/min limit** — tokens trickled back mid-window as a slow sequence of
  clicks ran — and Twelve Data's own error said so. Matching a provider means reproducing
  the provider's window arithmetic, so it now counts within the wall-clock minute and
  resets on the minute. Re-tested live: 12 distinct symbols in one minute gave exactly 8
  served, 4 refused with a retry hint, 0 rejected upstream.
* **Twelve Data reports a quota breach as HTTP 200 with an error body**, so the response
  code cannot be the thing that catches it. The body is parsed first and its message
  surfaced, because "the plan allowance is 8/min, retry in 23s" is actionable and "HTTP 200"
  is not. The limiter refuses *before* spending.
* **Four upstream quirks the parser handles, each with a test.** All OHLC values arrive as
  strings carrying float noise (`"225.080002"`). Bars come back **newest first**, so the
  request asks for `order=asc` and the parser sorts anyway, because a cached or batched
  payload can still arrive reversed. `1day`/`1week`/`1month` return a **bare date**
  (`2026-09-01`) and ignore `timezone` entirely, while intraday returns a full timestamp —
  two shapes, both naive wall-clock, both localised to ET explicitly. Guessing UTC for the
  date-only shape is the classic way to put every daily bar on the wrong date. A bar in the
  spring-forward gap is rejected rather than silently shifted an hour.
* **`prepost` is Pro+ only**, so this tab cannot serve pre-market bars — the demo key
  appeared to return extended-hours data, but that is the demo key behaving specially and
  is not evidence a free key will. Yahoo's pre-market coverage in the PRE-MARKET tab is
  therefore kept, and the two tabs stay separate rather than merging.
* **A no-op timeframe switch does not refetch.** Against an 8/min budget, re-selecting the
  active interval would spend a credit to redraw an identical chart. Covered by a test.
* **One tab per panel** (this milestone). The two panels used to share a row —
  headlines in a 1.35fr column, pre-market in a 1fr one — so neither ever got
  room for what it had to show. Each is now its own full-width tab, `TOP HEADLINES`
  and `PRE_MARKET`, with the KPI band still global above them. The tab bar is
  rendered from one `TABS` array, so the buttons, the digit shortcuts and the URL
  hash cannot drift apart; the active tab is written with `replaceState` rather
  than `location.hash` because the page repolls every 30s and a real history entry
  per switch would make Back useless. Panels are hidden with `.panel{display:none}`
  and the default `on` state is in the markup, so the first paint shows one panel
  without waiting on JS.
* **Pre-market spends its extra width on columns, not new fields.** The three
  watchlist groups become grid items (`repeat(auto-fit, minmax(min(320px,100%),1fr))`)
  so all 22 symbols are on screen without scrolling, and the per-row data is
  unchanged. `min()` rather than a hard 320px floor because a fixed track pushes
  a horizontal scrollbar onto a phone. The `:first-child` rule moved from
  `.group-title` to `.group` when the groups gained a wrapper: inside a grid, the
  first group of each column is not `:first-child`, so anchoring it on the title
  would have stripped the separator from all three.
* **§3's file tree was stale** (v0.2.1). It still described the pre-option-walls
  layout, so `options.rs`, `pipeline.rs`, both option sources, `install.sh` and the
  smoke test were absent — the build notes above described code the architecture
  section did not contain. Now checked against `find`: every file on disk appears in
  the tree and every entry in the tree exists. Anything added after a milestone
  needs the tree updated in the same change, or the two halves of the plan stop
  describing the same program.

## 1. Goal

A browser dashboard, served locally by a Rust binary, that you open roughly one hour
before the US cash open and see:

- the **top 20 market-moving news items** for the US market, ranked and tagged with tickers
- the **pre-market price action** for a watchlist of AI / memory / power tickers
  (last price, gap vs. previous close, sparkline of the 04:00→09:30 ET session)
- a **per-ticker timeline** plotting news markers on the pre-market price chart

Pre-open is 08:30 ET. In Sydney that is **22:30 AEDT** (or 22:30 AEST under EST —
the ET→Sydney offset is +14h or +13h depending on which side is in DST, so nothing
may be hardcoded in local time).

## 2. Verified data sources

All of the following were tested live before writing this plan.

| Source | Endpoint | Result |
|---|---|---|
| Yahoo news (per ticker) | `query1.finance.yahoo.com/v1/finance/search?q={T}&newsCount=8&quotesCount=0` | 200, no API key. Returns `title`, `link`, `publisher`, `providerPublishTime`, `relatedTickers` |
| Yahoo quotes (pre/post) | `query1.finance.yahoo.com/v8/finance/chart/{T}?interval=1m&range=1d&includePrePost=true` | 200, no API key. 1-minute bars from 04:00 ET incl. pre-market. `meta.previousClose` present |
| Yahoo trending | `query1.finance.yahoo.com/v1/finance/trending/US` | 200, works. Used as a fallback source of "what matters today" |
| Market-wide news | `news.google.com/rss/search?q=...&hl=en-US&gl=US&ceid=US:en` | 200, 100 items, `pubDate` (RFC 2822) + `<source>` publisher |
| Twelve Data OHLC | `api.twelvedata.com/time_series?symbol=..&interval=..` | 200, but **needs a key**. Header auth works and is used so the key stays out of the URL. All 12 intervals confirmed live |

**Rejected / avoided**

- `query1.finance.yahoo.com/v7/finance/quote` → **401 Unauthorized**, unusable.
- Yahoo legacy RSS `feeds.finance.yahoo.com/rss/2.0/headline` → 404.
- Yahoo `search?q=stock market` → low-quality ETF listicles, not usable for market-wide news.
  Market-wide news comes from Google News RSS instead.
- NewsAPI.org free tier → 100 req/day and ~24h article delay, useless for a live pre-open view.
- Finnhub → good but needs a signup key; deliberately avoided to keep this keyless.

**A note on the keyless property.** The dashboard is still keyless *for news and quotes* —
Google News RSS, Yahoo chart, Yahoo news and CBOE all remain unauthenticated. Twelve Data is
the single exception, added for the PRICE HISTORY tab only, and §10 covers what it took to
keep that key out of a public repository.

**Local LLM** (`http://10.0.0.2:52415`, mlx Qwen) was not responding during planning, so it is
wired in as an **optional** rerank step with a hard timeout and a silent fallback. Nothing
depends on it.

## 3. Architecture

Rust daemon, one binary, three subcommands. Browser UI on `localhost:8787`.

```
JericleProject/
├── Cargo.toml
├── Cargo.lock                 committed: pinned deps make the binary reproducible
├── config.toml                watchlist groups, port, refresh cadence, tuning
├── PLAN.md                    this file
├── README.md                  how to run, tune, install launchd
├── BUILD-REPORT.md            milestone build notes and verification results
├── launchd/
│   ├── com.jericle.opendash.plist        KeepAlive the daemon
│   ├── install.sh                 install / --sync / --uninstall
│   └── preopen_alert.sh                  DST-safe 8:25 ET → macOS banner
├── scripts/
│   └── smoke-ui.mjs             headless run of the page's JS against a live payload
├── secrets.example.toml         credentials TEMPLATE; the real file is never in the repo
├── src/
│   ├── main.rs                serve | snapshot | alert-once
│   ├── config.rs              config.toml loading + defaults
│   ├── marketclock.rs         ET/DST/holiday logic, session phase
│   ├── http.rs                shared reqwest client, throttle, retry
│   ├── model.rs               serialisable types
│   ├── score.rs               clustering, ticker tagging, sentiment, salience
│   ├── options.rs             option wall derivation from a chain (pure, no network)
│   ├── store.rs               SQLite history (rusqlite)
│   ├── pipeline.rs            the refresh cycle: quotes + news in, Dashboard out
│   ├── llm.rs                 optional local-LLM rerank
│   ├── secrets.rs             API keys from outside the repo; redacted Debug
│   ├── sources/
│   │   ├── mod.rs             source module list
│   │   ├── yahoo_chart.rs     pre-market quotes
│   │   ├── yahoo_news.rs      per-ticker news
│   │   ├── yahoo_options.rs   option chain via Yahoo's cookie + crumb session
│   │   ├── cboe.rs            fallback option chain from Cboe's OPRA feed
│   │   ├── gnews.rs           market-wide RSS
│   │   ├── trending.rs        Yahoo trending tickers
│   │   ├── twelvedata.rs      OHLC series at any interval (PRICE HISTORY tab)
│   │   └── ratelimit.rs       per-minute credit counter matched to the plan
│   └── web/
│       ├── mod.rs             axum router, JSON API
│       └── index.html         embedded single-page UI
└── data/dashboard.db          SQLite (gitignored)
```

Dependencies: `axum 0.8`, `tokio`, `reqwest` (rustls), `serde`, `serde_json`,
`quick-xml`, `chrono` + `chrono-tz`, `rusqlite` (bundled), `anyhow`, `toml`.
No npm, no build step for the frontend.

## 4. Refresh cycle

1. **Market clock** — convert `now` to `America/New_York`; determine trading day
   (weekday, minus NYSE holidays); derive phase:
   `Closed` / `Pre-market (04:00–09:30 ET)` / `Open (09:30–16:00 ET)` / `After (16:00–20:00 ET)`.
2. **Quotes** — one chart request per watchlist symbol (concurrency 4, 150 ms spacing).
   From each response:
   - `prior_close` = `meta.previousClose`
   - bars filtered to ET time-of-day in `[04:00, 09:30)` on the session date = pre-market bars
   - `premarket_last`, `gap % = premarket_last / prior_close − 1`, VWAP, high/low
   - sparkline = pre-market closes downsampled to ≤ 60 points
   - `last` / `change %` = most recent bar of any session vs `prior_close`
3. **News** — market-wide from Google News RSS across a query set
   (`stock market when:1d`, `S&P 500`, `premarket movers`, `Fed`, plus
   `when:6h` narrowing when inside 6h of the open) and per-ticker from Yahoo for
   every watchlist symbol.
4. **Normalise** — dedupe by URL, cluster near-duplicate headlines by token
   overlap, tag tickers (`$TKR`, exact uppercase token, company aliases), score
   sentiment, score salience, keep top 20.
5. **Persist** to SQLite, publish into shared state, serve at `/api/dashboard`.

Background loop refreshes every 60s (30s between 08:00 and 09:35 ET). Outside the
04:00–20:00 ET window it sleeps and serves the last stored snapshot, so the page is
never empty. `GET /api/refresh` forces an immediate cycle.

## 5. Metrics shown

- **gap %** — headline number at 08:30 ET: pre-market last vs previous close
- **change %** — last price of any session vs previous close
- **pre-market VWAP / high / low** and session change since 04:00 ET
- **sentiment** per news item and per ticker (average over that ticker's items)
- **salience** 0–100 with the breakdown exposed in the UI, never a black box
- **cluster size** — how many distinct outlets carried the same story

## 6. Salience score (0–100)

| Component | Weight | Rule |
|---|---|---|
| Recency | 30 | `30 · exp(−ln2 · age_min / 120)` (2 h half-life) |
| Cluster | 25 | `25 · min(1, ln(1+n) / ln(1+8))` where n = distinct outlets |
| Impact keywords | 25 | `25 · min(1, hits/2)`, hits = earnings, guidance, pre-announcement, FDA, approval, merger, downgrade, upgrade, SEC, antitrust, tariff, export controls, recall, lawsuit, probe, bankruptcy, guidance cut, Fed, rate hike/cut |
| Source tier | 10 | Reuters/Bloomberg/CNBC/WSJ/FT/Barron's/IBD/MarketWatch top tier; aggregators low |
| Watchlist relevance | 10 | 10 if any tagged ticker is on the watchlist |

## 7. Sentiment

~300-term finance lexicon with weights (beat, miss, upgrade, downgrade, plunge, surge,
recall, guidance cut, …) plus negation handling (`not`, `no`, `never`, `fails to`,
`without`, `avoids`). Score = weighted sum / √(matches), clamped to [−1, 1].
Disambiguation: aliases shorter than 5 characters only match when the token is
all-uppercase in the original text or `$`-prefixed, so `ARM` does not match the word
"arm" and `MU` does not match inside other words.

## 8. UI

Single embedded HTML page, no framework, no build step. Dark split-flap theme
reusing the palette from `Finance/price_hist.py` (`#1a1e24` background, `#4fc3f7`
accent, `#66bb6a` / `#ef5350` for up/down).

- **Header** — phase badge, split-flap countdown to 09:30 ET, current time in both
  ET and local time, data freshness, manual refresh
- **KPI band** — always visible, above the tabs: the four session cards
  (highest-volume buy / sell, top gainer / loser) each directly above the option
  walls for that same symbol
- **Panel tabs** — one tab per panel, each the full page width. `TOP HEADLINES`,
  `PRE_MARKET` and `PRICE HISTORY`, switchable by click, by digit `1`–`9`, or by a
  `#tab` URL fragment so a link reopens the same panel
- **Top 20 news** — headline, publisher, age, sentiment chip, tagged tickers, salience
  bar (breakdown on hover)
- **Pre-market table** — watchlist groups side by side as columns; price, gap %,
  sparkline, news count
- **Ticker detail** — click a row: full 04:00→09:30 ET price chart with news markers
  plotted at publish time, plus that ticker's news list
- **Price history** — OHLC bars for any watchlist symbol at any of twelve intervals,
  candles or line, crosshair readout of O/H/L/C/volume. Sourced from Twelve Data,
  one credit per opened symbol; the list itself reuses Yahoo data and is free

## 9. Automation

- `com.jericle.opendash.plist` — `KeepAlive` so the daemon is always up.
- `preopen_alert.sh` — **not** a fixed `StartCalendarInterval`. The ET→Sydney offset
  is +14h under EDT and +13h under EST, and Sydney's own DST shifts independently, so
  a hardcoded 22:25 local time would be wrong twice a year. The script computes the
  next 08:25 ET instant, sleeps until it, fires an `osascript` notification containing
  the top 3 headlines plus the biggest gap movers, then re-arms. Holidays are skipped
  using the same market clock as the daemon.

## 10. API keys

Credentials are **not** in `config.toml` and **not** in the repository. `BA-Projects`
is public on GitHub, so a key in a tracked file is a key anyone can clone — and a
leaked key can be used to burn the account's quota or change its plan.

The file is `~/.opendash/secrets.toml`, mode `600`, created from the committed
`secrets.example.toml` template by `launchd/install.sh`:

```toml
[providers.twelvedata]
api_key = "..."
```

Providers are a map keyed by name, so adding a service needs no code change. The
daemon resolves the file from `$OPENDASH_SECRETS`, then `$OPENDASH_DIR`, then
`~/.opendash/secrets.toml`; an explicitly named file that does not exist is an
error rather than an invitation to fall back to a different one.

Without a key the dashboard runs exactly as before and the PRICE HISTORY tab
explains what is missing. Nothing else depends on it.

## 11. Watchlist

Three groups, all validated live (quote + news + pre-post data present):

- **AI Core** — NVDA AMD AVGO TSM ARM MRVL INTC ORCL MSFT GOOGL
- **Memory & Storage** — MU WDC STX SNDK
- **Power & Grid** — CEG VST NRG TLN GEV ETN PWR EME

Editable in `config.toml`. Tickers not in a group fall into "Other".

## 12. Build order

1. `config` + `marketclock` + Yahoo chart → quote pipeline
2. Google News RSS + scoring → top-20 pipeline
3. HTML page: header, KPI band, panel tabs, news cards, price table
4. Yahoo per-ticker news + ticker tagging + sparklines + ticker timeline
5. SQLite history
6. launchd + pre-open alert
7. secrets file + Twelve Data + PRICE HISTORY tab

Step 5's history is what makes a later event-study ("how did this ticker actually
move after past similar headlines") possible without re-fetching anything.

## 13. Risks

- The zero-key Yahoo endpoints are unofficial and may throttle or change shape.
  Mitigated by 60 s caching, bounded concurrency, and serving the last SQLite snapshot
  on failure rather than blanking the page.
- Google News RSS is marked for personal, non-commercial use — fine here.
- Pre-market liquidity is thin before 09:00 ET; small gaps are noise.
- A `$`-less lowercase alias match is possible for 2–3 letter tickers; the uppercase
  rule in §7 covers the common cases.
- **The Twelve Data key is the one secret this project holds, and the repo is public.**
  Mitigated by keeping the file outside the repo, mode 600 checked at load, header-only
  auth, a redacted `Debug`, and a `.gitignore` backstop — but a key that reaches a
  commit, a log, or a shared transcript is exposed, and Twelve Data's own dashboard is
  where a rotation happens. The free tier's 800 credits/day is also a real ceiling: heavy
  clicking will exhaust it before the end of a day.
- **Twelve Data's free tier is 8 credits/minute and 800/day.** The PRICE HISTORY tab is
  therefore click-driven and cached, never polled. Expect a visible "no credits left this
  minute" during enthusiastic use; the counter is shown in the picker so it is never a
  surprise.
