# Plan: maps — themed, projected and classed, with built-in boundaries

Status: **approved by the owner (2026-10-17, "go on")**; decisions in §14, based on master `ba2eb9d`,
ggplot-rs 0.19.0. No code yet. Issue: #149. Interactivity (click-to-highlight,
cross-filter, `::LINK`, keyboard, `::SYNC_ZOOM`) is #147 and is **out of
scope**; this note builds on the attributes #147 adds.
Parent plan: [plan-anofox-visualization.md](plan-anofox-visualization.md).

**Goal.** Maps look as good as the rest of a dashboard and answer more
location questions: *where* it happens, *how much*, *how it moves* and *how it
changes over time*. Most business data has a region code or a lat/lon pair,
not WKT, so a map has to work from codes and coordinates, offline and on the
locked server, with no outside requests.

**The decisions this note proposes:**

1. **Rendering split.** Maps get **their own SVG writer** (`src/map/`,
   `Panel::Own`, like KPI tiles and `::DIAGRAM`). From ggplot-rs we keep only the
   public WKT parser (`ggplot_rs::spatial::parse_wkt`). Projections, class
   breaks, legends, labels, symbols, arcs and hexbins all live in our crate.
   There is no ggplot-rs release on the critical path (§3.1).
2. **Projections in our crate**: `'auto'` (default), `'equal-earth'`,
   `'mercator'`, `'albers-europe'`, `'conic'`, `'utm'` and `'none'`, about 350
   lines. `'auto'` picks from the data extent (§3.2).
3. **Class breaks in the renderer**, not in SQL: `::CLASSES 'quantile 5' |
   'jenks 5' | 'equal 5' | '0,10,50,100'`. The renderer draws a stepped legend
   with a count per class (§3.3).
4. **Built-in boundaries: a licence-clean pack.** It holds Natural Earth 1:110m
   countries (public domain) plus the German states and districts from BKG
   VG2500 (dl-de/by-2-0, commercial use allowed). **EU NUTS is not bundled**:
   GISCO's terms forbid commercial use (§4.2). The pack is 761 KB raw,
   224 KB gzip and 134 KB brotli. It is lazy-loaded per layer from
   `web/data/boundaries/` in the browser and embedded in native builds. It is
   **not** in the wasm (§4).
5. **Codes and coordinates are joined in the renderer.** `'DE-BY'::REGION`,
   `'09162'::REGION` and `'DEU'::REGION` look up the geometry in the pack;
   `::LAT` / `::LON` draw points. No spatial extension and no SQL file access
   are needed, so this works on the locked server as it is (§3.4).
6. **Map types reuse what exists**: `::SIZE` for graduated symbols,
   `::ORIGIN` / `::DEST` for flows (with `'flow'::LINESTYLE` motion),
   `::HEXBIN` for density, `::BIVARIATE` for a 3×3 choropleth, `::FACET` for
   small multiples. One new `::FRAME` role adds a client-side Play over periods
   (§5).
7. **No tiles by default; no canvas.** An opt-in raster tile basemap can be set
   per installation (browser only, never in exports). Big point sets are
   aggregated (`::HEXBIN`, or the h3 extension in SQL) rather than drawn on a
   canvas (§6, §7).

---

## 1. Current state (evidence)

