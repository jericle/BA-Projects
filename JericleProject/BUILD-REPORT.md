# Build Report — Pre-Open US Market News & Price Dashboard

**Project:** `JericleProject/` · **Binary:** `opendash` · **Built:** 2026-09-26
**Host timezone:** AEST (Sydney) · **All market logic in `America/New_York`**

A local Rust daemon serving a browser dashboard with the top 20 US market headlines and
pre-market price action for a watchlist of AI / memory / power tickers, built to be read
about an hour before the 09:30 ET cash open. No API keys, no npm, no frontend build step.

```sh
cd JericleProject
cargo build --release
./target/release/opendash serve      # → http://127.0.0.1:8787
```

---

## 1. Verification results

| Check | Result |
|---|---|
| `cargo test` | **29/29 pass** — NYSE holidays, DST offsets, alert scheduling, scoring, RSS, option walls, OCC parsing |
| `cargo clippy --release` | **0 warnings** |
| launchd supervision | survives `kill -9`, respawns in <8 s, answers on both stacks |
| Safari reachability | `localhost`, `127.0.0.1` and `[::1]` all return 200 |
| Layout | KPI band full width; beneath it `[Top headlines │ Pre-market]` |
| `scripts/smoke-ui.mjs` | **41/41 pass** — runs the page's real JavaScript against the live API payload under a stub DOM, in three scenarios: after-hours, simulated pre-open with no published volume, and an all-down watchlist |
| Per-card walls | 4 columns, each with its own symbol's chain; 4 parallel loads in ~1 s cold, 0 s cached |
| Live pipeline | 22 quotes, 20 headlines, 0 warnings |
| KPI cards | 4 session cards + 2 option-wall cards render, verified in the headless UI test |
| Option walls, Yahoo path | NVDA / MU / CEG / ARM / GEV all resolve to the current-month expiry |
| Option walls, CBOE fallback | Forced via `OPENDASH_OPTIONS_SOURCE=cboe`; returns results **identical** to Yahoo |
| Cross-feed agreement | call wall 230.00 (OI 90,075), put wall 105.00 (OI 47,486), total call OI 587,399 — same from both feeds |
| Pre-open alert | macOS banner fired with the top 3 headlines + biggest gap movers |
| DST-safe scheduling | `next-alert` → Mon 2026-09-28 08:25 EDT = 22:25 +10:00 local, weekends skipped |
| Upstream outage | 22/22 quotes kept last prices flagged `stale`, 20 headlines retained, 52 warnings surfaced |
| Restart | stored snapshot served immediately; no blank page |

---

## 2. What the dashboard shows

```
┌──────────────────────────────────────────────────────────────────┐
│  PRE·OPEN   [pre-market]  T-minus 04:12:33   ET / local  trending │
├──────────────────────────────────────────────────────────────────┤
│  high vol buy     │ high vol sell    │ top gainer │ top loser         │  ← KPI cards
│  ├ call / put     │ ├ call / put     │ ├ walls    │ ├ walls           │  ← per-card walls
├───────────────────────────────────────┬──────────────────────────┤
│  TOP 20 HEADLINES                     │  PRE-MARKET               │
│  ranked · sentiment · tickers ·       │  grouped by theme         │
│  salience breakdown on hover          │  gap % · sparkline        │
├───────────────────────────────────────┴──────────────────────────┤
│  drawer: 04:00→now ET chart with headline markers at publish time  │
└──────────────────────────────────────────────────────────────────┘
```

### KPI row 1 — session extremes

| Card | Definition |
|---|---|
| **high volume buy** | heaviest-traded name **among those that are up** — directional, not "highest volume" |
| **highest volume sell** | heaviest-traded name among those that are **down** |
| **top gainer** | biggest up move: pre-market gap before the open, last change after |
| **top loser** | biggest down move |

If no name moved that way, the card reads `none up today` instead of crowning a name from
the wrong side. Volume bars are scaled to the busiest name on the watchlist so the two
volume cards are directly comparable.

Each card is clickable and opens that symbol's drawer.

**On volume — a real data limit found and handled.** Yahoo reports `0` for *every*
pre-market 1-minute bar, at every interval (checked 1m, 5m and 15m), even though
pre-market prices clearly move. A naive volume card would therefore read `0` across the
board at 08:25 ET and crown an arbitrary winner. Instead:

* pre-open the cards show `meta.regularMarketVolume` — the last completed session's
  volume — labelled `last sess`, with the pre-market **print count** as the "today"
  activity signal (NVDA 330 prints vs EME 1, which tracks liquidity exactly)
