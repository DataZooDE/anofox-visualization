// Node test for the examples library in web/app.js (sidebar plan, PR 1): every
// example's place comes from its own header comments (`-- @title`, `-- @group`,
// `-- @tags`, read by the wasm dashboard_meta — the servers' parser), ids are
// unique slugs, and every `?dashboard=<name>` that worked before still opens
// the same example (LEGACY_NAMES). Skips the wasm part without web/pkg
// (`wasm-pack build --target web --out-dir web/pkg --no-default-features
// --features wasm`).
import { readFileSync, readdirSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (re, what) => {
  const m = src.match(re);
  assert.ok(m, `${what} not found in app.js`);
  return m[0];
};
const fn = (name) => grab(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"), `function ${name}`);
const block = (name, open, close) =>
  grab(new RegExp(`^const ${name} = \\${open}[\\s\\S]*?^\\${close};`, "m"), `const ${name}`);
const inline = grab(/^const SESSIONS = [\s\S]*?^const INLINE_SAMPLES = \[[\s\S]*?^\];/m, "INLINE_SAMPLES");
const lib = new Function(
  [
    inline,
    block("EX_AREAS", "[", "]"),
    grab(/^const EX_AREAS_CLOSED = .*;$/m, "EX_AREAS_CLOSED"),
    block("LEGACY_NAMES", "{", "}"),
    block("FILE_SAMPLES", "[", "]"),
    ...["slugify", "exampleEntry", "resolveDash", "exampleAreas", "exLibOpen", "exAreaOpen", "startDash"].map(fn),
    "return { INLINE_SAMPLES, EX_AREAS, EX_AREAS_CLOSED, LEGACY_NAMES, FILE_SAMPLES, slugify, exampleEntry, resolveDash, exampleAreas, exLibOpen, exAreaOpen, startDash };",
  ].join("\n")
)();
const { slugify, resolveDash, exampleAreas, exLibOpen, exAreaOpen, startDash } = lib;

// slugs: lower-case, ASCII, dashes; "&" reads "and"; never empty
assert.equal(slugify("Who should sales call first?"), "who-should-sales-call-first");
assert.equal(slugify("Groups, tabs & height"), "groups-tabs-and-height");
assert.equal(slugify("  Größe – Übersicht! "), "grosse-ubersicht");
assert.equal(slugify("???"), "dashboard");

// Every example file is in the sidebar, once.
const files = readdirSync(new URL("../../web/examples/", import.meta.url)).filter((f) => f.endsWith(".sql"));
assert.deepEqual([...lib.FILE_SAMPLES].map((u) => u.replace("examples/", "")).sort(), files.sort());
assert.equal(new Set(lib.FILE_SAMPLES).size, lib.FILE_SAMPLES.length);

// Library state: open while the viewer has no dashboards, else closed, unless
// they chose; the feature tours start closed.
assert.equal(exLibOpen({ areas: {} }, 0), true);
assert.equal(exLibOpen({ areas: {} }, 3), false);
assert.equal(exLibOpen({ open: true, areas: {} }, 3), true);
assert.equal(exLibOpen({ open: false, areas: {} }, 0), false);
assert.equal(exAreaOpen({ areas: {} }, "How it works"), false);
assert.equal(exAreaOpen({ areas: {} }, "Demand & stock"), true);
assert.equal(exAreaOpen({ areas: { "How it works": true } }, "How it works"), true);

// Areas: EX_AREAS order, unknown areas after them by name, ungrouped last-ish,
// one level of nesting as a sub-heading.
const fake = (name, group) => ({ id: slugify(name), name, group, tags: [], source: "example", sql: "" });
const areas = exampleAreas([
  fake("a", "Zebra"),
  fake("b", "How it works"),
  fake("c", "Start here"),
  fake("d", "Alpha / One"),
  fake("e", "Alpha"),
  fake("f", "Alpha / One / Deep"),
  fake("g", ""),
]);
assert.deepEqual(
  areas.map((a) => a.area),
  ["Start here", "How it works", "Alpha", "More examples", "Zebra"]
);
assert.deepEqual(
  areas[2].items.map((i) => [i.sub, i.entry.name]),
  [["One", "d"], ["", "e"], ["One / Deep", "f"]]
);

// Every name master's sidebar had (2026-10-17, before the library), each with
// the example (by its file or its inline script's first SQL line) it opened.
const MASTER_NAMES = {
  Overview: "inline:Overview",
  "Signal explorer": "inline:No table needed",
  "Chart gallery": "inline:Chart gallery",
  "Filters & inputs": "inline:Filters & inputs",
  "Selections & drill": "inline:Selections & drill",
  "Formatting & paging": "inline:Tables — rich per-column",
  "Groups, tabs & height": "inline:Layout — groups, tabs",
  "How much will each item sell next year?": "inline:Live in-browser SeasonalES",
  "Which forecast method fits our demand?": "inline:M5 — MFLES vs MSTL",
  "How does data move through our pipeline?": "diagram-pipeline.sql",
  "Period & KPIs": "period-kpis.sql",
  "How will demand develop after today?": "motion-demand.sql",
  "When could we run out of stock?": "stock-outlook.sql",
  "A sales cockpit in your brand": "theming-branded.sql",
  "A German branch report": "theming-de-locale.sql",
  "What sets the price of a home?": "statistics-showcase.sql",
  "Did the new checkout lift conversion?": "statistics-ab-test.sql",
  "Why do customers cancel?": "statistics-churn-drivers.sql",
  "Will any substation exceed its capacity next week?": "forecast-energy-load.sql",
  "How many agents do we need per half-hour?": "forecast-callcentre-staffing.sql",
  "Can we switch on the new CRM?": "tabular-showcase.sql",
  "Is today's supplier feed safe to load?": "tabular-feed.sql",
  "Which duplicate customers should we merge?": "tabular-dedup.sql",
  "How do we plan a busy day at the distribution centre?": "optimize-showcase.sql",
  "Which line runs which order, with the least changeover?": "optimize-production.sql",
  "Which van serves which stop tomorrow?": "optimize-delivery.sql",
  "What should we order this week to sell the most?": "optimize-replenishment.sql",
  "Can a model triage the support inbox?": "decide-showcase.sql",
  "Which claims can we fast-track?": "decide-claims.sql",
  "Who should sales call first?": "decide-leads.sql",
  "Where will stock run short, and can we trust the plan?": "inventory-twin.sql",
  "Where does our supply chain amplify demand, and what would stabilise it?": "supply-chain.sql",
  "How reliable are our suppliers, and what does it cost us in stock?": "supplier-lead-times.sql",
};
assert.equal(Object.keys(MASTER_NAMES).length, 33);
for (const id of Object.values(lib.LEGACY_NAMES)) assert.match(id, /^[a-z0-9-]+$/);

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of examples_lib_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });

// The library exactly as app.js builds it: inline scripts, then the files.
const EXAMPLES = [];
const origin = new Map(); // id → what it was built from
const add = (sql, fallback, from) => {
  const e = lib.exampleEntry(sql, JSON.parse(wasm.dashboard_meta(sql)), fallback);
  EXAMPLES.push(e);
  origin.set(e.id, from);
};
lib.INLINE_SAMPLES.forEach((sql, i) => add(sql, `Example ${i + 1}`, sql));
for (const url of lib.FILE_SAMPLES) {
  const f = url.replace("examples/", "");
  add(readFileSync(new URL(`../../web/${url}`, import.meta.url), "utf8"), f, f);
}

assert.equal(EXAMPLES.length, 33);
assert.equal(new Set(EXAMPLES.map((e) => e.id)).size, EXAMPLES.length, "example ids are unique");
for (const e of EXAMPLES) {
  assert.ok(/^-- @title /m.test(e.sql), `${e.name}: no -- @title`);
  assert.ok(lib.EX_AREAS.includes(e.group), `${e.name}: @group "${e.group}" is not a business area`);
  assert.ok(e.tags.length >= 1 && e.tags.length <= 10, `${e.name}: ${e.tags.length} tags`);
  assert.ok(e.name.endsWith("?"), `${e.name}: name it as a plain question`);
  assert.equal(e.id, slugify(e.name));
  // examples first: an id opens the example even when a saved dashboard has its name
  assert.equal(resolveDash(e.id, EXAMPLES, { [e.id]: { sql: "x" } }).entry, e);
  assert.equal(resolveDash(e.name, EXAMPLES, { [e.name]: { sql: "x" } }).entry, e);
}
for (const a of lib.EX_AREAS) assert.ok(EXAMPLES.some((e) => e.group === a), `area "${a}" is empty`);
// the extension feature tags stay searchable
for (const t of ["charts", "tables", "forecast", "statistics", "decide", "optimize", "tabular"])
  assert.ok(EXAMPLES.some((e) => e.tags.includes(t)), `no example tagged ${t}`);