| Aspect | Today | Evidence |
|---|---|---|
| Dispatch | Any `Role::Geometry` column makes a map panel, ahead of the measure dispatch. Without WKT text it shows "No data". | `src/lib.rs:1935-1941` |
| Renderer | `render_map`: the WKT goes to ggplot-rs `geom_sf`, fill = measure (else `::CATEGORY`), `::LABEL` = tooltip, then `coord_sf()` or `coord_cartesian_zoom`, then `house_void`. | `src/lib.rs:4735-4827` |
| Colours | A **fixed grey** `::BASEMAP` (`fill (228,230,233)`, stroke `(198,201,206)`) and fixed `(90,98,112)` borders. **Viridis** whenever a basemap is present, else `seq_low → seq_high`. No diverging ramp, no classes. | `src/lib.rs:4795-4826`; theme ramps `src/theme.rs:905-940` |
| Roles | `MAP` (aliases `GEOMETRY`, `GEO`, `CHOROPLETH`), `BASEMAP` (`MAPBASE`, `BACKDROP`), `ALPHA`. `SIZE`, `LABEL_TOP`, `RANK`, `DATALABELS`, `FACET`, `RANGE`, `ANIMATE`, `TRANSITION`, `LINESTYLE` and `SPARKLINE` exist for other charts. | `src/roles.rs:62-330` |
| ggplot-rs `geom_sf` | `GeomSf {fill, color, alpha, line_width, point_size, projection}`. **No `size` aesthetic** (every point is drawn at `point_size`). Points are coloured by fill. **Holes are stroked, not cut out**. WKT is parsed twice per feature. `label` only feeds the tooltip. | `ggplot-rs-0.19.0/src/geom/sf.rs:108-305` |
| ggplot-rs projections | Only `SfProjection::{PlateCarree, Mercator}`, applied inside the geom. Other layers (text, points) are **not** projected. `coord_sf` only fixes the aspect, and `GGPlot.coord` is `pub(crate)`, so a projecting coord **cannot be plugged in from outside**. | `spatial/mod.rs:57-79`; `coord/sf.rs:7-55`; `plot.rs:99` |
| ggplot-rs scales/labels | `scale_*_steps` bins are **equal-width only**, with fixed `[a.aa, b.bb)` labels. `scale_fill_gradient2` has a midpoint. `scale_size` is linear in radius (no area/sqrt). Text has **no halo**. `geom_text_repel`, `geom_curve` (no arrowheads) and `geom_hex` (hexes sized in pixels) exist. | `scale/steps.rs:65-80`; `scale/size.rs:14-26`; `render/backend.rs:127-136`; `geom/curve.rs`, `geom/hex.rs:66-71` |
| #147 (open) | Adds `data-row`, `data-series`, `data-value` and `data-label` per feature, plus `data-map` on the root. They are smuggled through `geom_sf`'s label aesthetic as a private-use marker and moved by `Panel::finish` (`src/mapmarks.rs`, 165 lines). Also adds `SYNC_ZOOM` (`ZOOM_GROUP`), click/Esc/keyboard and `mapMarkInfo`. Known limit: features are drawn in **legend order, not row order**. | `gh pr diff 147` |
| Zoom (master) | `attachMapZoom` (wheel 0.85×, drag pans, double-click resets) keeps a lon/lat window seeded from wasm `map_bounds` and **re-renders** through `renderChart(…, zoom)`. There is no viewBox transform. | `web/app.js:11282-11391, 7226, 7244`; `src/wasm.rs:184-200` |
| Motion | `'tween'::TRANSITION` matches old and new marks by `tag\|data-x\|data-series`, on re-run only. Map features have no `data-x`. The only Play is the `::STORY` stepper; **there is no frame player**. `::PERIOD` is a date-range input (`p_from`, `p_to`, …), not a scrubber. | `web/anim.js:352, 508, 524-660`; `src/period.rs:1-25` |
| A11y | `chartKind` → "Map". `chartSummary` gives the generic series/value sentence. "Charts as tables" **skips `LABEL`** and shows the **raw WKT** column. | `web/app.js:11586-11630, 11859, 12071` |
| Examples | The gallery Map tab fetches Natural Earth 110m from **cdn.jsdelivr.net** and the USGS feed at runtime (`ensureGeo`), loaded through DuckDB `spatial`. `optimize-delivery` draws **planar km** routes (`POINT(x_km y_km)`, ~40 KB CSV). The supply-chain example has no map. **No geodata files are in the repo.** | `web/app.js:284-345, 1262-1287, 1408`; `web/examples/optimize-delivery.sql:88-102` |
| Data files | `ensureDataFiles` fetches `'data/<dir>/<file>.csv\|parquet'` same-origin and registers it with DuckDB-Wasm. The extension's `serve` embeds `web/` (`include_dir!`), so `web/data` is served by the locked server too. | `web/app.js:1364-1385`; `duckext/src/serve.rs:47,59` |
| CSP | **No server sets a CSP** today (only the example auth proxy, `examples/served/auth-proxy.mjs:60`). The locked server runs `enable_external_access=false`. | `docs/secure-serving.md:92, 353` |
| Export | `dashboard --svg` uses the same `render_map`. The HTML export clones the live DOM (a map is saved at its current zoom). **No canvas anywhere.** | `src/bin/dashboard.rs:277-310`; `web/app.js:11173-11254` |
| wasm size | `web/pkg/anofox_visualization_bg.wasm` = 2.34 MB raw, 885 KB gzip (build of 2026-10-17). | — |

**Spike (native debug build, `dashboard --svg`, 1200 px wide).**

| Map | Features / polygons | Points in SVG | SVG raw / gzip | Time |
|---|---|---|---|---|
| DE districts, simplified pack (s3) | 400 / 467 | 31,885 | 517 KB / 141 KB | 0.14 s |
| DE districts, unsimplified (r4) | 400 / 465 | — | 914 KB / 267 KB | 0.41 s |
| World, Natural Earth 1:50m (s3) | 242 / 1,632 | 85,845 | 1.38 MB / 401 KB | 0.43 s |

The same SVGs **simplified in pixel space** (Douglas-Peucker, 0.5 px) and
written as relative `<path>` data with 1 decimal place:
- districts: 31,885 → 10,873 points, about **128 KB** of path data;
- world 1:50m: 85,845 → 8,119 points, about **176 KB**.

`geom_sf` writes 2-decimal absolute `points=` for every input vertex. Owning
the writer cuts the SVG **4–8×**.

## 2. How comparable tools do it (and what we take)

| Tool | Mechanism | Take |
|---|---|---|
| Datawrapper | Built-in basemaps joined by ISO/NUTS/name codes; classes (quantile, natural breaks, equal, custom) with a stepped legend; projection per basemap; symbol maps with a size legend; no tiles. | The whole M1 model: code join + classes + stepped legend + offline basemaps. |
| Tableau | Geocoding roles (country, state, postcode) from a built-in database; filled maps vs. symbol maps; a tile/vector basemap from a server. | Codes as the main input; tiles opt-in only. |
| Power BI (Shape map / Azure Maps) | Shape map uses TopoJSON keyed by region; Azure Maps adds bubble, heat and flow layers on tiles. | Bubble = graduated symbols; heat = hexbin. |
| Observable Plot / d3-geo | `geoEqualEarth`, `geoAlbers`, `geoConicEqualArea`, `geoMercator`; `fitExtent` to the data; `geoCentroid` for labels; hexbin transform in screen space. | Projection list, fit-to-extent and hexbins in projected space. |
| kepler.gl / deck.gl | Arc layer (curved OD flows, width by volume), hexagon/H3 layer, time playback with a slider. | Flow arcs; a frame player keyed by feature. |
| ggplot2 + sf | `coord_sf(crs=)` projects every layer; `scale_fill_binned`; `geom_sf_label`. | The grammar; we can't use it as-is (§1). |

