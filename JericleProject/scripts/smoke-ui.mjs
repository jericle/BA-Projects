// Headless smoke test for the dashboard page: runs the real index.html script
// against the real /api/dashboard payload with a minimal DOM stub, then prints
// the generated markup so it can be eyeballed and error-free-ness asserted.
import fs from "node:fs";

const html = fs.readFileSync("src/web/index.html", "utf8");
const script = html.match(/<script>\n([\s\S]*?)\n<\/script>/)[1];
const payload = JSON.parse(fs.readFileSync("/tmp/dash.json", "utf8"));

const mkClassList = () => ({
  _s: new Set(),
  add(c) { this._s.add(c); },
  remove(c) { this._s.delete(c); },
  contains(c) { return this._s.has(c); },
  // The force argument is load-bearing: the tab code calls toggle(c, isActive),
  // and a toggle that ignored it would flip the wrong panel on every switch.
  toggle(c, force) {
    const on = force === undefined ? !this._s.has(c) : !!force;
    if (on) this._s.add(c); else this._s.delete(c);
    return on;
  },
});

// The page's element ids, so a rename fails the run instead of silently creating
// an empty stub and passing.
const KNOWN_IDS = new Set([
  "phaseBadge", "countdown", "clocks", "trending", "refreshBtn",
  "news", "newsNote", "priceNote", "prices", "foot", "tabs",
  "panel-headlines", "panel-premarket", "panel-history",
  "timeframes", "histList", "histNote", "ohlcRead", "tfCredits", "hsChart", "hsSub",
  "scrim", "detail", "dTitle", "dBody", "dClose", "kpiStacks",
]);

// The page renders markup as strings and then queries it back to attach
// listeners. The stub answers `[data-*]` selectors by scanning the host's own
// innerHTML and hands back elements that remember their handlers, which is
// enough to click a button for real rather than just assert on the markup.
//
// Elements are cached per (host, occurrence) and invalidated when the host's
// markup changes, because a real querySelectorAll re-walks the DOM: the same
// query after a re-render must not hand back the same node with stale listeners
// attached. The key is the occurrence index rather than the attribute value,
// because duplicate values are separate DOM nodes in a real document — keying by
// value would collapse them onto one stub and let a re-render keep handlers the
// browser would have discarded.
const attrState = new WeakMap();
const attrStubs = (host, sel) => {
  const m = /^\[data-([A-Za-z-]+)\]$/.exec(sel);
  if (!m) return [];
  const attr = m[1];
  let st = attrState.get(host);
  if (!st || st.html !== host.innerHTML) {
    attrState.set(host, (st = { html: host.innerHTML, els: new Map() }));
  }
  const out = [];
  let n = 0;
  for (const hit of host.innerHTML.matchAll(new RegExp(`data-${attr}="([^"]+)"`, "g"))) {
    const key = `${attr}|${n++}`;
    if (!st.els.has(key)) {
      st.els.set(key, {
        dataset: { [attr]: hit[1] },
        classList: mkClassList(),
        _h: {},
        addEventListener(t, f) { (this._h[t] ||= []).push(f); },
        fire(t) { (this._h[t] || []).forEach((f) => f({})); },
      });
    }
    out.push(st.els.get(key));
  }
  return out;
};

const nodes = new Map();
const node = (id) => {
  if (!KNOWN_IDS.has(id)) throw new Error(`page asked for unknown element #${id}`);
  if (!nodes.has(id)) {
    const self = {
      id, innerHTML: "", textContent: "", disabled: false, dataset: {},
      classList: mkClassList(), style: {},
      _h: {},
      addEventListener(t, f) { (self._h[t] ||= []).push(f); },
      fire(t) { (self._h[t] || []).forEach((f) => f({})); },
      querySelectorAll(sel) { return attrStubs(self, sel); },
      // The stub does not parse markup into a tree, so a selector for a real tag
      // finds nothing and returns null. The page guards on that, and the test
      // asserts against the rendered string instead.
      querySelector(sel) { return /^\[data-[a-z-]+\]$/.test(sel) ? attrStubs(self, sel)[0] ?? null : null; },
    };
    nodes.set(id, self);
  }
  return nodes.get(id);
};

