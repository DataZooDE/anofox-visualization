// Node test for the browser side of ::DIAGRAM (issue #43): metricRole leaves
// a diagram's KPI format alone, the a11y helpers (chart kind, marks without
// the hover-only edges, the reading-order walk) and the "Charts as tables"
// text alternative — against the SVG the wasm renderer draws. Skips the wasm
// part without web/pkg.
import { readFileSync, existsSync } from "fs";
import assert from "assert/strict";

const src = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8");
const grab = (name) => {
  const m = src.match(new RegExp(`^function ${name}\\([\\s\\S]*?^}`, "m"));
  assert.ok(m, `function ${name} not found in app.js`);
  return m[0];
};
const line = (re) => {
  const m = src.match(re);
  assert.ok(m, `${re} not found in app.js`);
  return m[0];
};

// A ::DIAGRAM statement's ::COMPACT is its nodes' value line, not a KPI tile.
const { metricRole } = new Function(
  `const METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];\n${line(/^const role = .*$/m)}\n${line(/^const metricRole = .*$/m)}\nreturn { metricRole };`
)();
assert.ok(!metricRole({ roles: [[0, "DIAGRAM"], [1, "COMPACT"]] }));
assert.deepEqual(metricRole({ roles: [[0, "COMPACT"], [1, "LABEL"]] }), [0, "COMPACT"]);

const kinds = line(/^const CHART_KINDS = \[[\s\S]*?^\];/m);
const { chartKind, chartMarks, markGrid, diagramTextHtml } = new Function(
  `${kinds}\n${grab("chartKind")}\n${grab("chartMarks")}\n${grab("markGrid")}\n${grab("escapeHtml")}\n${grab("diagramTextHtml")}\nreturn { chartKind, chartMarks, markGrid, diagramTextHtml };`
)();
assert.equal(chartKind([[0, "DIAGRAM", ""], [1, "LABEL", ""]]), "Diagram");

// Stub elements built from the SVG's own tags.
const attrsOf = (tag) => Object.fromEntries([...tag.matchAll(/ ([\w-]+)="([^"]*)"/g)].map((m) => [m[1], m[2].replace(/&gt;/g, ">").replace(/&amp;/g, "&")]));
const stub = (tagName, attrs) => ({
  tagName,
  attrs,
  dataset: Object.fromEntries(
    Object.entries(attrs)
      .filter(([k]) => k.startsWith("data-"))
      .map(([k, v]) => [k.slice(5).replace(/-(\w)/g, (_, c) => c.toUpperCase()), v])
  ),
  getAttribute: (k) => (k in attrs ? attrs[k] : null),
  hasAttribute: (k) => k in attrs,
});
const svgOf = (svg) => {
  const els = [...svg.matchAll(/<(rect|polyline)( [^>]*)>/g)].map((m) => stub(m[1], attrsOf(m[2])));
  // what attachHover leaves: titled marks become .dp-hit with a data-tip
  for (const e of els) if (e.attrs["data-node"] || e.attrs["data-edge"]) e.attrs["data-tip"] = "tip";
  const root = attrsOf(svg.slice(0, svg.indexOf(">")));
  return {
    hasAttribute: (k) => k in root,
    getAttribute: (k) => root[k] ?? null,
    querySelectorAll: (sel) =>
      sel === ".dp-hit"
        ? els.filter((e) => "data-tip" in e.attrs)
        : sel === "rect[data-node]"
          ? els.filter((e) => e.tagName === "rect" && e.attrs["data-node"])
          : sel === "polyline[data-edge]"
            ? els.filter((e) => e.tagName === "polyline" && e.attrs["data-edge"])
            : [],
  };
};

const pkg = new URL("../../web/pkg/", import.meta.url);
if (!existsSync(new URL("anofox_visualization_bg.wasm", pkg))) {
  console.log("web/pkg not built — skipping the wasm part of diagram_test");
  process.exit(0);
}
const wasm = await import(new URL("anofox_visualization.js", pkg));
wasm.initSync({ module: readFileSync(new URL("anofox_visualization_bg.wasm", pkg)) });

const rows = [
  { c0: "ingest", c1: "Ingest", c2: 10000, c3: "ok", c4: [] },
  { c0: "build", c1: "Build twin", c2: 9800, c3: "ok", c4: ["ingest", "calibrate"] },
  { c0: "monitor", c1: "Monitor drift", c2: 412, c3: "alarm", c4: ["build"] },
  { c0: "calibrate", c1: "Calibrate", c2: 37, c3: "warn", c4: ["monitor"] },
];
const roles = [[0, "DIAGRAM"], [1, "LABEL"], [2, "COMPACT"], [3, "BADGE"], [4, "UPSTREAM"]];
const out = wasm.render_panel(JSON.stringify(rows), JSON.stringify(roles), 800, 300, "", "");
assert.match(out, /^<svg[^>]* data-diagram="LR"/);
assert.match(out, /data-summary="4 steps in 4 stages, from Ingest; 4 connections; 1 loop \(Calibrate → Build twin\); largest Ingest \(10K\)"/);
const svg = svgOf(out);

// Edges are hover-only: no keyboard stop; the walk is the nodes in reading order.
assert.deepEqual(chartMarks(svg).map((e) => e.attrs["data-node"]), ["ingest", "build", "monitor", "calibrate"]);
assert.deepEqual(markGrid(svg)[0].map((e) => e.attrs["data-node"]), ["ingest", "build", "monitor", "calibrate"]);

// Charts as tables: the steps as an ordered list, the connections as a table.
const html = diagramTextHtml(svg, "Pipeline");
assert.match(html, /^<ol class="dp-diagram-steps" aria-label="Pipeline"><li>Ingest \(10K, ok\) → Build twin<\/li><li>Build twin \(9\.8K, ok\) → Monitor drift<\/li>/);
assert.match(html, /<li>Monitor drift \(412, alarm\) → Calibrate<\/li><li>Calibrate \(37, warn\) → Build twin<\/li><\/ol>/);
assert.match(html, /<table class="dp-dv" aria-label="Pipeline: connections">/);
assert.equal((html.match(/<tr><td>/g) || []).length, 4);
// A hostile label stays text.
const evil = wasm.render_panel(JSON.stringify([{ c0: "a", c1: "<img src=x onerror=alert(1)>" }]), JSON.stringify([[0, "DIAGRAM"], [1, "LABEL"]]), 400, 200, "", "");
assert.ok(!/<img/.test(diagramTextHtml(svgOf(evil), "x")));
console.log("diagram_test: ok");
