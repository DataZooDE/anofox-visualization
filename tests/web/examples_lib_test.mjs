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

assert.equal(EXAMPLES.length, 36);
assert.ok(EXAMPLES.some((e) => e.name === "Where do our deliveries and customers cluster, and which regions are underserved?" && e.group === "Logistics & production"), "the delivery-coverage map showcase");
assert.equal(new Set(EXAMPLES.map((e) => e.id)).size, EXAMPLES.length, "example ids are unique");
for (const e of EXAMPLES) {
  assert.ok(/^-- @title /m.test(e.sql), `${e.name}: no -- @title`);
  assert.ok(lib.EX_AREAS.includes(e.group), `${e.name}: @group "${e.group}" is not a business area`);
  assert.ok(e.tags.length >= 1 && e.tags.length <= 10, `${e.name}: ${e.tags.length} tags`);
  assert.ok(e.name.endsWith("?"), `${e.name}: name it as a plain question`);
  assert.equal(e.id, slugify(e.name));
  // examples first: an id opens the example even when a saved dashboard has its name
  assert.equal(resolveDash(e.id, EXAMPLES, [{ id: e.id, name: e.id, sql: "x" }]).entry, e);
  assert.equal(resolveDash(e.name, EXAMPLES, [{ id: "x", name: e.name, sql: "x" }]).entry, e);
}
for (const a of lib.EX_AREAS) assert.ok(EXAMPLES.some((e) => e.group === a), `area "${a}" is empty`);
// the extension feature tags stay searchable
for (const t of ["charts", "tables", "forecast", "statistics", "decide", "optimize", "tabular"])
  assert.ok(EXAMPLES.some((e) => e.tags.includes(t)), `no example tagged ${t}`);

// -- @nav (#355): exampleEntry carries a short nav label distinct from the
// full name used for its id, its tooltip and every menu action.
assert.equal(lib.exampleEntry("SELECT 1;", { title: "Which products depend on this supplier?", nav: "Supplier risk" }, "fallback").nav, "Supplier risk");
assert.equal(lib.exampleEntry("SELECT 1;", { title: "T" }, "fallback").nav, "", "no @nav: an empty string, not undefined");
assert.equal(lib.exampleEntry("SELECT 1;", null, "fallback").nav, "", "an unreadable header: no @nav either");

// Every old name still opens the same example.
for (const [name, want] of Object.entries(MASTER_NAMES)) {
  const r = resolveDash(name, EXAMPLES, []);
  assert.ok(r && r.kind === "example", `?dashboard=${name} no longer resolves`);
  const from = origin.get(r.entry.id);
  if (want.startsWith("inline:")) assert.ok(from.includes(want.slice(7)), `${name} → ${r.entry.name}`);
  else assert.equal(from, want, `${name} → ${r.entry.name}`);
}
for (const name of Object.keys(lib.LEGACY_NAMES)) assert.ok(MASTER_NAMES[name], `LEGACY_NAMES has an unknown name ${name}`);

// Saved dashboards ([{id, name, sql}], the store's list): by name after the
// examples; `my:` addresses them only (by id, name or the name's slug).
const saved = [
  { id: "weekly", name: "Weekly sales", sql: "SELECT 1" },
  { id: "who-should-sales-call-first", name: "Who should sales call first?", sql: "SELECT 2" },
];
const ws = { kind: "mine", id: "weekly", name: "Weekly sales", sql: "SELECT 1" };
assert.deepEqual(resolveDash("Weekly sales", EXAMPLES, saved), ws);
assert.deepEqual(resolveDash("my:weekly", EXAMPLES, saved), ws);
assert.equal(resolveDash("my:weekly-sales", EXAMPLES, saved).name, "Weekly sales");
assert.equal(resolveDash("my:Weekly sales", EXAMPLES, saved).name, "Weekly sales");
assert.equal(resolveDash("my:who-should-sales-call-first", EXAMPLES, saved).sql, "SELECT 2");
assert.equal(resolveDash("Who should sales call first?", EXAMPLES, saved).kind, "example");
assert.equal(resolveDash("my:overview", EXAMPLES, saved), null);
for (const miss of ["", null, "nope", "__proto__", "constructor", "my:", "my:__proto__"])
  assert.equal(resolveDash(miss, EXAMPLES, saved), null, String(miss));