// Every old name still opens the same example.
for (const [name, want] of Object.entries(MASTER_NAMES)) {
  const r = resolveDash(name, EXAMPLES, {});
  assert.ok(r && r.kind === "example", `?dashboard=${name} no longer resolves`);
  const from = origin.get(r.entry.id);
  if (want.startsWith("inline:")) assert.ok(from.includes(want.slice(7)), `${name} → ${r.entry.name}`);
  else assert.equal(from, want, `${name} → ${r.entry.name}`);
}
for (const name of Object.keys(lib.LEGACY_NAMES)) assert.ok(MASTER_NAMES[name], `LEGACY_NAMES has an unknown name ${name}`);

// Saved dashboards: by name after the examples; `my:` addresses them only.
const saved = { "Weekly sales": { sql: "SELECT 1" }, "Who should sales call first?": { sql: "SELECT 2" } };
assert.deepEqual(resolveDash("Weekly sales", EXAMPLES, saved), { kind: "mine", name: "Weekly sales", sql: "SELECT 1" });
assert.equal(resolveDash("my:weekly-sales", EXAMPLES, saved).name, "Weekly sales");
assert.equal(resolveDash("my:Weekly sales", EXAMPLES, saved).name, "Weekly sales");
assert.equal(resolveDash("my:who-should-sales-call-first", EXAMPLES, saved).sql, "SELECT 2");
assert.equal(resolveDash("Who should sales call first?", EXAMPLES, saved).kind, "example");
assert.equal(resolveDash("my:overview", EXAMPLES, saved), null);
for (const miss of ["", null, "nope", "__proto__", "constructor", "my:", "my:__proto__"])
  assert.equal(resolveDash(miss, EXAMPLES, saved), null, String(miss));

// Without a link: the dashboard open last, if it still exists; else the first
// saved one; else null (the first example).
assert.deepEqual(startDash("my:Weekly sales", EXAMPLES, saved), { kind: "mine", name: "Weekly sales", sql: "SELECT 1" });
assert.equal(startDash("ex:why-do-customers-cancel", EXAMPLES, saved).entry.name, "Why do customers cancel?");
assert.equal(startDash("ex:gone", EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash("my:Deleted", EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash(null, EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash("ex:why-do-customers-cancel", EXAMPLES, {}).entry.id, "why-do-customers-cancel");
assert.equal(startDash("my:Weekly sales", EXAMPLES, {}), null);
assert.equal(startDash(null, EXAMPLES, {}), null);
assert.equal(startDash("my:__proto__", EXAMPLES, {}), null);

// dashboard_meta is the servers' parser: group levels, tag clean-up, the cap.
const m = JSON.parse(wasm.dashboard_meta("-- @title  T\n-- @group Sales/EMEA\n-- @tags A, b ,a\nSELECT 1;"));
assert.deepEqual(m, { title: "T", group: "Sales / EMEA", tags: ["a", "b"], refresh: 0 });
assert.deepEqual(JSON.parse(wasm.dashboard_meta("SELECT 1;")), { title: "", group: null, tags: [], refresh: 0 });

console.log(`examples_lib_test: ok (${EXAMPLES.length} examples in ${exampleAreas(EXAMPLES).length} areas)`);