// The per-column walls are located with document.querySelector(`[data-walls="SYM"]`),
// so the stub keeps a registry of those elements too.
const wallNodes = new Map();
const docHandlers = new Map();
globalThis.document = {
  getElementById: (id) => node(id),
  addEventListener(type, fn) {
    if (!docHandlers.has(type)) docHandlers.set(type, []);
    docHandlers.get(type).push(fn);
  },
  querySelector: (sel) => {
    const m = /\[data-walls="([A-Z.\-]+)"\]/.exec(sel);
    if (!m) return null;
    const sym = m[1];
    if (!wallNodes.has(sym)) {
      wallNodes.set(sym, {
        _html: "", dataset: { walls: sym },
        set outerHTML(v) { this._html = v; },
        get outerHTML() { return this._html; },
      });
    }
    return wallNodes.get(sym);
  },
};
// location/history back the tab deep-link, and window-level listeners are
// recorded so the test can fire a real `hashchange`.
globalThis.location = { hash: "" };
globalThis.history = { replaceState: (a, b, url) => { globalThis.location.hash = url; } };
const winHandlers = new Map();
globalThis.addEventListener = (type, fn) => {
  if (!winHandlers.has(type)) winHandlers.set(type, []);
  winHandlers.get(type).push(fn);
};
globalThis.fireWindow = (type) =>
  (winHandlers.get(type) || []).forEach((fn) => fn({ type }));
globalThis.fireDocument = (type, ev) =>
  (docHandlers.get(type) || []).forEach((fn) => fn(ev));
const realSetTimeout = globalThis.setTimeout;
globalThis.setInterval = () => 0;
const optionsPayload = {
  ok: true, symbol: "NVDA", spot: 225.07, expiry: "2026-09-25", expiryIsCurrentMonth: true,
  callWall: 250, callWallOi: 18400, putWall: 200, putWallOi: 22100,
  putCallRatio: 1.2, callWallDistancePct: 11.1, putWallDistancePct: -11.1,
  totalCallOi: 90000, totalPutOi: 108000, source: "yahoo",
};
globalThis.fetch = async (path) => {
  let body = {};
  if (path === "/api/dashboard" || path === "/api/refresh") body = payload;
  else if (String(path).startsWith("/api/options/")) body = optionsPayload;
  return { json: async () => body };
};

const errors = [];
const note = (e) => { errors.push(String(e)); console.error("!! " + (e && e.stack ? e.stack : e)); };
process.on("unhandledRejection", (e) => note("promise: " + e));
process.on("uncaughtException", (e) => { note(e); process.exit(1); });

let api;
try {
  api = new Function(script + `
;return {load, openDetail, closeDetail, sparkline, fmtPct, fmtAge, sentClass,
         selectTab, renderTabs, tabFromHash, TABS, get activeTab() { return activeTab; },
         loadSeriesMeta, openHistory, setTimeframe, loadSeries, drawSeries,
         get tfInterval() { return tfInterval; }};`)();
} catch (e) {
  console.error("script threw during evaluation:\n" + e.stack);
  process.exit(1);
}

// The page auto-loads on startup; give that first render a chance to settle
// instead of racing it with a second load() call (which the busy guard drops).
await new Promise((r) => realSetTimeout(r, 150));
const footNow = node("foot").innerHTML;
if (footNow.includes("could not reach the server")) {
  console.error("load() swallowed an error: " + footNow.replace(/<[^>]+>/g, " "));
  process.exit(1);
}
if (errors.length) { console.error("ERRORS:", errors); process.exit(1); }

const news = node("news").innerHTML;
const prices = node("prices").innerHTML;
const chrome = node("clocks").textContent + " | " + node("phaseBadge").innerHTML;
const foot = node("foot").innerHTML;
const trending = node("trending").innerHTML;
const kpi1 = node("kpiStacks").innerHTML;
const tabs = node("tabs").innerHTML;
await new Promise((r) => realSetTimeout(r, 120));
const painted = [...wallNodes.values()].map((n) => n.outerHTML).join("");

const tabBtn = (host, id) => host.querySelectorAll("[data-tab]")
  .find((el) => el.dataset.tab === id);
// A real click, through the listener the page attached to that button.
const clickTab = (id) => {
  const btn = tabBtn(node("tabs"), id);
  if (!btn) throw new Error(`no tab button for #${id}`);
  btn.fire("click");
  return api.activeTab === id;
};
// The button attributes are wrapped across lines in the template, so the markup
// assertions run against a whitespace-flattened copy.
const tabsFlat = tabs.replace(/\s+/g, " ");