// Without a link: the dashboard open last, if it still exists; else the first
// saved one; else null (the first example).
assert.deepEqual(startDash("my:weekly", EXAMPLES, saved), ws);
assert.deepEqual(startDash("my:Weekly sales", EXAMPLES, saved), ws, "a key from before ids (my:<name>)");
assert.equal(startDash("ex:why-do-customers-cancel", EXAMPLES, saved).entry.name, "Why do customers cancel?");
assert.equal(startDash("ex:gone", EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash("my:Deleted", EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash(null, EXAMPLES, saved).name, "Weekly sales");
assert.equal(startDash("ex:why-do-customers-cancel", EXAMPLES, []).entry.id, "why-do-customers-cancel");
assert.equal(startDash("my:Weekly sales", EXAMPLES, []), null);
assert.equal(startDash(null, EXAMPLES, []), null);
assert.equal(startDash("my:__proto__", EXAMPLES, []), null);

// dashboard_meta is the servers' parser: group levels, tag clean-up, the cap.
const m = JSON.parse(wasm.dashboard_meta("-- @title  T\n-- @group Sales/EMEA\n-- @tags A, b ,a\nSELECT 1;"));
assert.deepEqual(m, { title: "T", nav: null, group: "Sales / EMEA", tags: ["a", "b"], refresh: 0, description: null, owner: null, certified: null, astables: false, tours: [], tour_autostart: null });
assert.deepEqual(JSON.parse(wasm.dashboard_meta("SELECT 1;")), { title: "", nav: null, group: null, tags: [], refresh: 0, description: null, owner: null, certified: null, astables: false, tours: [], tour_autostart: null });
// -- @astables on (#401): the "Charts as tables" bar is opt-in.
assert.equal(JSON.parse(wasm.dashboard_meta("-- @astables on\nSELECT 1;")).astables, true);
assert.equal(JSON.parse(wasm.dashboard_meta("-- @astables off\nSELECT 1;")).astables, false);
// -- @nav: a short label distinct from -- @title (#355)
assert.equal(JSON.parse(wasm.dashboard_meta("-- @title Which products depend on this supplier?\n-- @nav Supplier risk\nSELECT 1;")).nav, "Supplier risk");
assert.equal(JSON.parse(wasm.dashboard_meta("-- @nav \nSELECT 1;")).nav, null, "an empty @nav is null, like @owner");
// owner and certification (a bad date is no certification)
const oc = JSON.parse(wasm.dashboard_meta("-- @owner  Finance team\n-- @certified 2026-10-15\nSELECT 1;"));
assert.deepEqual([oc.owner, oc.certified], ["Finance team", "2026-10-15"]);
assert.equal(JSON.parse(wasm.dashboard_meta("-- @certified 2026-02-30\nSELECT 1;")).certified, null);

// The delivery-routing maps: both ::MAP panels of the Map tab carry a
// ::CATEGORY (the van), a ::LABEL (the hover text), the same ::SYNC_ZOOM group
// and a ::COLORS level map, and the renderer keys each feature by its van
// (data-series) and row (data-row), over the street grid (::BASEMAP); the Stop list's route map is a ::STORY
// keyed by stop number (::XAXIS).
{
  const sql = readFileSync(new URL("../../web/examples/optimize-delivery.sql", import.meta.url), "utf8");
  const all = JSON.parse(wasm.plan(sql)).filter((s) => s.roles.some((r) => r[1] === "MAP"));
  const maps = all.filter((s) => s.roles.some((r) => r[1] === "SYNC_ZOOM"));
  assert.equal(maps.length, 2, "two maps on the Map tab");
  const story = all.filter((s) => !maps.includes(s));
  assert.equal(story.length, 1, "one route map on the Stop list tab");
  for (const r of ["XAXIS", "STORY", "COLORS"]) assert.ok(story[0].roles.some((x) => x[1] === r), `route map without ::${r}`);
  for (const s of maps) {
    for (const r of ["CATEGORY", "LABEL", "BASEMAP", "SYNC_ZOOM", "COLORS"]) assert.ok(s.roles.some((x) => x[1] === r), `map without ::${r}`);
    const z = s.roles.find((x) => x[1] === "SYNC_ZOOM")[0];
    assert.match(s.sql, new RegExp(`'routes' AS c${z}\\b`), "both maps zoom as 'routes'");
  }
  const rows = JSON.stringify([
    { c0: "LINESTRING(0 0, 1 2, 0 0)", c1: "Van 1", c2: "Van 1: 4 km, back 07:10, 1 stop late", c3: null, c4: "routes", c5: "Depot:neutral; Late stop:bad", c6: "Today" },
    { c0: "POINT(1 2)", c1: "Van 1", c2: "Backstube 3 — bakery\nVan 1, stop #1", c3: null, c4: "routes", c5: "Depot:neutral; Late stop:bad", c6: "Today" },
    { c0: "MULTIPOLYGON(((1 3.1, 2.1 2, 1.6 2, 1 2.6, 1 3.1)))", c1: "Late stop", c2: "Backstube 3 — bakery", c3: null, c4: "routes", c5: "Depot:neutral; Late stop:bad", c6: "Today" },
  ]);
  const svg = wasm.render_panel(rows, JSON.stringify(maps[0].roles), 460, 300, "", "");
  assert.match(svg, /^<svg[^>]* data-map=""/);
  // areas draw as <path>; an eligible map's lines now draw through
  // ggplot-rs's own geom_sf, as a plain <polyline> (not a <path>, the old
  // writer's choice) — app.js's hoverability gate already covers
  // <polyline> explicitly, so this is an accepted rendering delta, not a
  // contract break (same note as tests/maps.rs and
  // tests/regression_graphics.rs).
  assert.match(
    svg,
    /<(?:path|polyline) [^>]*fill="none"[^>]*data-row="0" data-series="Van 1" data-label="Van 1: 4 km, back 07:10, 1 stop late"/
  );
  // Fixed by sipemu/ggplot-rs#234 (#226, ggplot-rs 0.30.1): attribute
  // values now entity-escape `\t`/`\n`/`\r`, so `data-label` gets the old
  // writer's own `&#10;` instead of a literal newline — same fix
  // documented in tests/maps.rs.
  assert.match(svg, /<circle [^>]*data-row="1" data-series="Van 1" data-label="Backstube 3 — bakery&#10;Van 1, stop #1"/);
  assert.match(svg, /<path [^>]*data-row="2" data-series="Late stop"/);
}

// Maps: the zoom UI starts from map_bounds — the projected window the whole
// panel shows unzoomed, at the panel's aspect — and hands it back as the
// zoom, which draws the same view.
{
  const rows = JSON.stringify([
    { c0: "POLYGON((9 49, 10 49, 10 50, 9 50, 9 49))", c1: 3, c2: "Nord" },
    { c0: "POLYGON((10 49, 11 49, 11 50, 10 50, 10 49))", c1: 7, c2: "Ost" },
  ]);
  const roles = JSON.stringify([[0, "MAP"], [1, "BARCHART"], [2, "LABEL"]]);
  const b = JSON.parse(wasm.map_bounds(rows, roles, 460, 300));
  assert.equal(b.length, 4);
  assert.ok(Math.abs((b[1] - b[0]) / (b[3] - b[2]) - 460 / 300) < 1e-9, `aspect: ${b}`);
  const plain = wasm.render_panel(rows, roles, 460, 300, "", "");
  assert.match(plain, /^<svg[^>]* data-map="" data-proj="utm"/);
  const zoomed = wasm.render_panel(rows, roles, 460, 300, "", JSON.stringify(b));
  const paths = (s) => [...s.matchAll(/<path d="([^"]*)" fill="#[0-9a-f]{6}" data-row/g)].map((m) => m[1]);
  assert.deepEqual(paths(zoomed), paths(plain), "the start window draws the same view");
  assert.deepEqual(JSON.parse(wasm.map_bounds("[]", roles, 460, 300)), []);
}

// Built-in boundaries: a ::REGION panel names the layers it needs; once the
// UI registers a layer's CSV (fetched from web/data/boundaries/), the wasm
// draws from it — the pack itself is not in the wasm.
{
  const rows = JSON.stringify([
    { c0: "DE-BY", c1: 3 },
    { c0: "DE-BE", c1: 7 },
  ]);
  const roles = JSON.stringify([[0, "REGION"], [1, "BARCHART"]]);
  assert.deepEqual(JSON.parse(wasm.map_layers(rows, roles)), ["de-states"]);
  // M2b: ::ORIGIN/::DEST codes guess the layer the same way ::REGION does.
  const flowRoles = JSON.stringify([[0, "ORIGIN"], [1, "DEST"]]);
  assert.deepEqual(JSON.parse(wasm.map_layers(rows, flowRoles)), ["de-states"]);
  const before = wasm.render_panel(rows, roles, 460, 300, "", "");
  assert.ok(!/data-row/.test(before), "nothing to draw before the layer is registered");
  const csv = readFileSync(new URL("../../web/data/boundaries/de-states.csv", import.meta.url), "utf8");
  assert.equal(wasm.register_boundaries("de-states", csv), 16);
  assert.equal(wasm.register_boundaries("not-a-layer", csv), -1);
  assert.deepEqual(JSON.parse(wasm.map_layers(rows, roles)), []);
  const svg = wasm.render_panel(rows, roles, 460, 300, "", "");
  assert.match(svg, /<path [^>]*data-row="0" data-series="Bavaria"/);
  assert.match(svg, /dl-de\/by-2-0/);
  const bm = JSON.stringify([[0, "LAT"], [1, "LON"], [2, "BASEMAP"]]);
  assert.deepEqual(JSON.parse(wasm.map_layers(JSON.stringify([{ c0: 52.5, c1: 13.4, c2: "world" }]), bm)), ["world"]);
}

// M2a: a LAT/LON + ::SIZE panel draws graduated symbols with data-size /
// data-fsize (no Role::Value column, so render_panel writes data-fvalue too).
{
  const rows = JSON.stringify([
    { c0: 50.0, c1: 10.0, c2: 100 },
    { c0: 51.0, c1: 10.0, c2: 10 },
  ]);
  const roles = JSON.stringify([
    [0, "LAT"],
    [1, "LON"],
    [2, "SIZE"],
  ]);
  const svg = wasm.render_panel(rows, roles, 460, 300, "", "");
  assert.match(svg, /<circle [^>]*data-size="100"/);
  assert.match(svg, /<circle [^>]*data-fsize="[^"]*"/);
  assert.match(svg, /<circle [^>]*data-fvalue="[^"]*"/);
}

// M2b: a flow panel's arrowhead is a ggplot-rs mark-part, data-part="arrow"
// (#308 — a real geom_curve + arrow() layer, not hand-written SVG).
{
  const rows = JSON.stringify([
    { c0: "{\"lon\": 9.0, \"lat\": 50.0, \"name\": \"A\"}", c1: "{\"lon\": 10.0, \"lat\": 51.0, \"name\": \"B\"}" },
  ]);
  const roles = JSON.stringify([[0, "ORIGIN"], [1, "DEST"]]);
  const svg = wasm.render_panel(rows, roles, 460, 300, "", "");
  assert.match(svg, /data-part="arrow"/);
}

console.log(`examples_lib_test: ok (${EXAMPLES.length} examples in ${exampleAreas(EXAMPLES).length} areas)`);
