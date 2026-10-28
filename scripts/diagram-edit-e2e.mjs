// 'edit'::DIAGRAM_MODE (#682) in a real browser: the toolbar adds a node of
// a palette type, Connect draws an edge (and refuses one the palette's
// connects_to forbids, saying why), a picked node is renamed / removed, Reset
// goes back to the data; the drawn diagram keeps each existing node's value;
// the edited graph is ONE JSON variable the other panels read with SQL; the
// URL carries it; no page errors.
//
//   scripts/build-wasm-pkg.sh
//   python3 serve_nocache.py 8947 &
//   NODE_PATH=<dir with playwright>/node_modules node scripts/diagram-edit-e2e.mjs \
//       [--url http://127.0.0.1:8947/] [--shots <dir>]
import { createRequire } from "module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const base = arg("--url", "http://127.0.0.1:8947/");
const shots = arg("--shots", "");
const fail = (m) => {
  console.error("FAIL: " + m);
  process.exitCode = 1;
};
const check = (ok, m) => (ok ? console.log("ok   " + m) : fail(m));
const b64 = (s) => Buffer.from(s, "utf8").toString("base64");

const SQL = `-- @title Diagram edit e2e (#682)
CREATE OR REPLACE TEMP TABLE network_node AS SELECT * FROM (VALUES
 ('sup', 'Supplier', 'supplier', NULL, 12.0),
 ('wh', 'Warehouse', 'stock', ['sup'], 50.0),
 ('store', 'Store', 'demand', ['wh'], 7.0)) v(id, name, kind, upstream, stock);
SELECT type ::DIAGRAM_PALETTE, label, connects_to FROM (VALUES
 ('supplier', 'Supplier', ['stock']), ('stock', 'Stocking point', ['stock', 'demand']), ('demand', 'Demand', [])) t(type, label, connects_to);
SELECT id ::DIAGRAM, name ::LABEL, kind ::CATEGORY, upstream ::UPSTREAM, stock ::METRIC, 'edit' AS graph ::DIAGRAM_MODE, 'Network' ::TITLE FROM network_node;
SELECT n->>'id' AS id ::TABLE, n->>'type' AS type, n->>'label' AS label FROM (SELECT unnest((getvariable('graph')->'nodes')::JSON[]) AS n);
SELECT count(*) AS edges ::METRIC, 'Edges' ::LABEL FROM (SELECT unnest((getvariable('graph')->'edges')::JSON[]));
`;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
const errs = [];
page.on("pageerror", (e) => errs.push(String(e)));
const ready = () =>
  page.waitForFunction(
    () => /\d+ panels?/.test(document.getElementById("status")?.textContent || "") && !document.body.classList.contains("loading"),
    null,
    { timeout: 120000 }
  );
const settle = async () => {
  await page.waitForTimeout(600);
  await ready();
};
const st = () =>
  page.evaluate(() => ({
    nodes: [...document.querySelectorAll(".dash rect[data-node]")].map((r) => r.getAttribute("data-node")).sort(),
    edges: [...document.querySelectorAll(".dash polyline[data-edge]")].map((r) => r.getAttribute("data-edge")).sort(),
    labels: [...document.querySelectorAll(".dash rect[data-node]")].map((r) => r.getAttribute("data-xtext")),
    status: document.querySelector(".dp-edit-status")?.textContent || "",
    kpi: document.querySelector(".metric-value")?.textContent,
    rows: [...document.querySelectorAll(".dash table tbody tr")].map((tr) => tr.querySelector("td")?.textContent),
    url: decodeURIComponent(location.search),
    reset: document.querySelector(".dp-edit-bar button:last-of-type")?.disabled,
  }));
const bar = (text) => page.click(`.dp-edit-bar >> text=${text}`);
const node = (id) => page.click(`.dash rect[data-node="${id}"]`, { force: true });

await page.goto(`${base}?mode=view#sql=${b64(SQL)}`);
await ready();
let s = await st();
check(s.nodes.join(",") === "store,sup,wh" && s.edges.join(",") === "sup>wh,wh>store", `the data's graph (${s.nodes}; ${s.edges})`);
check(s.kpi === "2" && s.rows.length === 3, `the graph variable reaches SQL (${s.kpi} edges, ${s.rows.length} rows)`);

await page.selectOption(".dp-edit-type", "supplier");
await page.fill(".dp-edit-name", "Supplier 2");
await bar("Add");
await settle();
s = await st();
check(s.nodes.includes("supplier_2") && s.rows.includes("supplier_2"), `Add: a new node, in the diagram and the SQL (${s.nodes})`);
check(/1 node\(s\) added/.test(s.status), `the change is listed (${s.status})`);

await bar("Connect");
await settle();
await node("supplier_2");
await settle();
await node("store");
await settle();
s = await st();
check(!s.edges.includes("supplier_2>store") && /can only feed Stocking point/.test(s.status), `the palette refuses supplier → demand, saying why (${s.status})`);
await node("supplier_2");
await settle();
await node("wh");
await settle();
s = await st();
check(s.edges.includes("supplier_2>wh") && s.kpi === "3", `Connect: supplier_2 → wh (${s.edges}; ${s.kpi} edges)`);
await bar("Cancel connect");
await settle();

await node("wh");
await settle();
await page.fill(".dp-edit-name", "DC North");
await bar("Rename");
await settle();
s = await st();
check(s.labels.some((l) => /DC North/.test(l || "")), `Rename: the picked node's label (${s.labels})`);
check(await page.evaluate(() => /50/.test(document.querySelector('.dash rect[data-node="wh"]')?.parentElement?.textContent || "")), "an existing node keeps its row's value");

await node("store");
await settle();
await bar("Remove");
await settle();
s = await st();
check(!s.nodes.includes("store") && !s.edges.includes("wh>store"), `Remove: the node and its edges (${s.nodes}; ${s.edges})`);
check(/v\.graph=\{"nodes":/.test(s.url), "the URL carries the edited graph");
if (shots) await page.screenshot({ path: `${shots}/diagram-edit.png`, fullPage: true });

await bar("Reset");
await settle();
s = await st();
check(s.nodes.join(",") === "store,sup,wh" && s.kpi === "2", `Reset: back to the data (${s.nodes}; ${s.kpi})`);
check(!errs.length, "no page errors: " + errs.join(" | "));
await browser.close();
console.log(`diagram-edit-e2e: ${process.exitCode ? "failed" : "0 failed"}`);
