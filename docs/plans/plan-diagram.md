# Plan: `::DIAGRAM` — data-driven node/edge diagrams

Status: design note for owner review (2026-10-16), based on master `0947b4f`,
ggplot-rs 0.19. No code yet. Issue: #43. Use case: #65 (inventory twin,
"Pipeline" tab) and DataZooDE/inventory-rs#28 (`run_manifest` + `run_edges`).
Parent plan: [plan-anofox-visualization.md](plan-anofox-visualization.md).

**Goal.** A dashboard can draw a **process, pipeline or lineage diagram from
two small tables**: boxes for nodes, arrows for edges. It is laid out
automatically (left to right, layered) and drawn in the dashboard theme. It
uses the motion we already ship (`'flow'` connectors, `::STORY` steps,
`::ANIMATE`), and the browser, `dashboard --svg` and the HTML export all get
the same SVG. Clicking a node cross-filters the rest of the dashboard, like
clicking a bar.

**Scope recommendation.** Yes, but only **data-driven** diagrams: model
pipelines, data lineage, process stages with counts, and state machines.
**No** free drawing tool. Explainer content like the one in the video (weight
grids, decorative shapes) stays outside. Its useful parts are covered here:
boxes, flowing connectors, step-by-step highlights, small charts in boxes, and
pinned coordinates for a hand-placed layout.

**The three decisions this note proposes:**

1. **SQL shape.** A diagram is one statement with **one row per node**. The
   id column is cast `::DIAGRAM`. Edges come in one of two ways:
   - a `::UPSTREAM` list per node, for plain lineage;
   - a `::EDGES` list of structs, for edges that carry counts, labels or a style.

   This follows the existing "list column as JSON" convention. For extension
   output there is also a contract macro, `anofox_plot_diagram(n, edges := …)`.
   Both routes build the same internal `Diagram`.
2. **Layout.** A **small Sugiyama (layered) layout of our own** in Rust, about
   700 lines, deterministic, with no new dependency. Optional `::XAXIS` /
   `::YAXIS` columns pin a node's layer or its exact position. We do not
   depend on a layout crate: see §3.2 for the spike numbers.
3. **Rendering.** A **separate SVG path** (`Panel::Own`, like KPI tiles), not
   ggplot-rs. Mini-charts inside nodes are nested `render_with_at` fragments.
   The SVG reuses the existing mark contract (`<title>`, `data-x`,
   `data-series`, `class="dp-flow"`). Hover, selection, the a11y layer and
   `anim.js` then work with small, local extensions.

---

## 1. Current state (evidence)