* once the cash session is running they switch to real session volume, labelled `session`
* they never render a bare `0`; both paths are covered by the headless UI test

### Option walls — one block under each card

Each of the four columns carries the walls for **its own symbol**, so the call/put levels
always belong to the ticker directly above them rather than to whatever was last clicked.

Call wall and put wall = the strike carrying the **highest open interest**, matching the
existing `Finance/price_hist.py` definition. Each block shows both strikes with their
distance from spot and the open interest behind them, then a footer with the expiry, the
put/call OI ratio and spot. The expiry is the current ET month; when a name has no listed
expiry left this month the block says `nearest expiry` rather than quietly showing the
wrong month — which is live right now for ARM and ORCL.

Live:

```
[1] Highest Volume Buy   NVDA 76.07M 225.00  +0.24%
      CALL $230.00 +2.2% OI 12.7K   PUT $220.00 -2.3% OI 6.0K   exp 2026-09-28  pc 0.77
[2] Highest Volume Sell  INTC 93.67M 122.95  -0.45%
      CALL $130.00 +5.7% OI  3.5K   PUT $125.00 +1.6% OI 2.8K   exp 2026-09-28  pc 1.14
[3] Top Gainer           ARM  +4.01% 310.45
      CALL $370.00 +19.2% OI 1.7K   PUT $150.00 -51.7% OI 2.5K  exp 2026-10-02  pc 0.69
[4] Top Loser            ORCL  -0.60% 137.04
      CALL $160.00 +16.7% OI 12.5K   PUT $130.00 -5.2% OI 4.9K   exp 2026-10-02  pc 0.67
```

All four chains resolve in ~1 s on a cold cache. The cookie + crumb handshake is behind a
mutex, so four parallel column loads perform one handshake rather than four.

---

## 3. Data sources (all keyless)

| Used for | Endpoint |
|---|---|
| Pre-market + intraday prices | `query1.finance.yahoo.com/v8/finance/chart/{sym}?interval=1m&range=1d&includePrePost=true` |
| Per-ticker news | `query1.finance.yahoo.com/v1/finance/search?q={sym}&newsCount=8` |
| Market-wide news | `news.google.com/rss/search?q=…&hl=en-US&gl=US&ceid=US:en` |
| Also-moving-today | `query1.finance.yahoo.com/v1/finance/trending/US` |
| Option chains | `query2.finance.yahoo.com/v7/finance/options/{sym}` with a cookie + crumb |
| Option fallback | `cdn.cboe.com/api/global/delayed_quotes/options/{sym}.json` (OPRA) |

Yahoo's `v7/finance/quote` and legacy RSS both fail; they are not used.

---

## 4. How the ranking works

Salience is 0–100 and every component is shown on hover:

| Component | Max | Rule |
|---|---|---|
| Recency | 30 | exponential decay, 2 h half-life |
| Cluster | 25 | distinct outlets carrying the same story, log-scaled |
| Impact keywords | 25 | earnings, guidance, FDA, merger, downgrade, SEC, tariff, Fed, … |
| Source tier | 10 | Reuters/Bloomberg/CNBC/WSJ… above aggregators |
| Watchlist relevance | 10 | tagged ticker is on your watchlist |

Then near-duplicate headlines are **clustered** and only the best-scoring member is shown,
so you get the top 20 *stories* rather than 20 syndicated copies of five. Boilerplate is
dropped, off-topic stories are damped to 60% rather than deleted, and sentiment is a
finance-tuned lexicon with negation handling.

---

## 5. Watchlist

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

---

## 6. Things that were wrong and got fixed

Each of these was caught by a test or by looking at real output, not by inspection.

1. **Memorial Day was computed as April 30.** The "last weekday of month" helper
   decremented before checking, so any month whose 31st fell on the target weekday broke.
2. **Google News publishers rendered as "Finance", "Uk", "Investors".** The `<source>`
   element's *text* is the publisher; the `url` attribute is only a fallback.
3. **The same story appeared twice in the top 20.** Added cluster collapse.
4. **A Czech macro story and a "Stock Price, News, Quote" page ranked in the top 10.**
   Added a boilerplate blocklist and a relevance damper.
5. **"S&P Global" the ratings agency matched the `s&p` market marker.** Narrowed to
   `s&p 500`. A unit test pins it.
6. **`pick_expiry` would show an already-expired contract.** On the 28th, the 25th's
   monthly expiry is dead but still in the current month. Now filtered by date.
7. **Yahoo encodes `expirationDates` as midnight UTC**, so reading them in ET shifted
   them back a day and made *today's* live monthly expiry look expired — the card
   silently showed the following week. The options module now uses one UTC frame for
   every comparison and for the displayed date.