## 3. Proposal: look and boundaries (M1)

### 3.1 Rendering split

A new module `src/map/` (est. `mod.rs` model + dispatch, `project.rs`,
`classes.rs`, `labels.rs`, `legend.rs`, `boundaries.rs`, `write.rs`) builds a
`Panel::Own { body }`, as `kpi.rs` and the diagram writer do. That panel then
goes through `finish` → theme → `finish_svg` (dark remap) like every other.

Pipeline: rows → features (WKT via `parse_wkt`, a pack lookup by code, or
lat/lon) → project → fit to the panel → **simplify in pixel space** (0.5 px) →
class and colour → write `<path fill-rule="evenodd">` (holes cut out) or
`<circle>` → labels → legend → furniture.

Why not keep `geom_sf` and patch upstream? It would take six or more upstream
changes, one each for:
- projections that cover every layer, which needs a public `coord_with`;
- a `size` aesthetic;
- cutting holes;
- text halos;
- manual or quantile bins with custom labels;
- data attributes without #147's private-use marker hack.

Even then, a map has no axes, no expansion and no grid; it would use the
grammar only for its fill scale. It is the same reasoning as for `::DIAGRAM`
(plan-diagram §3.3).

**Cost of owning it:** about 1,000 lines for M1's writer, legends and labels.
The upstream path costs about 100–150 lines for extra `SfProjection` variants,
plus 200–300 lines for a projecting coord, plus the other gaps, and then a
release in lockstep.