| Aspect | Today | Evidence |
|---|---|---|
| Panel = one statement = one result set | Each query statement with role casts becomes one panel. A **contract call** is the only form that brings in a second table, through a named argument that holds a scalar subquery (`tests := (SELECT list(t) FROM …)`). | `src/sql.rs:43-100`; `src/sql/contract_call.rs:1-28`; `src/macros.rs:239-253` (`anofox_plot_compare`) |
| List / struct columns | A LIST or STRUCT cell reaches the renderer as its JSON text (`Value::Str`). This is how a KPI's `list(v)::SPARKLINE` and a contract call's arguments travel, in every host. | `src/sql.rs:196-201`; `src/kpi.rs:258` |
| Role registry | A single source of truth for tokens, aliases, category, `renders` and docs. A new role is a registry row plus a `Role` variant, and tests keep `DOCS.md` in sync. | `src/roles.rs:62-325` |
| Non-ggplot panels | `Panel::Own { body }` already exists: KPI tiles, notes and headings are hand-written SVG and go through the same `finish` → theme → post-passes. | `src/lib.rs:1002-1016, 1081-1098`; `src/kpi.rs:303` |
| Nested charts | `render_with_at` writes a positioned `<svg x y …>` fragment, which is how the headless dashboard composes panels. | `src/lib.rs:1197-1210`; `src/dashboard.rs:646` |
| Map precedent | A non-x/y panel is picked by its own column role (`::MAP`) before the measure dispatch. | `src/lib.rs:1705-1710` |
| ggplot-rs | It has `rect`, `segment`, `curve`, `path` and `text` geoms, but **no arrowheads, no rounded boxes and no coordinate-free layer**. Every layer goes through scales and a coord. | `ggplot-rs-0.19.0/src/geom/` |
| Flow connectors | `'flow'::LINESTYLE` adds `stroke-dasharray="6 4" class="dp-flow"` to data polylines, and CSS moves the dashes (paused under reduced motion, plain dashes in static output). | `src/lib.rs:1331-1408`; `web/theme-base.css:149-151` |
| Motion directives | `data-animate`, `data-story` (`[{k,t}]`), `data-story-by` and `data-transition` sit on the SVG root. `story_steps` keys steps by the row's x (or `::CATEGORY`). | `src/lib.rs:1415-1493` |
| Motion kit | `animKit` is driven only by SVG attributes. `marksOf` treats `rect[data-x]` as **bars** (it grows them from the axis). Story lights marks whose `data-x` / `data-series` matches (`sameX`). `emph` layers dimming. The HTML export embeds `animKit.toString()`. | `web/anim.js:38-46, 104, 212-245, 282-445`; `web/app.js:7057` |
| Hover / selection | Every `rect, circle, polygon, polyline` with a `<title>` becomes a `.dp-hit` mark (**not `path`**). `data-series` is the selection key used by click-select, `::SELECT_AS`, `::LINK` `{value}` and drill. | `web/app.js:7251-7290, 2645-2700, 2885` |
| A11y layer | A one-sentence `chartSummary` (kind, series, x range, extremes) goes into `aria-label` + `<desc>`. Arrow keys walk a series × x grid (`markGrid`). "Charts as tables" swaps a chart for `dataTableHtml(fig._dp)`. | `web/app.js:7347-7480, 7362-7380, 7575-7594` |
| Headless | `render_dashboard_svg` → `render_with_at` per panel. Contract calls go through `contract_call::render_rows_opts`. Theme: `ResolvedTheme` (palette, `colors`, `chart` tokens, font, radius) and `finish_svg` (dark neutral remap). | `src/dashboard.rs:105, 302, 646`; `src/theme.rs:195-214, 1019-1040` |
| Lint | Rule codes are namespaced (`story/bad-by`, `linestyle/unknown`, `design/*`, `render-warning`). | `src/lint.rs:49, 1021-1063` |
| wasm size | `web/pkg/anofox_visualization_bg.wasm` = 2.03 MB (build of 2026-10-16). | — |

Summary: everything **around** a diagram exists already: list columns, a
second table through a contract argument, own-SVG panels, nested charts, flow
CSS, story stepping, dimming, selection, a11y and export. What is missing is
(a) a graph layout, (b) a node/edge SVG writer, and (c) a few places where
JS assumes "x/y chart" (`marksOf` bars, `markGrid`, the hover selector without
`path`).

## 2. How comparable tools do it (and what we take)