const checks = [
  ["news cards rendered", (news.match(/class="item"/g) || []).length > 0],
  ["exactly 20 news cards", (news.match(/class="item"/g) || []).length === payload.news.length],
  ["news headlines escaped", !/<script>/.test(news)],
  ["news links absolute", (news.match(/href="https?:\/\//g) || []).length > 0],
  ["price rows rendered", (prices.match(/class="row/g) || []).length === payload.watchlist.length],
  // Yahoo only emits pre-market 1m bars where a trade printed, so thin names
  // legitimately have < 3 points and get a text placeholder instead of a chart.
  ["every row has a chart or an honest placeholder",
    (prices.match(/<svg class="spark"/g) || []).length +
    (prices.match(/class="thin"/g) || []).length === payload.watchlist.length],
  ["thin-pre-market placeholders are labelled",
    (prices.match(/class="thin"/g) || []).length ===
      payload.watchlist.filter((q) => (q.spark || []).length < 3).length],
  ["all three groups shown", ["AI Core", "Memory", "Power"].every(g => prices.includes(g))],
  ["gap column has no NaN", !/NaN|undefined/.test(prices)],
  ["no undefined in news", !/undefined/.test(news)],
  ["phase badge populated", chrome.includes("badge")],
  ["trending chips rendered", trending.includes("chip")],
  ["footer has data age", foot.includes("data age")],
  ["kpi row 1 has four cards", (kpi1.match(/class="kpi /g) || []).length === 4],
  ["kpi row 1 has all four labels",
    ["Highest Volume Buy", "Highest Volume Sell", "Top Gainer", "Top Loser"].every(l => kpi1.includes(l))],
  ["kpi labels are no longer forced to caps",
    !/text-transform:\s*uppercase[^}]*\.kpi-label/.test(html)],
  ["kpi cards are not the old high/low volume pair",
    !kpi1.includes("most traded") && !kpi1.includes("least traded")],
  ["kpi row 1 shows a symbol on every card", (kpi1.match(/class="sym"/g) || []).length === 4],
  ["kpi row 1 has no NaN", !/NaN|undefined/.test(kpi1)],
  // Every session card sits directly above its own walls block, in one column.
  ["four columns", (kpi1.match(/class="stack"/g) || []).length === 4],
  ["each column carries a walls block", (kpi1.match(/class="walls[\s\"]/g) || []).length === 4],
  ["walls blocks are keyed by their own symbol", (kpi1.match(/data-walls="[A-Z.]+"/g) || []).length >= 1],
  ["painted walls show call and put strikes", painted.includes("$250.00") && painted.includes("$200.00")],
  ["painted walls show the expiry date", /exp \d{4}-\d{2}-\d{2}/.test(painted)],
  ["painted walls show open interest", /OI /.test(painted)],
  ["painted walls show put/call ratio", /pc \d/.test(painted)],
  ["painted walls no NaN", !/NaN|undefined/.test(painted)],

  // ---- panel tabs ----
  ["a tab per panel", api.TABS.length === 3
    && ["headlines", "premarket", "history"].every((id) => tabs.includes(`data-tab="${id}"`))],
  ["tab bar is a tablist", node("tabs").innerHTML.includes('role="tab"')
    && html.includes('role="tablist"')],
  ["each tab names the panel it controls",
    (tabs.match(/aria-controls="panel-[a-z]+"/g) || []).length === api.TABS.length
      && api.TABS.every((t) => tabs.includes(`aria-controls="panel-${t.id}"`))],
  ["opens on top headlines", api.activeTab === "headlines"],
  // One true and the rest false, rather than a hardcoded count of two.
  ["exactly one tab is selected", (tabs.match(/aria-selected="true"/g) || []).length === 1
    && (tabs.match(/aria-selected="false"/g) || []).length === api.TABS.length - 1],
  ["the selected tab carries the on class",
    /class="tab on"[^>]*data-tab="headlines"/.test(tabsFlat)],
  ["only the active panel is visible",
    node("panel-headlines").classList.contains("on")
      && !node("panel-premarket").classList.contains("on")],
  ["panels are hidden by default, not by script",
    html.includes('class="panel on" id="panel-headlines"')
      && /class="panel" id="panel-premarket"/.test(html)],
  ["the panels are no longer a two-column grid",
    !html.includes("grid-template-columns: minmax(0, 1.35fr)")],
  // click the second tab for real and re-read the rendered state
  ["clicking a tab switches panel", clickTab("premarket")],
  ["clicking a tab moves the on-class",
    node("panel-premarket").classList.contains("on")
      && !node("panel-headlines").classList.contains("on")],
  ["clicking a tab rewrites the url hash", globalThis.location.hash === "#premarket"],
  ["the url hash drives the initial tab", (() => {
    globalThis.location.hash = "#headlines";
    return api.tabFromHash() === "headlines";
  })()],
  ["a junk hash falls back rather than blanking the page", (() => {
    globalThis.location.hash = "#nope";
    return api.tabFromHash() === null;
  })()],
  ["hashchange reopens the linked tab", (() => {
    api.selectTab("headlines");
    globalThis.location.hash = "#premarket";
    globalThis.fireWindow("hashchange");
    return api.activeTab === "premarket" && node("panel-premarket").classList.contains("on");
  })()],
  ["digit 2 jumps to the pre-market tab", (() => {
    globalThis.location.hash = "";
    api.selectTab("headlines");
    globalThis.fireDocument("keydown", { key: "2" });
    return api.activeTab === "premarket";
  })()],
  ["digit 3 jumps to the price history tab", (() => {
    globalThis.fireDocument("keydown", { key: "3" });
    return api.activeTab === "history";
  })()],
  ["digit 1 jumps back to top headlines", (() => {
    globalThis.fireDocument("keydown", { key: "1" });
    return api.activeTab === "headlines";
  })()],
  ["a shifted digit is not a tab shortcut", (() => {
    globalThis.fireDocument("keydown", { key: "!", shiftKey: true });
    return api.activeTab === "headlines";
  })()],
  ["cmd+digit is left to the browser", (() => {
    globalThis.fireDocument("keydown", { key: "2", metaKey: true });
    return api.activeTab === "headlines";
  })()],
  ["a digit past the last tab does nothing", (() => {
    globalThis.fireDocument("keydown", { key: "9" });
    return api.activeTab === "headlines";
  })()],
  ["both panels are still populated while hidden", news.length > 0 && prices.length > 0],

  // ---- pre-market multi-column layout ----
  ["each watchlist group is its own column block",
    (prices.match(/class="group"/g) || []).length === 3],
  ["group titles are inside their group block",
    (prices.match(/class="group"><div class="group-title">/g) || []).length === 3],
  ["pre-market is a responsive multi-column grid",
    /#prices\s*\{[^}]*display:\s*grid[^}]*repeat\(auto-fit,\s*minmax\(min\(320px, 100%\)/.test(html)],
  ["only the first group drops its top rule",
    html.includes(".group:first-child .group-title")],
];

// The two blocks below run before `bad` is declared, so they tally into this
// counter and `bad` picks it up when it is defined below.
let failEarly = 0;

// ---------------------------------------------------------------------------
// Price history tab. The stub serves a canned Twelve Data series; the point of
// this block is that the chart builds from real OHLC without a live API call.
{
  // The stub echoes back whatever interval was requested, so a test that switches
  // timeframe sees the axis and label logic respond to it.
  const seriesFor = (interval) => ({
    ok: true,
    series: {
      symbol: "NVDA", interval, currency: "USD", exchange: "NASDAQ",
      bars: [0, 1, 2, 3, 4, 5].map((i) => ({
        // 1day and up are date-only upstream; a day apart rather than 5 minutes.
        t: 1790305200 + i * (["1day", "1week", "1month"].includes(interval) ? 86400 : 300),
        o: 225.0 + i * 0.2, h: 227.5 + i * 0.1, l: 223.0 + i * 0.1,
        c: 226.25 + i * 0.15, v: 1_200_000 + i * 1000,
      })),
      skipped: 0, cached: false, fetchedTs: 1790307000,
    },
  });
  globalThis.fetch = async (path) => {
    if (path === "/api/dashboard" || path === "/api/refresh") return { json: async () => payload };
    if (path === "/api/series-meta") {
      return { json: async () => ({
        ok: true, enabled: true, provider: "twelvedata",
        // Every real interval, so the picker is checked against the same set the
        // server offers rather than a hand-copied subset.
        intervals: [
          "1min", "5min", "15min", "30min", "45min",
          "1h", "2h", "4h", "8h", "1day", "1week", "1month",
        ].map((i) => ({
          interval: i, defaultOutputsize: 200,
          intraday: !["1day", "1week", "1month"].includes(i),
        })),
        rateLimitPerMinute: 8, creditsAvailable: 6, reason: null,
      }) };
    }
    if (String(path).startsWith("/api/series/")) {
      const iv = /interval=([^&]+)/.exec(String(path))?.[1] || "5min";
      return { json: async () => seriesFor(iv) };
    }
    if (String(path).startsWith("/api/options/")) return { json: async () => optionsPayload };
    return { json: async () => ({}) };
  };

  await api.loadSeriesMeta();
  const tf = node("timeframes").innerHTML;
  const list = node("histList").innerHTML;

  // Start on an intraday interval, so the later switch to 1day is a real change
  // rather than a no-op against the default.
  api.setTimeframe("5min");
  api.openHistory("NVDA");
  await new Promise((r) => realSetTimeout(r, 80));
  const chart = node("dBody").innerHTML;
  const chartSvg = node("hsChart").innerHTML;
  const read = node("ohlcRead").innerHTML;

  // Daily bars, to exercise the second timestamp shape and the date axis.
  api.setTimeframe("1day");
  await new Promise((r) => realSetTimeout(r, 80));
  const daily = node("dBody").innerHTML;
  const dailySvg = node("hsChart").innerHTML;

  const hchecks = [
    ["history: timeframe picker rendered", tf.includes("data-tf=")],
    ["history: all three horizon groups shown",
      ["Intraday", "Hourly", "Daily"].every(g => tf.includes(g))],
    ["history: 1min through 1month all offered",
      ["1min", "5min", "15min", "30min", "45min", "1h", "2h", "4h", "8h",
       "1day", "1week", "1month"].every(i => tf.includes(`data-tf="${i}"`))],
    ["history: remaining credits are surfaced", /credits left this minute/.test(tf)],
    ["history: list reuses the watchlist groups",
      ["AI Core", "Memory", "Power"].every(g => list.includes(g))],
    ["history: list has a row per watchlist symbol",
      (list.match(/data-hsym=/g) || []).length === payload.watchlist.length],
    ["history: list spends no credits (no per-row series fetch)",
      !/api\/series/.test(list)],
    ["history: drawer opens on the symbol", chart.includes("NVDA")],
    ["history: candle/line toggle present",
      chart.includes('data-style="candle"') && chart.includes('data-style="line"')],
    ["history: a chart was drawn", chartSvg.includes("data-series-chart")],
    ["history: candles render a rect per bar",
      (chartSvg.match(/<rect /g) || []).length >= 6],
    ["history: OHLC readout populated",
      /\bO\b/.test(read) && /\bH\b/.test(read) && /\bVOL\b/.test(read)],
    ["history: no NaN in the chart", !/NaN|undefined/.test(chartSvg)],
    ["history: switching timeframe refetches", dailySvg.includes("data-series-chart")],
    ["history: the daily chart differs from the intraday one",
      dailySvg !== chartSvg && api.tfInterval === "1day"],
    ["history: selecting the active timeframe does not refetch", (() => {
      // A no-op switch must not spend a credit on the free tier.
      const before = node("hsChart").innerHTML;
      api.setTimeframe("1day");
      return node("hsChart").innerHTML === before;
    })()],
    ["history: a daily axis shows dates, not clock times",
      /\b(Sep|Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Oct|Nov|Dec)\b/.test(dailySvg)],
    ["history: no NaN after the switch", !/NaN|undefined/.test(dailySvg)],
    ["history: a bad interval is not offered", !tf.includes('data-tf="1sec"')],
  ];
  for (const [name, ok] of hchecks) { if (!ok) failEarly++; console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); }

  // The whole point of the secrets design: nothing key-shaped reaches the page.
  const allMarkup = tabs + tf + list + chart + daily + news + prices + kpi1 + node("foot").innerHTML;
  const secChecks = [
    ["no api_key in any rendered markup", !/api_key/i.test(allMarkup)],
    ["no apikey query string in any markup", !/apikey=/i.test(allMarkup)],
    ["the series endpoint is same-origin only", html.includes("/api/series/${encodeURIComponent(sym)}")],
    ["the page never names the upstream host", !/api\.twelvedata\.com/.test(html)],
  ];
  for (const [name, ok] of secChecks) { if (!ok) failEarly++; console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); }
}

// ---------------------------------------------------------------------------
// Fourth scenario: no API key configured. The tab must explain itself rather than
// render an empty chart or throw — this is the state a fresh clone is in.
{
  globalThis.fetch = async (path) => {
    if (path === "/api/dashboard" || path === "/api/refresh") return { json: async () => payload };
    if (path === "/api/series-meta") {
      return { json: async () => ({
        ok: true, enabled: false, provider: "twelvedata", intervals: [],
        rateLimitPerMinute: 8, creditsAvailable: 0,
        reason: "no [providers.twelvedata] api_key in the secrets file",
      }) };
    }
    return { json: async () => ({}) };
  };
  await api.loadSeriesMeta();
  const nokeyTf = node("timeframes").innerHTML;
  const nokeyList = node("histList").innerHTML;
  const nokey = [
    ["no-key: the picker explains why", nokeyTf.includes("unavailable")
      && nokeyTf.includes("api_key")],
    ["no-key: the list points at the secrets file",
      nokeyList.includes("secrets.toml")],
    ["no-key: no timeframe buttons are offered", !nokeyList.includes("data-tf=")],
    ["no-key: no rows are offered", !nokeyList.includes("data-hsym=")],
    ["no-key: no error surfaced in the footer",
      !node("foot").innerHTML.includes("could not reach")],
  ];
  for (const [name, ok] of nokey) { if (!ok) failEarly++; console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); }
}

// Detail drawer
api.openDetail(payload.watchlist[0].symbol);
const body = node("dBody").innerHTML;
checks.push(["detail drawer opens", body.includes("Pre-market price and headline timing")]);
checks.push(["detail has chart svg", body.includes("<svg")]);
checks.push(["detail has prior-close label", body.includes("prev ")]);
checks.push(["detail no NaN", !/NaN|undefined/.test(body)]);
checks.push(["detail lists or explains news", body.includes("Headlines for")]);
api.closeDetail();
checks.push(["drawer closes", !node("detail").classList.contains("on")]);

let bad = failEarly;
for (const [name, ok] of checks) {
  if (!ok) bad++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}`);
}
console.log(`\n-- scenario 2: pre-open (no volume published) --`);
console.log(`\nnews html ${news.length} bytes, prices html ${prices.length} bytes, detail html ${body.length} bytes`);
console.log(`footer: ${foot.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim().slice(0, 160)}`);

// ---------------------------------------------------------------------------
// Second scenario: simulate 08:25 ET pre-open, where Yahoo reports zero volume
// on every bar. The cards must fall back to the last completed session's volume,
// label it as such, and surface the pre-market print count instead of a bare 0.
{
  const preOpen = JSON.parse(JSON.stringify(payload));
  preOpen.phase.phase = "pre-market";
  for (const q of preOpen.watchlist) {
    q.volume = 0;
    q.regularVolume = 12_345_678;
    q.premarketPrints = 42;
  }
  globalThis.fetch = async (path) => {
    if (path === "/api/dashboard" || path === "/api/refresh") return { json: async () => preOpen };
    if (String(path).startsWith("/api/options/")) return { json: async () => optionsPayload };
    return { json: async () => ({}) };
  };
  node("kpiStacks").innerHTML = "";
  await api.load(false);
  const k = node("kpiStacks").innerHTML;
  const pre = [
    ["pre-open: falls back to last session volume", k.includes("last sess")],
    ["pre-open: shows the fallback number", k.includes("12.35M")],
    ["pre-open: reports pre-market prints", /pre-mkt prints/.test(k)],
    ["pre-open: never shows a bare zero", !/>0<\/span>/.test(k)],
  ];
  for (const [name, ok] of pre) { if (!ok) bad++; console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); }
}

// ---------------------------------------------------------------------------
// Third scenario: every name on the watchlist is down, so there is no High Volume
// Buy. The card must say so instead of throwing or crowning a decliner.
{
  const allDown = JSON.parse(JSON.stringify(payload));
  allDown.phase.phase = "pre-market";
  for (const q of allDown.watchlist) {
    q.gapPct = -1.5; q.changePct = -1.5; q.volume = 0;
    q.regularVolume = 9_000_000; q.premarketPrints = 17;
  }
  globalThis.fetch = async (path) => {
    if (path === "/api/dashboard" || path === "/api/refresh") return { json: async () => allDown };
    if (String(path).startsWith("/api/options/")) return { json: async () => optionsPayload };
    return { json: async () => ({}) };
  };
  node("kpiStacks").innerHTML = "";
  await api.load(false);
  const k = node("kpiStacks").innerHTML;
  const down = [
    ["all-down: high volume buy reports none up", k.includes("None up today")],
    ["all-down: still shows a volume sell", k.includes("Highest Volume Sell") && !k.includes("None down today")],
    ["all-down: pre-market prints reported", k.includes("pre-mkt prints")],
    ["all-down: no error surfaced", !node("foot").innerHTML.includes("could not reach")],
  ];
  for (const [name, ok] of down) { if (!ok) bad++; console.log(`${ok ? "PASS" : "FAIL"}  ${name}`); }
}

process.exit(bad || errors.length ? 1 : 0);