**Interaction contract (unchanged from #147).** Every feature carries:
- `data-row`, `data-series` (`::CATEGORY`, else the region code or `::LABEL`),
  `data-value` and `data-label`;
- a `<title>`;
- `data-map` on the root.

New attributes:
- `data-x`: the feature key (region code or row key), so `'tween'` can match
  features across re-runs and frames;
- `data-class`: the class index;
- `data-proj`: the projection, on the root.

The writer emits features in **row order**, which fixes #147's
legend-order limitation. `src/mapmarks.rs` becomes unnecessary once M1 lands.
Order: #147 merges first; M1 then swaps the inside of `render_map` and keeps
its JS and tests green.

### 3.2 Projections

| `::PROJECTION` | Use | Implementation |
|---|---|---|
| `'auto'` (default) | see rule below | — |
| `'equal-earth'` | world maps (equal area, pleasant shape) | closed-form forward (Šavrič et al. 2018), ~25 lines |
| `'mercator'` | tiles, small areas; **lint hint on a choropleth** (area distortion) | ~10 lines, latitude clamped at ±85.05° |
| `'albers-europe'` | Europe (fixed λ₀ = 10°, φ₁ = 43°, φ₂ = 62°) | Albers equal-area conic, ~35 lines |
| `'conic'` | a country or region: Albers equal-area, λ₀ = centre, φ₁/φ₂ at 1/6 and 5/6 of the latitude extent | shares the Albers code |
| `'utm'` | a city or district (≤ 6° wide); zone from the centre longitude | transverse Mercator (Krüger series), ~70 lines |
| `'none'` | planar data (km grids, floor plans: `optimize-delivery`) | identity, y up |

**`'auto'` rule.** In order:
1. Any coordinate outside lon ±180 / lat ±90 → `'none'`. This keeps
   `optimize-delivery`'s km grid working.
2. Longitude span > 120° → `'equal-earth'`.
3. Span ≤ 6° → `'utm'`.
4. Otherwise → `'conic'`.

Equal-area wins by default because choropleths compare areas.

Other details:
- **Fit:** the projected bounding box goes into the panel with a 4% margin.
- **Densify:** straight edges are densified to 1° steps before a conic
  projection, so long straight boundaries curve properly.
- **No inverse needed.** The scale bar uses the projected distance between two
  points 1 km apart at the map centre, and zoom works in projected units (below).

**Zoom.** `map_bounds` returns the **projected** box, and the zoom window is in
projected units. JS treats the units as opaque (centre, width), so
`attachMapZoom` and #147's `sharedZoom` keep working unchanged. Lint
`map/sync-projection`: maps in one `::SYNC_ZOOM` group must resolve to the
same projection. Each zoom re-renders, and the pixel-space simplification
then acts as level-of-detail for free: zooming in shows more vertices, up to
the pack's resolution.

Tests check known points against PROJ (`cs2cs`) reference values, recorded as
constants in the test.

**Upstream.** If ggplot-rs later wants projections, `project.rs` can move
there unchanged. It is not a dependency for this work.

### 3.3 Colour, classes, no-data, theme

All colours come from theme tokens, so light, dark, `colorblind` and
`high-contrast` follow without new tokens:

| Element | Token |
|---|---|
| Land / basemap fill | `wash(card, muted, 0.92)`; dark: the card lifted one step |
| Region borders | `chart.grid` at 0.6 px (subtle); the outer country outline in `chart.axis` |
| Water | none (the card shows through). An optional `'water'` fill is an open question |
| No data | a `<pattern>` hatch in `chart.muted`, plus a "No data (n)" legend entry |
| Sequential | `seq_low → seq_high` (theme). **Over a basemap** the ramp starts at 25% so pale points don't vanish. This replaces viridis |
| Diverging | `diverging_ramp()`. Used automatically when the measure spans 0, as for `::HEATMAP`; `::MIDPOINT n` sets the centre |
| Category | the theme palette (stable colours, as today) |
| Hover / selected | CSS in `theme-base.css`: `[data-map] .dp-hit:hover` gets a 1.5 px `--fg` outline; `.dp-sel` gets `filter: drop-shadow(0 1px 2px …)`. This is styling only; the behaviour is #147's |

**`::CLASSES`** (panel modifier, string):

| Spec | Breaks | Notes |
|---|---|---|
| `'quantile n'` | equal counts | ties deduplicated; fewer classes if the values have fewer distinct levels |
| `'jenks n'` | Fisher-Jenks optimal (DP, O(k·n²)) | above 2,000 values a deterministic sample (every m-th sorted value) |
| `'equal n'` | equal width | — |
| `'0,10,50,100'` | custom edges | values outside go to the end classes, labelled "< 0" / "≥ 100" |
| (absent) | continuous ramp, gradient legend | as today, themed |

- **Where:** in the renderer. The legend needs breaks *and* counts; facets and
  frames must share the classes; and SQL would need a second column per map.
  About 250 lines with the legend.
- **`::RANGE 'lo,hi'`** (existing role; its gauge meaning is "the domain")
  caps outliers. It clamps the colour domain, and the legend shows "≤ lo" /
  "≥ hi".
- **Stepped legend:** swatches with `[a – b)` labels formatted with the
  measure's format (`::MONEY`, `::PERCENT` and the locale) and the **count per
  class** ("12"). Placed top-left, or below the map at phone width.
- **Lint:** `map/bad-classes` (unknown method, n outside 2–9, edges not
  increasing).

### 3.4 Inputs: WKT, codes, lat/lon

| Input | SQL | Resolves to |
|---|---|---|
| WKT (as today) | `geom::MAP` | the parsed geometry |
| Region code | `code::REGION` | the pack geometry for that code |
| Coordinates | `lat::LAT, lon::LON` | a point |
| Explicit pack layer | `'de-districts'::BOUNDARIES` | forces the layer instead of auto-detection |
| Backdrop from the pack | `'world'::BASEMAP`, `'de-states'::BASEMAP` | a themed backdrop with no WKT column |

**Auto-detection of `::REGION`.** The layer with the most matching codes wins:
- 2 letters → country by ISO 3166-1 alpha-2 (Natural Earth `ISO_A2_EH`, so
  FR/NO work);
- 3 letters → alpha-3 / `ADM0_A3`;
- `DE-XX` → German state by ISO 3166-2;
- 5 digits → district by AGS;
- `DE` + 1–3 characters → German NUTS 1/2/3 (VG2500 carries a `NUTS` column).

Two-digit numeric codes (`'09'`) are ambiguous and need `::BOUNDARIES`. As a
**last resort**, an exact name match (`'Bayern'`, `'München'`) is tried, with
lint `map/region-by-name`.

**Lint:**
- `map/unknown-region` lists up to 5 codes that matched nothing;
- `map/lat-lon-swapped` fires when latitudes exceed ±90 but would be valid
  longitudes.

`::LABEL` defaults to the pack's name in the dashboard locale. Natural Earth
has `NAME_DE`, `NAME_EN` and others; BKG names are German.

Why join in the renderer rather than in SQL? It needs no `spatial` extension
(23.5 MB raw / 4.4 MB transfer in wasm, about 25× the whole pack), no SQL file
access (the locked server keeps `enable_external_access=false`), and it works
the same in the browser, `dashboard --svg`, `serve` and `anofox_render`.

### 3.5 Labels and furniture

- **Placement:** each polygon is labelled at its **pole of inaccessibility**
  (polylabel, ~120 lines), not its centroid, so labels stay inside C-shaped
  regions.
- **Collisions:** greedy, by priority (`::RANK`, else |value|, else area). A
  label that collides, or is wider than its region at the current zoom, is
  dropped.
- **Which labels:** `::LABEL_TOP n` labels the n largest; `::DATALABELS`
  labels all that fit.
- **Halo:** `paint-order="stroke"`, with the stroke in the card colour and the
  fill in `fg`, so labels read on any fill in both themes. Contrast is checked
  against the theme (T7 in plan-theming-layout).
- **Scale bar:** automatic when the projection isn't `'none'` and the map spans
  less than about 3,000 km. Its length is "nice" (1/2/5 × 10ⁿ km), in the
  bottom-left corner.
- **Attribution line:** automatic whenever a pack layer or tiles are drawn
  (the BKG licence requires it). It is small, muted text in the bottom-right
  corner: "Made with Natural Earth · © BKG (2026) dl-de/by-2-0, geometries
  simplified".
- **North arrow:** not in M1 (open question 5).

### 3.6 Accessibility

- **Summary.** `chartSummary` gets a map branch, keyed on `data-map`:
  "Map of 400 districts (equal-area conic). Highest: München 980; lowest:
  Zweibrücken 12. 5 quantile classes: 80 / 80 / 80 / 80 / 80; 3 without data."
  The headless SVG carries the same sentence in `<desc>`.
- **Charts as tables.** For a map it shows region name, code, value and class,
  sorted by value. It drops WKT and keeps `::LABEL`, which fixes today's gap
  (`web/app.js:12071`).
- **Not colour-only:** classes are named in the tooltip ("class 4 of 5:
  500–750"), and no-data is a hatch, not just grey.
- **Keyboard:** from #147. axe runs in the showcase e2e.

## 4. Built-in boundaries

### 4.1 Contents and sizes (measured 2026-10-17)

Built with DuckDB 1.5.5 + spatial: `ST_CoverageSimplify` keeps shared borders
intact, with 0 invalid geometries. **s3** = simplified, then rounded to 3
decimals (≈ 100 m). Each file is a CSV `key,k2,k3,name,wkt` (key + alternative
codes + name + WKT). Sizes in KB; gzip -9, brotli -q 11, parquet zstd.

| Layer | Source | Features | Download | s3 CSV / gzip / brotli / parquet |
|---|---|---|---|---|
| `world` (countries 1:110m) | Natural Earth | 177 | 839 | 168 / 63 / 46 / 53 |
| `world-50m` (optional) | Natural Earth | 242 | 3,083 | 1,347 / 484 / 303 / 345 |
| `de-states` | BKG VG2500 (as of 31.12.2025) | 16 | 3,297 (all levels) | 114 / 33 / 21 / 23 |
| `de-districts` | BKG VG2500 | 400 | (same file) | 479 / 128 / 88 / 95 |
| *NUTS 0–2 1:20M (not bundled, §4.2)* | GISCO | 39 / 115 / 299 | 1,409 | 424 / 145 / 101 / 117 |
| *NUTS 3 1:20M (not bundled)* | GISCO | 1,345 | 1,554 | 376 / 113 / 86 / 94 |

| Pack | raw | gzip | brotli |
|---|---|---|---|
| **Proposed default: world + de-states + de-districts** | **761** | **224** | **134** |
| + `world-50m` | 2,108 | 709 | 434 |
| (for comparison) with NUTS 0–2 at 20M | 1,194 | 368 | 187 |

Rounding to 4 decimals without simplifying roughly doubles every layer: the
districts take 1,015 KB raw / 285 KB gzip instead of 479 / 128.

### 4.2 Licences

| Source | Licence | Commercial | Attribution we print |
|---|---|---|---|
| Natural Earth ([terms](https://www.naturalearthdata.com/about/terms-of-use/)) | public domain | yes | optional; we print "Made with Natural Earth" |
| BKG VG2500 / VG5000 (`nutzungsbedingungen_vg2500.pdf`) | Datenlizenz Deutschland – Namensnennung 2.0 | yes, including redistribution | "© BKG (2026) dl-de/by-2-0, Datenquellen: https://sgx.geodatenzentrum.de/web_public/gdz/datenquellen/datenquellen_vg_nuts.pdf", plus a note that the data was modified (simplified). On the web "BKG" links to bkg.bund.de and "dl-de/by-2-0" to govdata.de/dl-de/by-2-0. |
| GISCO administrative units ([page](https://ec.europa.eu/eurostat/web/gisco/geodata/administrative-units)) | free "on condition that the data will **not be used for commercial purposes**", non-transferable | **no** (contact EuroGeographics) | "© EuroGeographics for the administrative boundaries" |
| GISCO NUTS | the NUTS page returned HTTP 429 twice; the geometry also comes from EuroGeographics, so **assume the same terms** until checked | assume no | as above |
| geoBoundaries | CC BY 4.0, but per-boundary sources vary | mostly | "geoBoundaries" + link |

anofox-visualization is a commercial (BUSL) product, so NUTS stays out of the
default pack. Germany is still covered at NUTS 1/2/3 granularity through
VG2500, which carries the NUTS code for each state, Regierungsbezirk and
district. For the rest of the EU, see open question 1.

**Disputed borders.** Natural Earth publishes "point of view" variants,
including a German view (`ne_110m_admin_0_countries_deu`). For a German
vendor the **DEU view** is the safer default (open question 4).

### 4.3 Shipping

- **Build:** `scripts/build-boundaries.sh` (DuckDB + spatial, run by a
  maintainer) downloads the sources, simplifies, rounds and writes
  `web/data/boundaries/<layer>.csv` plus `SOURCES.md` (URLs, dates, licences,
  tolerances). The outputs are committed (~761 KB).
- **Browser:** when a panel uses `::REGION` or a named `::BASEMAP`, JS fetches
  that layer from `data/boundaries/<layer>.csv` (same origin, memoised like
  `ensureDataFiles`). It hands the text to the wasm once with a new export,
  `register_boundaries(layer, text)`, and the renderer caches the parsed
  geometry. Only the layers in use are fetched: 63 KB gzip for `world`,
  128 KB for `de-districts`.
- **Native** (`dashboard --svg`, `serve`, extension, `anofox_render`):
  `include_str!` of the same files behind `cfg(not(target_arch = "wasm32"))`.
  That adds 761 KB to native binaries; the served extension already embeds
  `web/`, so it could reuse that copy.
- **Not in the wasm:** embedding the pack would add about 224 KB gzip to every
  page load, maps or not. The wasm grows only by the writer: an estimated
  +80–120 KB raw, with a budget of 150 KB for M1.
- **Offline and locked server:** same-origin static files with no outside
  request. The gallery's runtime jsdelivr fetch (`ensureGeo`) moves to the
  pack too.
- **SQL access** to the same files works today with no new code:
  `FROM 'data/boundaries/de-districts.csv'` already matches the
  `ensureDataFiles` pattern in the browser. A table function
  `anofox_boundaries('de-districts')` in the extension is deferred (open
  question 6).

## 5. Proposal: more map types (M2, M3)

All of these are roles on the same one-statement-per-panel shape.

| Type | SQL | Rendering |
|---|---|---|
| **Graduated symbols** (M2) | `lat::LAT, lon::LON, orders::SIZE` (or `code::REGION, orders::SIZE` → a symbol at the region's pole of inaccessibility); colour optional (a measure or `::CATEGORY`) | Area ∝ value (r = √v, max r 4% of the panel's short side), **largest drawn first** so small ones stay on top. 0.75 px card-coloured outline. **Size legend**: 3 nested circles at nice values. A constant `4::SIZE` is a fixed size, as for scatter |
| **Flow map** (M2) | `src::ORIGIN, dst::DEST, volume::SIZE`. Each end is a region code (`'DE-BY'`), a WKT `POINT`, or a struct `{'lon': …, 'lat': …}` (the list/struct-as-JSON convention) | Quadratic arcs bent to the right of the direction of travel, so A→B and B→A separate. Curvature 0.2 × length. Width 0.75–8 px (√ scale) with a width legend. A small arrowhead at 85% of the path. `'flow'::LINESTYLE` sets `class="dp-flow"` (existing CSS, paused under reduced motion). End points are drawn as small dots. `data-x` = `src→dst` |
| **Density** (M2) | `lat::LAT, lon::LON, 'auto'::HEXBIN` (or `'25 km'::HEXBIN`); the measure is summed, else rows are counted | A hex grid in **projected** space, so cells are equal-area under the default projections. Classes and legend as §3.3; empty cells are not drawn. With the h3 extension (community; builds for native and wasm, ~75 KB transfer) authors can aggregate in SQL instead: `h3_cell_to_boundary_wkt(h3_latlng_to_cell(lat, lon, 6))::MAP, count(*)::BAR`. The docs show both patterns. No h3 dependency in our code |
| **Bivariate** (M2) | `code::REGION, sales::BAR, margin::BIVARIATE` | Tertiles of each measure → a 3×3 palette mixed from `palette[0]` and `palette[1]` (Stevens-style). The 3×3 legend has axis captions from the column names. The tooltip names both classes |
| **Time map** (M3) | `week::FRAME` (new) next to any of the above | One SVG with the geometry once. Per-feature values per frame are kept as `data-frames` (fill, r, width per frame), with the **classes computed over all frames** so colours are comparable. JS adds ▶ / a slider / the frame label, and `'tween'` interpolates fills and radii keyed by `data-x`. Reduced motion steps without tweening. Headless and the HTML export show the last frame; the export keeps the player (in `animKit`). Lint `map/too-many-frames` > 200 |
| **Small multiples** (M3) | `region::FACET` or `week::FACET`, with `::FACET_NCOL` | A grid of map frames drawn by our writer (no ggplot facets), with a **shared projection, extent and classes** and one legend |
| **Tooltip mini charts** (M3) | `list(v ORDER BY week)::SPARKLINE` per feature row | `data-spark` values; the tooltip draws a 120×28 sparkline (the same JS as diagram node sparklines) |

**Why `::FRAME` and not `::PERIOD`.** `::PERIOD` is an input that re-runs the
query for one date range. A time map needs **all frames in one result** so
Play is instant and the classes are shared. The frame player is generic: a
later issue can enable it for bars and lines too.

**Spike symbols** (`'spike'` style) are deferred (open question 5).

## 6. Optional tile basemap (M3, opt-in)

- **Setting:** a per-installation setting next to the #146 app settings:
  - `app-config.json`: `"map_tiles": {"url": "https://tiles.example.internal/{z}/{x}/{y}.png", "attribution": "© OpenStreetMap contributors"}`;
  - `serve --app-map-tiles <url> --app-map-attribution <text>`, or the
    `anofox_serve` options JSON.

  `app_brand.rs` validates it with the same URL rules: https (or
  same-origin), and the `{z}`, `{x}`, `{y}` placeholders are required.
- **Per panel:** `'tiles'::BASEMAP` opts in. It forces `'mercator'` (raster
  tiles are Web Mercator); with no setting configured, lint
  `map/tiles-not-configured` fires and the pack backdrop is used instead.
- **Browser only:** the renderer emits `data-tiles` + the projected bounds,
  and JS inserts `<image>` tiles under the features with
  `referrerpolicy="no-referrer"`. `dashboard --svg` and the HTML export **never**
  include tiles (offline, no leak) and fall back to the pack backdrop.
- **Privacy:** the tile host sees the viewer's IP and viewport. The docs
  recommend a self-hosted tile server. The public OSM tile servers are not
  suitable: their usage policy limits heavy use.
- **CSP:** none today. If one is added, `img-src` must list the configured
  tile host.
- **Scope:** raster XYZ only. Vector styles (MapLibre GL) need a WebGL
  renderer and are out of scope.

## 7. Performance

- Pixel-space simplification at render time (§1 spike: 4–8× smaller SVG). The
  pack's own simplification sets the floor of detail.
- Each feature is parsed once and cached per result set. Today `geom_sf`
  parses each WKT twice per render.
- **No canvas.** A canvas fallback would split the static, export and a11y
  paths (`<title>`, keyboard, as-tables) in two. Instead:
  - lint `map/too-many-marks` above 20,000 points suggests `::HEXBIN` or h3;
  - the renderer hard-caps at 100,000 marks with a render warning.

  Open question 7.
- Static SVG and HTML export stay vector. The export embeds the frame data and
  player for time maps.
- **Budgets checked in tests:**
  - the districts showcase map ≤ 200 KB SVG at 1200 px;
  - render ≤ 50 ms natively in release;
  - wasm growth ≤ 150 KB raw for M1.

## 8. SQL surface summary

| Role | Category | Aliases | Value | Milestone |
|---|---|---|---|---|
| `MAP` | chart | `GEOMETRY`, `GEO`, `CHOROPLETH` | WKT (unchanged) | — |
| `BASEMAP` | chart | `MAPBASE`, `BACKDROP` | WKT, a pack layer (`'world'`, `'de-states'`, `'de-districts'`), or `'tiles'` | M1 (layers), M3 (tiles) |
| **`REGION`** | chart | `REGION_CODE`, `ISO_CODE` | ISO 3166-1/-2, AGS, DE NUTS, or a name | M1 |
| **`LAT`** / **`LON`** | chart | `LATITUDE` / `LONGITUDE`, `LNG` | degrees (WGS84) | M1 |
| **`BOUNDARIES`** | modifier | `REGION_SET` | a pack layer name | M1 |
| **`PROJECTION`** | modifier | `PROJ` | `'auto'`, `'equal-earth'`, `'mercator'`, `'albers-europe'`, `'conic'`, `'utm'`, `'none'` | M1 |
| **`CLASSES`** | modifier | `CLASS_BREAKS`, `BINS` | `'quantile 5'`, `'jenks 5'`, `'equal 5'`, `'0,10,50,100'` | M1 |
| **`MIDPOINT`** | modifier | — | the diverging centre (number) | M1 |
| `RANGE` | modifier | — | `'lo,hi'`: caps the colour domain on a map | M1 (new use) |
| `LABEL`, `LABEL_TOP`, `RANK`, `DATALABELS` | existing | — | per-feature labels | M1 (new use) |
| `SIZE` | existing | — | graduated symbols (area), flow width | M2 |
| **`ORIGIN`** / **`DEST`** | encoding | `FROM_PLACE` / `DESTINATION`, `TO_PLACE` | code, WKT POINT or `{lon, lat}` | M2 |
| **`HEXBIN`** | modifier | `HEX` | `'auto'` or `'25 km'` | M2 |
| **`BIVARIATE`** | encoding | `BIVAR` | the second measure | M2 |
| **`FRAME`** | encoding | `PLAY_BY`, `TIME_FRAME` | the frame key (ordered) | M3 |
| `FACET`, `FACET_NCOL`, `SPARKLINE`, `LINESTYLE`, `ANIMATE`, `TRANSITION` | existing | — | small multiples, tooltip spark, flow dashes, entrance, tween | M2/M3 |

Each new role is one registry row in `src/roles.rs` plus a `Role` variant, with
`DOCS.md` kept in sync by the existing tests. None of the new names is a
DuckDB type, and none collides with an existing token or alias. `HEXBIN` is
used because `DENSITY` is the KDE chart and `TILES` is a heatmap alias.

Lint rules (namespace `map/*`):
- `unknown-projection`, `bad-classes`;
- `unknown-region`, `region-by-name`, `lat-lon-swapped`;
- `mercator-choropleth` (hint), `sync-projection`;
- `too-many-marks`, `too-many-frames`, `tiles-not-configured`.

## 9. Delivery PRs and tests

| PR | Scope | Size | Tests / exit |
|---|---|---|---|
| **M1a: own map writer** | `src/map/` (model, projections, fit, pixel simplification, evenodd paths, themed colours, classes, stepped/gradient legends, no-data hatch, #147 attributes + `data-x`/`data-class`/`data-proj`, row order); `PROJECTION`, `CLASSES`, `MIDPOINT`, `RANGE` on maps; `map_bounds` in projected units; map a11y summary + as-tables; lint `map/*` (first five) | **L**: ~1,400 Rust lines incl. tests, ~120 JS, ≈ 1.5 wk | Render regression for every projection × class method × light/dark; projection points vs PROJ constants; Jenks against a published dataset; #147's map tests unchanged and green; the existing examples render identically in structure; wasm Δ ≤ 150 KB |
| **M1b: boundaries, codes, labels** | `scripts/build-boundaries.sh`, `web/data/boundaries/*` + `SOURCES.md`; `register_boundaries` + lazy fetch; native embed; `REGION`, `LAT`/`LON`, `BOUNDARIES`, pack `::BASEMAP`; polylabel + collision labels with halo; scale bar; attribution; gallery Map tab off jsdelivr | **M–L**: ~900 Rust, ~100 JS, ≈ 1–1.5 wk | Pack integrity test (each layer parses, codes unique, the 16/400 counts, attribution present); code-detection unit tests; e2e: offline (network blocked except same origin), axe, phone width |
| **M1c: showcase + example upgrades** | "Where do our deliveries and customers cluster, and which regions are underserved?" (§10); `optimize-delivery` gets the themed look (`'none'` projection auto-detected) | **S–M** | `dashboard --check --strict` green; showcase e2e |
| **M2: types** | graduated symbols + size legend; flows (+ supply-chain flow tab: retailer → DC → factory → supplier); hexbin; bivariate | **M–L**: ~900 Rust, ~80 JS, ≈ 1.5 wk | Render regression per type and theme; flow direction separation test; hex area equality under Equal Earth; e2e on the supply-chain tab |
| **M3: motion and more** | `FRAME` player (Rust frames + `anim.js`), `FACET` grids, tooltip sparklines, opt-in tiles (+ #146 setting, validation, lint), zoom tools (double-click to feature, fit to selection, reset button, "locate" region search) | **L**: ~600 Rust, ~450 JS, ≈ 2 wk | e2e: play/pause/slider keyed tween, reduced motion; tiles absent from export; setting validation tests; a11y of the player controls |

**Total:** about 5.5–6.5 weeks. M1a and M1b can go in parallel once the
`src/map/` model types are merged.

## 10. Showcase

**"Where do our deliveries and customers cluster, and which regions are
underserved?"** It is a generated, synthetic dataset
(`scripts/gen-maps-showcase.sh`, deterministic seed) for a German wholesaler:
customers and deliveries with lat/lon and a district code, 3 DCs, and 12
weeks.

| Tab | Panels |
|---|---|
| Overview | KPI row (customers, deliveries, on-time %, share of districts with no customer). Choropleth: customers per 10k inhabitants by district (`'quantile 5'`, labels top 8). Table of the 10 most underserved districts |
| Clusters | Deliveries as a hexbin (`'auto'::HEXBIN`) with the DCs as graduated symbols, plus the same data as an h3 SQL panel side by side |
| Flows | DC → district flows by volume (`'flow'::LINESTYLE`), coloured by DC |
| Over time | On-time % by state, `week::FRAME` with Play; small multiples of four weeks |

Population per district is part of the generated data. VG2500 has no
population figures, and Destatis figures can be added later under dl-de/by-2-0.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Owning the writer drifts from the chart look (fonts, legend style). | Legends reuse the theme tokens and font metrics used for KPI and diagram text; one shared legend helper; render-regression snapshots in both themes. |
| #147 and M1a touch `render_map` at the same time. | #147 merges first; M1a keeps its attribute contract and tests; `mapmarks.rs` is removed in the same PR. |
| Licence error in the pack. | Only public-domain and dl-de/by-2-0 sources; `SOURCES.md` per layer; a test that the attribution line is present whenever a pack layer is drawn. |
| Auto-projection surprises (a dataset spanning 125° flips to Equal Earth). | Deterministic rule documented; `data-proj` shown in the panel's ⓘ; `::PROJECTION` overrides. |
| Code detection guesses the wrong layer. | Majority rule + `::BOUNDARIES` override + `map/unknown-region` listing the misses. |
| Time maps with many frames bloat the SVG. | Geometry once, values per frame; the frame cap is linted. |

## 12. Decisions for the owner

1. **Own map writer** (`Panel::Own`), with ggplot-rs used only for
   `parse_wkt`. *Recommended.* The alternative is six or more upstream
   ggplot-rs changes before M1.
2. **Default pack = Natural Earth 110m + BKG VG2500 states/districts**,
   134 KB brotli / 224 KB gzip, lazy per layer. NUTS for the rest of the EU is
   not bundled (licence). Add `world-50m` (+300 KB brotli) or not?
3. **Codes resolved in the renderer** (no SQL table function in M1).
4. **Projections in our crate**, with `'auto'` = none / Equal Earth / UTM /
   conic by extent.
5. **Classes in the renderer** with the stepped legend and counts.
6. **Tiles:** opt-in, per installation, raster only, browser only. Or drop
   them altogether?
7. **`::FRAME`** for time maps (a client-side player), rather than `::PERIOD`.
8. **Density:** a renderer hexbin plus a documented h3 SQL pattern; no h3
   dependency.
9. **No canvas**: aggregate instead.
10. **Milestone cut** as in §9 (M1a/M1b/M1c, M2, M3). Performance work moves
    into M1, because the own writer does it naturally.

## 13. Open questions

1. **EU regions:** license EuroGeographics data for NUTS, ship a
   `scripts/fetch-nuts.sh` that users run themselves (they then hold the
   licence), or use geoBoundaries ADM1 (CC BY 4.0, not NUTS-coded)? The NUTS
   page itself still needs a check; Eurostat returned 429.
2. Should the default pack include **`world-50m`** for country-level zooms
   (300 KB brotli), or should it stay a separate optional layer?
3. **Water/ocean fill**, or keep the card colour? (Datawrapper-style blank
   oceans read cleaner in dark mode.)
4. Use the **Natural Earth "DEU" point-of-view** borders by default?
5. **North arrow** (print only?) and **spike symbols**: worth adding, or out?
6. Should the extension get **`anofox_boundaries(layer)`** for SQL joins
   (e.g. a point-in-polygon with spatial), or is reading
   `data/boundaries/*.csv` enough?
7. The hard cap for marks (100k) is still to settle: is a render warning
   enough, or should the panel refuse?
8. Should label names follow the **dashboard locale** (`NAME_DE` / `NAME_EN`)
   or the data's `::LABEL` only?

## 14. Decisions (2026-10-17)

The owner approved the plan. Every decision in §12 takes its recommendation, and the open questions in §13 are settled as follows. The owner can still redirect any of them.

1. **EU regions:** no NUTS in the pack. Ship `scripts/fetch-nuts.sh`, which users run themselves (they then hold the EuroGeographics licence) and which writes the same layer format. geoBoundaries ADM1 is not bundled. Someone still has to read the NUTS terms page before the script ships.
2. **`world-50m`:** a separate optional layer, not in the default pack.
3. **Water:** no fill; it keeps the card colour.
4. **Borders:** the Natural Earth default view. A point-of-view option can come later.
5. **North arrow:** print only, optional. **Spike symbols:** out for now.
6. **`anofox_boundaries(layer)`:** not in M1. Revisit after M1b.
7. **Mark cap:** a render warning above 100k marks, with the aggregation hint. The panel is not refused.
8. **Labels:** built-in boundaries use the dashboard locale (`NAME_DE`/`NAME_EN`) when available, and `::LABEL` from the data always wins.
9. **Tiles:** stay opt-in, per installation, raster and browser only.