| Tool | Mechanism | Take |
|---|---|---|
| Grafana Node Graph ([docs](https://grafana.com/docs/grafana/latest/panels-visualizations/visualizations/node-graph/)) | **Two data frames** with naming conventions. Nodes: `id, title, subtitle, mainstat, secondarystat, color, arc__*`. Edges: `id, source, target, mainstat, thickness, strokeDasharray`. Layered or force layout. | Closest analogue. Convention-named columns for the contract macro; `mainstat` ≈ our measure; edge `thickness` from a value. |
| Mermaid flowchart | Text DSL, dagre (or ELK) layout, `subgraph` clusters, `classDef` styles, `linkStyle` with animated dashes (v11). | LR/TB direction, clusters = our lanes, animated edges = `'flow'`. We stay table-driven, not DSL-driven. |
| Graphviz `dot` | The original Sugiyama pipeline: network-simplex ranks, median ordering, `rank=same`, `pos=` pins. | The four-phase algorithm; pins = our `::XAXIS/::YAXIS`. |
| dagre / dagre-d3, d3-dag | JS layered layout. Nodes have width/height; edges get routing points. | Node sizes from the label; edges are routed through dummy points. |
| ECharts `graph` series | `nodes[]` + `links[]` with `layout: 'none' \| 'force' \| 'circular'`; `'none'` uses given x/y. | Explicit coordinates as a first-class option. |
| Evidence / Superset Sankey | `source, target, value` rows. | The edges table carries `value` → edge width. (A Sankey is a separate, later chart.) |
| dbt docs / Airflow graph view | Lineage DAGs, layered LR, click a node → details. | The click-to-cross-filter interaction. |

## 3. Proposal

### 3.1 SQL shape

**Dashboard form (roles).** One row per node. The `::DIAGRAM` cast marks the
panel, just as `::MAP` does.

```sql
SELECT stage::DIAGRAM,                          -- node id (required; unique)
       title::LABEL,                            -- box text (default: the id)
       records_in::COMPACT,                     -- a measure line in the box (KPI formats)
       status::BADGE,                           -- a status pill ('ok', 'warn', 'alarm')
       upstream::UPSTREAM,                      -- VARCHAR[] of ids feeding this node
       lane::LANE,                              -- optional swimlane / cluster
       caption::STORY,                          -- per node: a story step
       'Inventory pipeline'::TITLE
FROM run_manifest ORDER BY seq;
```

Edges with attributes use `::EDGES`, a list of structs. It is usually a scalar
subquery, so the result stays one result set:

```sql
SELECT stage::DIAGRAM, title::LABEL, records_in::COMPACT,
       (SELECT list({'from': from_stage, 'to': to_stage, 'value': count,
                     'label': format('{:,}', count),
                     'style': CASE WHEN to_stage = 'calibrate' THEN 'flow' END}
                    ORDER BY from_stage, to_stage)
        FROM run_edges)::EDGES
FROM run_manifest ORDER BY seq;
```

The first non-NULL `::EDGES` cell wins. `::UPSTREAM` and `::EDGES` may be
combined, and their edges are merged by `(from, to)`.

| Column | Role | Meaning |
|---|---|---|
| node id | **`::DIAGRAM`** (alias `::NODE`) | Required, unique, string or number. Row order = default reading order and tie-break order. |
| `::LABEL` | existing | Box title. Wrapped to 2 lines and ellipsised; the full text goes in the tooltip. |
| `::METRIC/::MONEY/::PERCENT/::COMPACT` | existing | A value line in the box, formatted with the locale. This becomes `data-value` / `data-fvalue`. |
| `::BADGE` | existing | A status pill in the box corner. |
| `::THRESHOLDS` | existing | Colours the box border/fill by its value (same rules as the KPI). |
| `::CATEGORY` | existing | Node colour group (theme palette, the same stable colours as charts). |
| `::INFO` | existing | Extra tooltip text. |
| `::LINK` | existing | Click target (`'#Calibration'`, `?v.stage={value}`). |
| `::SPARKLINE` | existing | A `list(v)` column → a mini line in the box (M3). |
| `::STORY` | existing | Caption → one story step per node, in row order. |
| `::XAXIS` / `::YAXIS` | existing | Pinning (§3.2): `XAXIS` alone pins the layer; both pin exact grid coordinates. |
| **`::UPSTREAM`** (alias `::PARENTS`) | new | A single id or a list of ids → edges `upstream → this`. |
| **`::EDGES`** | new | `LIST(STRUCT(from, to [, value, label, style]))`. `style`: `'flow'`, `'dashed'` or `'solid'`. `value` → width (1–6 px, sqrt scale). |
| **`::LANE`** | new | A swimlane (LR: horizontal bands) / cluster box. Lanes keep their first-appearance order. |
| **`::DIRECTION`** | new, panel modifier | `'LR'` (default), `'TB'`. (If `::OPTS` lands first, this becomes `direction=` there.) |

New roles: `DIAGRAM`, `UPSTREAM`, `EDGES`, `LANE` and `DIRECTION`, each a
registry row. Existing modifiers keep their meaning:
- `::ANIMATE`: `'auto'` = nodes appear in layer order and edges draw in after
  their source.
- `::STORY_BY`: steps light a node (`'x'`, the default) or a lane (`'series'`
  = `::LANE`).
- `::SELECT_AS`, `::LINK` and `::EMPTY` work as on any chart.

Why not two statements (nodes, then an edges statement that "applies to the
next diagram", like `::THRESHOLDS` before a table)? Because headless, lint,
the `::EMPTY` / `@requires` handling and the per-panel re-run in the browser
all assume one statement = one panel. A scalar subquery keeps all of that
unchanged and costs the author one `list({…})`.

**Contract form (macro).** For extension output and plain DuckDB, without
dashboard SQL:

```sql
SELECT anofox_plot_diagram(m, edges := (SELECT list(e) FROM run_edges e),
                           id := 'stage', label := 'title', value := 'records_in',
                           source := 'from_stage', target := 'to_stage', edge_value := 'count')
FROM run_manifest m;
```

Default column names follow Grafana's node graph, so most callers pass no
mapping:
- nodes: `id, title, mainstat, status, group, lane, story, x, y`;
- edges: `source, target, value, label, style`.

We would add a `graph` schema (nodes + edges) to
[integration-contract.md](integration-contract.md). We would also ask
inventory-rs#28 to emit those names, or keep the `id :=` mapping for its
current `stage` / `from_stage` names. The macro maps rows onto the same role
columns (this is how `src/contract.rs` already works), so there is one
renderer.

### 3.2 Layout

**Modes.**
1. **Auto (default):** a layered layout, LR or TB.
2. **Pinned layer:** `::XAXIS` alone fixes a node's layer (an integer ≥ 0).
   Ordering within the layer stays automatic, so an author can force
   "all monitors in column 3".
3. **Explicit:** `::XAXIS` + `::YAXIS` on every node → grid coordinates (layer,
   slot) or free numbers, scaled to fit. Edges are still routed. This is the
   explainer case: a hand-placed figure that is still data-driven.

Mixed pins (some nodes with x/y, some without) are allowed in mode 2 only.
Mode 3 requires x/y on all nodes; lint flags partial pins.

**Algorithm (own, ~600–900 lines + tests).** The standard four phases. Every
phase uses `Vec` indexed by row order (no hash-map iteration), so the output
is a pure function of the rows:

1. *Cycle breaking:* DFS in row order, reverse the back edges. Back edges are
   drawn as loops along the lane edge. This is the "calibrate → build" loop in
   the inventory pipeline.
2. *Ranking:* longest path from sources, then one tightening pass (pull sinks
   toward their predecessors). Network simplex only if real dashboards need
   it. Pinned layers are constraints.
3. *Ordering:* dummy nodes on long edges; barycenter with 8 down/up sweeps and
   adjacent transposition; ties broken by row order; lanes keep their nodes
   contiguous (lane-major key).
4. *Coordinates & routing:*
   - Nodes are sized from their text (a text-width estimate for the theme font) and placed with a median-alignment pass (simplified
     Brandes–Köpf).
   - Edges become polylines through the dummy points, with rounded joins and an
     arrowhead.
   - Coordinates are rounded to 0.5 px.
   - The whole drawing is scaled to fit the panel. Below a minimum font size it
     scales down to 70%, then lint warns.

**Candidate crates (spike, 2026-10-16).** A scratch cdylib laid out the
7-stage inventory pipeline, including the calibrate→build back edge. It was
built with `opt-level="z"`, LTO and `panic=abort` for `wasm32-unknown-unknown`,
plus a native run 40× on the same graph and on a 40-node random DAG to check
determinism. The baseline cdylib is 15.7 KB.

| Crate | License | Last release / downloads | Deps | wasm32 build | Δ size raw / gzip | Deterministic (40 runs) | Fit |
|---|---|---|---|---|---|---|---|
| `rust-sugiyama` 0.4.0 | MIT | 2025-09 / 47k | petgraph, log | ✅ | +179 KB / +58 KB | ✅ | Node sizes yes; **top-down only** (LR = swap axes); no clusters/lanes, no pins, no routing points; uses `std` HashMap internally |
| `mermaid-dagre` 0.1.5 (dagre port) | MIT/Apache-2.0 | 2026-07 / 0.4k | ahash, indexmap, serde, serde_json | ❌ **fails**: `ahash` → `getrandom` 0.3 needs `--cfg getrandom_backend="wasm_js"` + a direct `getrandom` dep | +240 KB / +76 KB (after the workaround) | ✅ | Most complete: LR/TB, compound graphs (lanes), edge points. But a young port (395 downloads) and it needs a build-flag change for every wasm consumer |
| `layout-rs` 0.1.3 | MIT | 2025-04 / 808k | (log, optional) | ✅ | +102 KB / +40 KB | ✅ | A Graphviz-like engine with its own dot parser and SVG backend we would not use; the coordinates API goes through `VisualGraph` |
| **own** | BUSL (ours) | — | none | ✅ | est. +20–40 KB | by construction | Exactly our features: pins, lanes, row-order ties, theme-aware sizes |

Recommendation: **own implementation.** The crates either miss what we need
(pins, lanes, LR, routing) or add 40–80 KB gzipped plus a wasm build hazard,
and we would still write the pinning, lanes and routing around them. Graphs
here are small (≤ 150 nodes, see limits), so the simple heuristics are fast
enough: the 40-node spike graph lays out in well under the 30 ms a whole
native process run takes.
Fallback if the own layout disappoints: `rust-sugiyama` behind a cargo
feature (licence-compatible, builds for wasm).

**Limits.** 150 nodes / 300 edges per diagram (the `max_categories`
pattern). Above that the panel draws a note "Diagram has N nodes; showing the
first 150", and lint raises `diagram/too-large`.

### 3.3 Rendering

A new module `src/diagram.rs`:
- `Diagram::from_columns(cols)` reads the role columns (or the macro's mapped
  rows) and validates them.
- `layout(&Diagram, &Fonts, dir) -> Placed`.
- `body(&Placed, w, h, o) -> String` returns a `Panel::Own`.
- Dispatch sits in `render_inner` next to the map branch
  (`src/lib.rs:1705`).

SVG contract (what the browser keys on):

```xml
<svg … data-diagram="LR" data-story="[{k,t}…]" data-animate="auto">
  <g class="dp-lanes"><rect …/><text>Monitoring</text></g>            <!-- no <title>: not a mark -->
  <g class="dp-edges">
    <polyline points="…" data-edge="ingest>build" data-x="ingest" data-to="build"
              data-series="ingest" data-value="9800" data-fvalue="9.8K"
              stroke-width="3" marker-end="url(#dp-arrow)" class="dp-flow" stroke-dasharray="6 4">
      <title>Ingest → Build twin: 9.8K</title></polyline>
  </g>
  <g class="dp-nodes">
    <rect x y width height rx data-node="build" data-x="build" data-series="build"
          data-value="9800" data-fvalue="9.8K"><title>Build twin: 9.8K records</title></rect>
    <text pointer-events="none">Build twin</text> <text …>9.8K</text>
    <svg x y width height>…sparkline fragment…</svg>
  </g>
</svg>
```

Why this shape:
- Edges are **polylines**, not `path`, because the hover selector, `linesOf`,
  draw-in (`getTotalLength`) and the `dp-flow` CSS already handle polylines.
  Curved edges (`path`) would mean widening those selectors, so they are left
  for later.
- `data-series` = node id makes click-select, `::SELECT_AS` (`getvariable('stage')
  = 'calibrate'`), `::LINK {value}` and the "dim the rest" layer work unchanged.
  An edge's `data-series` is its source, so selecting a node keeps its outgoing
  edges lit.
- Colours come from `ResolvedTheme`:
  - node fill = `colors.card`, stroke = `chart.line`;
  - category accent = `palette[i]`;
  - text = `chart.ink` / `chart.muted`;
  - edges = `chart.muted`; flowing edges use `palette[0]`;
  - status = `positive` / `negative` / warning tokens.

  Dark mode and the `colorblind` / `high-contrast` themes then follow without a
  new token. `finish_svg` still runs.
- The arrowhead `<marker>` is defined once per SVG. Its id gets the panel's
  unique suffix so that several diagrams on one page do not collide.

Not ggplot-rs, because a diagram has no scales, axes or coord. Faking it with
`geom_rect` + `geom_segment` would mean adding arrowheads, rounded rects and
"no axes / no expansion" upstream for a single panel type. `Panel::Own` is the
same route KPI tiles take, and it needs no ggplot-rs release.

### 3.4 Motion: reuse of flow, story, animate

| Feature | Change |
|---|---|
| `'flow'` | Per edge (`style` in `::EDGES`), or `'flow'::LINESTYLE` on the panel = every edge. The writer sets `class="dp-flow"` itself; the CSS is unchanged; static output shows dashes. A **dot** variant (`'flow-dots'`: `stroke-dasharray="0 10"` + round caps) is a CSS-only addition, matching "dots travelling along connectors" in the video. |
| `::STORY` | `story_steps` keys by the `::DIAGRAM` column instead of `::XAXIS`. In `anim.js` `story()`, a lit node also lights its **incident edges** (`data-x` / `data-to` match), and the caption anchors to the node box (the `boxesOf` placement already works on rects). Open question 7: should only the edges *into* the node light (reads as "data arrives here")? |
| `::ANIMATE` | `enter()` gets a `data-diagram` branch: nodes fade or scale in by layer (`data-layer`), and edges draw in (`drawLine`) after their source. Without the branch, `marksOf` would treat node `rect[data-x]` as bars and "grow" them from an axis that does not exist. Reduced motion: no animation (as today). |
| `::TRANSITION 'tween'` | Out of scope in M1/M2. `keyed()` would match nodes by `data-x`, but moving rects plus re-routing edges is a separate job. A re-run swaps the diagram. |
| HTML export | Everything lives in `animKit` and in attributes, so the export replays it with no extra work (`web/app.js:7057`). |

### 3.5 Accessibility

- **Summary** (`chartSummary` has a diagram branch; `CHART_KINDS` gets
  `/^DIAGRAM/ → "Diagram"`): "Diagram: Inventory pipeline; 7 steps from
  Ingest to Recommend in 5 stages; 8 connections; 1 loop (Calibrate → Build
  twin); largest Ingest 10K." This comes from the node and edge marks
  (`data-node`, `data-edge`), not from the x/y heuristics.
- **Keyboard walk** (one Tab stop, as for charts). Arrow keys follow the
  **graph**, not the grid:
  - →: first outgoing neighbour; ←: first incoming;
  - ↑ / ↓: previous / next node in the same layer;
  - Home / End: first source / last sink;
  - Enter: select / link; Esc: leave.

  Focus shows the tooltip and an outline. `markGrid` is bypassed when the root
  has `data-diagram`.
- **Text alternative.** "Charts as tables" shows an **ordered list in
  layer/reading order** instead of the generic data table: "1. Ingest — 10K
  records → Build twin, Replay", one line per node with its outgoing edges and
  values. It also gets a "Connections" table (from, to, value, label). The
  headless SVG carries the same list as `<desc>` text, so exported HTML and
  `dashboard --svg` output are not image-only.
- Status is never colour-only: the badge text is in the box and in the
  tooltip. Every text colour and fill passes the theme contrast check (T7 in
  plan-theming-layout).

### 3.6 Headless, export, themes, lint

- **Parity:**
  - `dashboard --svg`, `serve`, `anofox_render` and the HTML export all call the
    same `render_inner` → `diagram::body`.
  - The CLI path needs nothing new beyond the dispatch.
  - The contract macro is a new `PLOTS` entry plus a `MacroSpec`, rewritten in
    the browser through `contract_call` like `anofox_plot_compare`.
  - Snapshot tests are added to the existing SVG tests (roxmltree
    well-formedness + XSS checks on labels).
- **Panel height:** a diagram has a natural size. The default panel height is
  used, and the drawing is scaled to fit (`viewBox`). Lint suggests `::HEIGHT`
  when the scale drops below 70%.
- **Lint rules (`diagram/*`):**
  - `unknown-node`: an edge to an id that is not a node; the edge is dropped.
  - `duplicate-node`: the first row wins.
  - `cycle` (info): the edges drawn as back edges.
  - `too-large`.
  - `partial-pins`: x without y, or pins on some nodes in explicit mode.
  - `bad-edges`: `::EDGES` that is not a list of structs with from/to.
  - `story-unknown` (reuses the story checks).
  - `crowded`: the scale is below 70%.

  `--check --strict` treats the warnings as errors, as it does today.
- **Docs and skills:** registry rows feed `DOCS.md` automatically. We add a
  `DIAGRAM` section with the pipeline example, update the `build-dashboard`
  skill vocabulary, and add a builder sample "How data moves through the
  pipeline" (a plain question name, per the example-naming rule).

## 4. Roadmap and size

| Milestone | Scope | Size | Exit criteria |
|---|---|---|---|
| **M1: static diagram** | Roles `DIAGRAM/UPSTREAM/EDGES/DIRECTION`; `src/diagram.rs` (model, own layered layout LR/TB, polyline routing, back edges, own SVG, theme tokens, labels, KPI-format values, badges, thresholds); dispatch; headless parity; lint `diagram/*`; a11y summary + ordered-list text alternative; DOCS | **M–L** (≈1.5–2 wk; ~1,500 Rust lines incl. tests, ~150 JS) | Inventory pipeline fixture identical in browser and `dashboard --svg`, light + dark; 40-node layout snapshot stable; wasm growth ≤ 50 KB raw |
| **M2: interaction & motion** | Hover/select/`::SELECT_AS`/`::LINK` on nodes; keyboard graph walk; `'flow'` / `'flow-dots'` edges; `::STORY` on nodes + incident edges; `::ANIMATE` diagram entrance; export replay | **M** (≈1 wk, mostly `anim.js` / `app.js`) | e2e: click node → dependent panel re-runs; story steps lit correctly; reduced-motion run shows final state; axe clean |
| **M3: richer nodes & pins** | `::LANE` (bands/clusters), pinned layers and explicit coordinates (`::XAXIS`/`::YAXIS`), `::SPARKLINE` mini-chart in nodes, edge width by value, `anofox_plot_diagram` macro + `graph` contract schema | **M** (≈1–1.5 wk) | Explainer-style fixture with explicit coordinates; macro works in both extension builds and via the browser rewrite |
| **M4: showcase** | "Pipeline" tab of #65 from `run_manifest` / `run_edges` (needs inventory-rs#28) | **S** | `dashboard --check --strict` green; showcase e2e covers it |

Total ≈ L, in line with the issue's estimate. M1 alone already replaces the
"bar chart of counts per stage" fallback in #65.

## 5. Risks

| Risk | Mitigation |
|---|---|
| Layout quality (crossings, odd spacing) on real graphs falls short of dagre and erodes trust. | Snapshot fixtures from real pipelines and lineage; median + transpose sweeps; pins as an escape hatch; `rust-sugiyama` as a feature-gated fallback. |
| Scope creep toward a drawing tool (shapes, free text, icons, layout editor). | This note's scope line: boxes, edges, lanes, mini-charts, pins. Anything else needs its own issue. |
| JS assumes x/y charts in several places (`marksOf`, `markGrid`, `chartSummary`, tween), so a regression can hit other charts. | All branches keyed on `data-diagram`; the existing e2e suites (`rules-e2e`, showcase e2e) run unchanged. |
| Long labels / many nodes do not fit a 12-col panel on a phone (#67). | Wrap + ellipsis, scale-to-fit with a floor, TB direction recommended under 480 px (author choice; no automatic flip in M1). |
| `::LABEL` already means "title" in charts; here it is per node. | Same as maps (per-feature label). Panel title via `::TITLE` (lint hint if a single-row diagram uses `::LABEL`). |
| The scalar-subquery `::EDGES` reads awkwardly. | `::UPSTREAM` covers plain lineage without it; the macro covers extension tables; the docs show both patterns. |
| Determinism across hosts (f64 differences native vs wasm). | Integer layer/slot math, rounding to 0.5 px, no hash iteration; the native snapshot is compared against the browser build in e2e. |

## 6. Decisions for the owner

1. **Scope:** data-driven diagrams (pipelines, lineage, processes) are in;
   free-form explainer drawing is out. Agree?
2. **SQL surface:** one-row-per-node statement with `::DIAGRAM` +
   `::UPSTREAM` / `::EDGES` (scalar subquery), rather than a separate edges
   statement. Agree?
3. **Role names:** `::DIAGRAM` (alias `::NODE`), `::UPSTREAM`, `::EDGES`,
   `::LANE`, `::DIRECTION`. Or prefer e.g. `::FLOWCHART` / `::PARENT`?
4. **Layout engine:** our own small Sugiyama (recommended) vs. depending on
   `rust-sugiyama` (+58 KB gz, top-down only)? `mermaid-dagre` is ruled out
   for now because it does not build for `wasm32-unknown-unknown` without
   build-flag changes.
5. **Rendering:** own SVG path (`Panel::Own`), no ggplot-rs upstream work.
   Agree?
6. **Contract macro + schema:** add a `graph` schema (Grafana-style names
   `id/title/mainstat`, `source/target/value`) to the integration contract and
   ask inventory-rs#28 to emit it? Or keep inventory-rs's names and map with
   `id :=` / `source :=`?
7. **Story semantics:** a step lights a node plus **all incident edges**, or
   only **incoming** edges?
8. **Explicit coordinates and lanes:** are M3 timing (after the first
   showcase) and pins via `::XAXIS`/`::YAXIS` acceptable?
9. **Mini-charts in nodes:** sparkline only (M3), or also tiny bars/heatmaps
   (the video's "weights grid")?
10. **Limits:** 150 nodes / 300 edges per diagram, with a note above that.
    OK?
11. **Edges as marks:** should edges be clickable / selectable (select their
    source), or hover-only?

## 7. Decisions (2026-10-16)

The owner accepted the proposals; M1 starts.

1. **Scope:** yes — data-driven diagrams in, free-form explainer drawing out.
2. **SQL surface:** yes — one row per node, `::UPSTREAM` / `::EDGES`.
3. **Role names:** as listed — `::DIAGRAM` (alias `::NODE`), `::UPSTREAM`,
   `::EDGES`, `::LANE`, `::DIRECTION`.
4. **Layout engine:** our own small Sugiyama, no layout crate.
5. **Rendering:** our own SVG writer, no ggplot-rs work.
6. **Contract:** a `graph` contract schema with Grafana-style names
   (`id/title/mainstat`, `source/target/value`), plus aliases for
   inventory-rs's current names (`stage`, `from_stage`, …) via the alias table
   from #71 (`src/contract/names.rs`).
7. **Story:** a step lights **all edges touching the node**, both directions.
8. **Lanes / pins:** in M3.
9. **Mini-charts:** sparkline only (M3).
10. **Limits:** 150 nodes / 300 edges, with a note above that.
11. **Edges:** hover-only in M1; whether they become clickable is decided in M2.