8. **The OCC strike field is in thousandths.** Cross-checked against Yahoo, which reports
   `NVDA260925C00050000` as strike 50.0 — not 500. My first test expectation was wrong.
9. **CBOE returns `open_interest: 5.0` as a JSON float**, which serde will not coerce into
   `i64`. The fallback path failed to decode until this was fixed.
10. **A transient network blip zeroed out good prices.** Failed quotes now inherit the
    last known values flagged `stale`; an empty news fetch keeps the previous headlines.
11. **A config key appended after a `[table]` header silently landed in that table.**
    `deny_unknown_fields` turns that into a hard parse error.
12. **Pre-market volume is not published by Yahoo at all.** Found by looking at the
    rendered cards, not by a failing test — every pre-market bar reports `0` volume. The
    cards now fall back to the last completed session's volume and show pre-market print
    count as the activity signal.
13. **A missing `spot` from a feed blanked the whole wall card.** `money()` was called on
    an absent value. Formatting is now tolerant, so one missing key degrades a single
    figure instead of throwing.
14. **The smoke harness silently invented removed elements.** A stub `getElementById`
    created any id on demand, so a scenario still reading a deleted `kpiRow1` got an empty
    node and reported failures with no cause. The harness now allowlists the page's element
    ids and throws on anything else, and a swallowed uncaught exception fails the run.
15. **Column/symbol misalignment when a card is empty.** Walls were looked up from a
    deduplicated symbol list indexed by position, so an empty High Volume Buy card would
    have shown the *next* column's symbol's walls. Each column now carries its own symbol;
    the fetch list is deduplicated separately.
16. **Four parallel chains each did their own cookie + crumb handshake.** Now behind a
    mutex with a double-check, so a cold load performs one handshake.
17. **The High Volume Buy tooltip was built eagerly**, dereferencing the symbol before the
    "nothing is up" empty state could render — so an all-down watchlist threw and blanked
    the whole row. The tooltip is now a thunk evaluated only once a symbol exists, and
    that case is covered by the third test scenario.

---

## 7. Known limitations

- **Yahoo's public endpoints are unofficial.** Throttled politely (4 concurrent, 150 ms
  spacing) and cached, but they can change or disappear. Every failure path degrades
  rather than blanking the page, and the option walls have a second feed behind them.
- **Pre-market liquidity is thin before 09:00 ET.** Yahoo only emits 1-minute pre-market
  bars where a trade printed, so EME legitimately showed one bar today. Those rows say
  `thin pre-market` instead of drawing a chart that implies data that isn't there.
- **Open interest is end-of-day data**, so the walls do not move intraday. The chain is
  cached for 30 minutes (`options_cache_seconds`).
- **Google News RSS is marked personal, non-commercial.** Fine here, but it is not a
  licensed commercial feed.
- **The sentiment lexicon is English-only** and tuned for financial headlines. An
  optional local-LLM rerank is wired in but off by default.

---

## 8. Automation

```sh
./launchd/install.sh            # install + start + health check
./launchd/preopen_alert.sh loop # macOS briefing at 08:25 ET = 22:25 Sydney
```

**A launchd constraint worth knowing about.** The project sits on an external SSD, and
launchd's spawn context blocks indefinitely on `open()` for paths under `/Volumes/*`. A
LaunchAgent aimed at the SSD appears to start — `launchctl print` reports
`state = running`, no error is logged — but the process is stuck in `open()` on
`config.toml` and never binds a port. `sample <pid>` is what pinned it down:

```
main → config::Config::load → std::fs::read_to_string → File::open → open
```

Moving only the runtime directory to the internal volume made it work immediately, so
`install.sh` puts the supervised binary in `~/.local/bin/opendash` and the config in
`~/.opendash/config.toml`, generating the plist with resolved absolute paths. The repo's
`config.toml` remains the source of truth; `--sync` re-copies it. Verified by `kill -9`:
launchd respawned the daemon (pid 10998 → 11020) and it answered within 8 s.

`loop` asks the binary for the next 08:25 ET instant and sleeps until then, rather than
using `StartCalendarInterval`, because the ET→local offset is +14h or +13h depending on
which side is in daylight saving.

---

## 9 Suggested next step

The SQLite store already holds per-cycle prices and per-headline sentiment with cluster
sizes, which is everything an event study needs. The natural follow-up is measuring, for
each sentiment bucket, what the ticker actually did over the following 1 h / 1 d — turning
the dashboard from a monitor into a prediction aid. A starter query is in the README.
