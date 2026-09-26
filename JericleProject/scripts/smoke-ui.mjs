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
  toggle(c) { this._s.has(c) ? this._s.delete(c) : this._s.add(c); },
});

// The page's element ids, so a rename fails the run instead of silently creating
// an empty stub and passing.
const KNOWN_IDS = new Set([
  "phaseBadge", "countdown", "clocks", "trending", "refreshBtn",
  "news", "newsNote", "priceNote", "prices", "foot",
  "scrim", "detail", "dTitle", "dBody", "dClose", "kpiStacks",
]);

const nodes = new Map();
const node = (id) => {
  if (!KNOWN_IDS.has(id)) throw new Error(`page asked for unknown element #${id}`);
  if (!nodes.has(id)) {
    nodes.set(id, {
      id, innerHTML: "", textContent: "", disabled: false,
      classList: mkClassList(), style: {},
      querySelectorAll: () => [{ classList: mkClassList(), addEventListener: () => {} }],
      addEventListener: () => {},
    });
  }
  return nodes.get(id);
};

// The per-column walls are located with document.querySelector(`[data-walls="SYM"]`),
// so the stub keeps a registry of those elements too.
const wallNodes = new Map();
globalThis.document = {
  getElementById: (id) => node(id),
  addEventListener: () => {},
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
  api = new Function(script + "\n;return {load, openDetail, closeDetail, sparkline, fmtPct, fmtAge, sentClass};")();
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
await new Promise((r) => realSetTimeout(r, 120));
const painted = [...wallNodes.values()].map((n) => n.outerHTML).join("");

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
];

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

let bad = 0;
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
