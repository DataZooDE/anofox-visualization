# Plan: theming, layout & customization

Status: proposal (2026-10-15), based on master `43757d8`, ggplot-rs 0.17.1.
Parent plan: [plan-anofox-visualization.md](plan-anofox-visualization.md).

**Goal.** Let a dashboard look *on-brand, correct in dark mode and in German
number formats, and well laid out*, configured **in SQL** (role casts and a
few directives), with no GUI. The browser, `dashboard` CLI, `serve`, exported
HTML and the contract macros should all produce the same result.

**The three decisions this plan proposes:**

1. **One theme model for the whole dashboard** (`src/theme.rs`): a
   JSON-serialisable set of design tokens.
   - It covers charts **and** page chrome: background, cards, header, tabs, KPI tiles, callouts, tables, inputs, tooltips, buttons, footer, print.
   - Rust applies it to ggplot-rs and to the headless dashboard SVG.
   - The same tokens are emitted as **CSS custom properties** for the builder, `serve`, `duckext` serve / locked pages, the CLI static HTML and the exported HTML/PDF.
   - A dashboard selects or defines it with **`::THEME`** (a name or a JSON object).
   - Author branding (logo, brand name, favicon) is part of the theme. The "Powered by DataZoo GmbH" credit is **mandatory** and always shown (owner decision, see T9).
2. **One generic per-panel options role** **`::OPTS 'key=value; …'`** (JSON
   accepted too). It is backed by an *options registry*, built like the role
   registry, so the parser, lint, DOCS and skills all derive from one table.
   No new role per knob.
3. **A dashboard locale** (**`::LOCALE 'de-DE'`** or the theme's `locale`
   token) that every number and date format honours. Today `en-US` and `$` are
   hard-coded.

---

## 1. Current state (evidence)

### 1.1 Theming

| Aspect | Today | Evidence |
|---|---|---|
| Render options | `RenderOptions { brand, zoom, max_categories, max_line_points }`. The **brand colour is the only visual knob.** | `src/lib.rs:384-406` |
| Brand default | `DZ_COLORS[0]` steel `#456481`. Legacy thread-local `set_brand` is read only by `render()`. | `src/lib.rs:411-413`, `:452`, `:768-772` |
| Palette | One fixed 12-colour `DZ_COLORS`: [4] is near-black, [11] is slate (used for "Other"). Categories are sorted and then mapped in order. A level whose value is `#rrggbb` is drawn in that colour. | `src/lib.rs:349-364`, `:474-487` |
| Brand → ggplot | `GGPlot::primary_color(brand)` plus explicit `color:`/`fill:` on many geoms; `lighten`/`darken` variants for box, violin and gauge. | `src/lib.rs:1617, 1701-1917, 1711, 1835, 2750` |
| Sequential scales | Hard-coded 2-stop gradients from light to brand (heatmap `#edf1f7`, calendar `#ebf1f7`). **No diverging scale**; viridis/brewer are unused. | `src/lib.rs:2350-2353`, `:2420` |
| ggplot theme | `theme_minimal()` for role charts, `theme_void()` for sparklines. Contract "house theme" is `theme_minimal_base(13.2)` plus tweaks. | `src/lib.rs:2164…2554`, `:2620`; `src/contract/look.rs:17, 67-83` |
| ggplot-rs offers | Presets gray/bw/minimal/classic/pubr/linedraw/light/dark/void (+ `_base(size)`). A full `Theme` (text, grid, backgrounds, legend incl. `LegendPosition::Inside`, strips, `primary`). Scales: manual/gradient/gradient2/gradientn/viridis/brewer/fermenter/grey/steps. **No base font family** (only per-element `ElementText.family`, default `"sans-serif"`). | `ggplot-rs-0.17.1/src/theme/presets.rs`, `theme/mod.rs:10-18, 71-141`, `theme/elements.rs:4, 28` |
| Web app dark mode | 🌙 toggles `body.dark`. It does not follow `prefers-color-scheme` and is not persisted. **Charts are not themed:** dark mode forces cards back to white. PNG export picks its background from the dark flag. | `web/index.html:390, 405-408, 431`; `web/app.js:1157, 4191` |
| CSS tokens | `:root` has `--bg --card --ink --muted --line --accent --accent2 --accent3 --grad --font-body` (Hanken Grotesk). | `web/index.html:11-20` |
| SQL / spec / URL reach | **No** theme, palette or dark option in roles or macros. Macros take only `title/x_label/y_label`. Spec JSON reads only `"primary"`. URL `?primary=RRGGBB` (undocumented) recolours the UI accent and brand. | `src/contract/look.rs:33-39`; `src/lib.rs:621-625`; `web/app.js:1103-1119` |
| Exports & servers | `downloadHtml()` copies the page CSS and rendered panels with a hard-coded `#f4f6f9` background; dark mode is lost. `dashboard` CLI has **no** `--brand/--theme` flag and system-ui CSS. `serve` has hard-coded light `STYLE`. | `web/app.js:3606-3614`; `src/bin/dashboard.rs:18-23, 292`; `src/bin/serve.rs:79-97, 654-663` |
| Consistency | The SVG is identical in browser (wasm) and headless (same `render_with`), but **page chrome diverges**: backgrounds `#e8f5f3` (web), `#f4f7fc` (CLI SVG), `#f4f6f9` (serve/export); fonts Hanken / system-ui / SVG `sans-serif`. | `src/wasm.rs:92-94`; `src/dashboard.rs:41-66, 274-276` |
| Stable model colours | `model_color` gives known model ids a fixed `DZ_COLORS` slot (ols→0, huber→1, …). Unknown ids use FNV-1a mod 11. `level_colors` gives unknowns free slots first. **No user override; independent of theme.** | `src/contract/look.rs:147-236`; `docs/DOCS.md:250-253` |

### 1.2 Page chrome (everything around the charts)

There are five independent style sources, and only one uses tokens:
1. `web/index.html` CSS
2. inline styles in `web/app.js`
3. `src/bin/serve.rs` `STYLE`
4. `src/bin/dashboard.rs` `STYLE`
5. the literals in the `src/dashboard.rs` SVG

| Element | Today | Evidence |
|---|---|---|
| Tokens | `:root`: `--bg --card --ink --muted --line --accent(#0fb5a5) --accent2 --accent3 --grad --grad-soft --shadow --shadow-hi --radius(18px) --font-display(Bricolage Grotesque) --font-body(Hanken Grotesk)`. Fonts come from Google Fonts. | `web/index.html:9, 11-21` |
| Token bypass | About 159 hex and 90 `rgb()/rgba()` literals after `:root`. The teal accent is repeated as raw `rgba(42,157,143,…)` / `rgba(15,181,165,…)`. Radii 8/9/10/12/16px are hard-coded (`--radius` is used only by `.panel/.group`). Deltas `#0ca678/#e03131`, badges, tooltip, zoom `#456481`, markdown and scrollbar are literals. | `web/index.html:37-388` (e.g. 37, 168, 179, 191, 224, 238, 241-244, 318-338) |
| Page bg | 3 layered gradients plus a dotted `body::before`, all literal | `web/index.html:23-29` |
| Header | `<h1>anofox-visualization</h1>` with a gradient dot (no logo), tagline, actions (MotherDuck, Share, ⤓ HTML, refresh, 🌙). `document.title` is never set. | `web/index.html:6, 35-40, 418-434` |
| Cards / groups / sections | `.panel` (card, 1px line, radius, shadow, teal hover lift); `.panel-title` with gradient underline; `.group` with a 5px gradient left bar; `.section` heading | `web/index.html:167-200` |
| Tabs | `.tabbar/.tab-btn` gradient underline; `.subtabbar` pills | `web/index.html:305-316` |
| KPI tiles | `.metric-value` gradient-clipped text 2.6rem; grouped strip; `.textcard` | `web/index.html:187-190, 220-230` |
| Markdown / "What to look for" | `.md-box` = `--grad-soft` plus a gradient bar. There is **no callout variant**; "What to look for" is a titled `::MARKDOWN`. | `web/index.html:204-205, 369-385`; `web/app.js:2370`; `web/examples/statistics-showcase.sql:287` |
| Tables | Sticky header with 2px rule, **no zebra** (hover only), numerics right-aligned in JS, badges; `::COLORSCALE` hard-coded green→amber→red in JS; data bars literal rgba | `web/index.html:206-247`; `web/app.js:2891, 2993-3027, 3243-3251` |
| Inputs / filters | `.controls` bar with a gradient left bar; multiselect `.dp-ms`; select chevron as an inline SVG `#667085`; slider/spinner CSS **injected from JS** (`#0f766e #5eead4 #cbd5e1`) | `web/index.html:135, 169-178, 283-297`; `web/app.js:1843-1864` |
| Buttons | gradient primary, `.btn2`, `.dl-btn`, pager, panel tools `.dp-tool` | `web/index.html:149-154, 210-214, 259, 329-335` |
| Tooltip / legend | Tooltip is HTML `.dp-tip` (dot inline-styled). Legend is SVG inside the chart; hidden entries are greyed `#c4c9d2` from JS. | `web/index.html:318-325`; `web/app.js:3866, 4210-4256` |
| Footer / credit | "Powered by DataZoo GmbH" is appended on **every** render, **not hideable**, shown in embed and in HTML export. Sidebar shows "© DataZoo GmbH". | `web/app.js:2498-2508`; `web/index.html:299-301, 456-459` |
| Favicon | none (no `rel="icon"` anywhere) | — |
| Dark | about 47 `body.dark` rules. They override only `--bg --ink --muted --line` (not `--card`/`--accent`), darken the shell, **force cards white**, and are not persisted. | `web/index.html:390-412`; `web/app.js:1157` |
| Print | `@media print` hides editor, actions, loadbar and `.dl`, but targets a stale `.layout` (the shell is `.shell/.workspace`), so the sidebar and toolbar may print | `web/index.html:276` |
| Builder chrome | Sidebar, toolbar, editor and explore view share the **same** tokens as the dashboard. `?embed=1` or `window.__served` → `body.embed` hides builder chrome. `?primary=` is the only runtime hook. | `web/index.html:41-129`; `web/app.js:1102-1123` |
| Served nav | Fully inline-styled nav (`#fff #e5e9f0`, active `#0f766e`, `system-ui`), off-brand | `web/app.js:1874-1893` |
| `serve` (CLI) pages | Own template `<h1>{title}</h1>`, `STYLE` with system-ui, `#f4f6f9`, radius 14, no tokens, no footer | `src/bin/serve.rs:586-607, 654-665` |
| `duckext` serve / locked | Injects `window.__served` into `index.html`, so it gets the full builder CSS in embed mode plus the inline nav and the Powered-by credit. List page inline-styled. | `duckext/src/serve.rs:833-873` |
| Headless SVG | bg `(f4,f7,fc)`; card `rx=14 #fff stroke #e6e9f1`; titles system-ui 14px `#1f2430`; KPI 34px `#1f2937` / caption `#667085`; table colours literal. **Nothing themeable except bg/brand.** | `src/dashboard.rs:51-64, 218-219, 288-299, 375-413, 457-497` |
| Static HTML (CLI) | Title/h1 "anofox-visualization dashboard" plus source line; own mini `:root` | `src/bin/dashboard.rs:280-302` |
| HTML export | Copies all `index.html` CSS and `.dash` (incl. credit). Fixed title "anofox-visualization dashboard". Inline `background:#f4f6f9` overrides the token. Loses dark, `?primary` and the JS-injected CSS. | `web/app.js:3606-3634` |

### 1.3 Layout

| Aspect | Today | Evidence |
|---|---|---|
| Grid | 12-col CSS grid, gap 1.15rem. `::COLUMNS n` sets default span `12/n`; `::COL n` and `::HEIGHT px` apply to the *next* panel and then reset. | `web/index.html:165`; `web/app.js:2004, 2050, 2070-2072, 2152-2169` |
| Heights | `::HEIGHT` → `min-height` px only; contract plots use `contractHeight(macro)`. No row equalisation, no named sizes, no `auto`. | `web/app.js:2169, 2181` |
| Boxes / tabs | `::GROUP … ::ENDGROUP` (always full-width, ignores `::COL`), `::TAB`, `::SUBTAB` (deep link `?tab=`), `::PLACEHOLDER`. | `src/roles.rs:228-235`; `web/app.js:~2052-2152` |
| KPI rows | A run of ≥2 bare top-level KPI tiles auto-packs into a strip (browser only; not inside groups or tabs). | `web/app.js:2006-2012`; `docs/dashboard-design.md` §2 |
| Text | `::MARKDOWN`, `::TEXT_SMALL/MEDIUM/LARGE`; a lone `::LABEL` is a full-width heading (and forces a row break). | `src/roles.rs:193-196`; `docs/dashboard-design.md` §7 |
| Contract composition | ggplot-rs `PlotGrid` (patchwork-style), `ncol(grid_cols(n,4))`, `collect_legends(true)`. | `src/contract.rs:747, 774`; `src/contract/terms.rs:674`; `src/contract/stats.rs:113-119, 1478` |
| Responsive | Two breakpoints: ≤760px all panels full width; ≤820px editor stacks. | `web/index.html:166, 413` |
| Print / export | `@media print` hides chrome (no page breaks). `::DOWNLOAD_PDF` = `window.print()`. Per-panel PNG (2×, white bg). Table CSV. Whole-page HTML via `downloadHtml()`. | `web/index.html:276`; `web/app.js:3471-3513, 3606` |
| Chrome | `::RELOAD`, `::HEADER_IMAGE`, `::FOOTER_LINK`. **No dashboard title/subtitle/logo/footer text.** | `src/roles.rs:237-239`; `web/app.js:~2341-2355` |
| Headless | `render_dashboard_svg` packs spans left→right and honours `::COLUMNS/::COL/::HEIGHT`, but **skips groups and tabs** (no boxes, no sections) and is not responsive. | `src/dashboard.rs:91, 108, 136-226` |

### 1.4 Customization

| Can do today | Evidence |
|---|---|
| Axis formats `::YFORMAT/::XFORMAT`: `$ € £ ¥ % comma` keywords, otherwise literal prefix, or suffix if the value starts with a space. **Currency always prefixed** (`€1,234`). | `src/lib.rs:2255-2285` |
| KPI formats Plain / Percent / Compact / Money; **Money is hard-coded `$`**. | `src/format.rs:87-106` |
| Transforms `::XSCALE/::YSCALE` (log10/sqrt/reverse), `::FLIP`, `::FACET(_FREE)` + `::FACET_NCOL`, `::SMOOTH_METHOD`, `::ALPHA` (maps), gauge `::RANGE/::COLORS/::LABELS`. | `src/roles.rs:162-181` |
| Annotations `::REFLINE ::XLINE ::MARKAREA ::BAND_* ::ABLINE ::IDENTITY ::DATALABELS ::LABEL_TOP ::COOKS_CONTOUR`. | `src/roles.rs:142-159` |
| Tables: `::COLORSCALE`, `::BADGE`, `::TREND`, `::PLAIN` (no rule-based thresholds). | `src/roles.rs:199-211`; `web/app.js:1752` |
| Per-series colour only via a `#rrggbb` category *value*. | `src/lib.rs:474-487` |
| Contract macros: `title, x_label, y_label, width, height` plus macro-specific (`highlight`, `facet`, `band`, `kind`, `label_top`, …). | `src/macros.rs:146-201`; `src/contract/look.rs:36-38` |

**Cannot do today:** choose a theme or palette; dark charts; dark-mode SVG/PNG/HTML export;
fonts; per-series colours by name; stable colours across a dashboard for non-model
series; legend position or visibility; axis limits and ticks (the internal
`with_limits/with_breaks` at `src/lib.rs:2106-2113` is not exposed);
subtitle/caption; gridline style; text annotations; locale number/date formats
(`NUM_LOCALE="en-US"`, `web/app.js:3298`; decimal mark always `.` in ggplot-rs
`scale/format.rs:43`); tooltip templates; table thresholds; dashboard
title/logo; custom CSS; section headings that don't break 2-up rows; sticky
filters; print page layout; brand or theme in CLI/serve.

---

## 2. How comparable tools do it (and what we take)

| Tool | Mechanism | Take for a SQL-first tool |
|---|---|---|
| Evidence.dev ([themes](https://docs.evidence.dev/core-concepts/themes)) | ~8 semantic tokens, each with a `light`/`dark` pair, in `evidence.config.yaml`; named `colorPalettes` and `colorScales`; named formats (`eur0k`, `pct1`). | Tokens with light/dark pairs; **named formats** next to raw ones. |
| Observable Framework ([themes](https://observablehq.com/framework/themes)) | Stackable named themes (`["air","near-midnight"]` = auto light/dark); CSS variables; `grid-cols-N`, `grid-colspan-N`, `card`. | `::THEME 'light+dark'` stacking; charts read the page tokens. |
| Grafana ([overrides](https://grafana.com/docs/grafana/latest/panels-visualizations/configure-overrides/)) | Field defaults plus **overrides by name/regex**; thresholds; value mappings; `?theme=`; 24-col `gridPos`. | Series→colour map; `thresholds` for tables and KPIs; viewer `?theme=`. |
| Superset 6 ([theming](https://superset.apache.org/admin-docs/configuration/theming/)) | AntD tokens plus brand logo; `THEME_DEFAULT`/`THEME_DARK`; **themes as JSON rows**, per dashboard; `label_colors` JSON. | Themes as JSON, loadable from a **table or file** in SQL; dashboard-wide label colours. |
| Metabase ([appearance](https://www.metabase.com/docs/latest/configuring-metabase/appearance)) | White-label colours, logo, font; global localisation (separators, currency, dates) with per-column overrides. | Global `locale` plus per-column format overrides. |
| Vega-Lite ([config](https://vega.github.io/vega-lite/docs/config.html), [vega-themes](https://github.com/vega/vega-themes)) | One `config` object (range.category, axis, legend, font, background); d3-format plus `formatLocale`. | Compile tokens into **one** renderer config; never hard-code colours in chart code. |
| ggplot2 + thematic ([thematic](https://rstudio.github.io/thematic/)) | Complete themes plus element overrides; `thematic_on()` derives plot bg/fg/accent/font from page CSS. | Same tokens drive page CSS and plot theme. |
| ECharts ([theme builder](https://echarts.apache.org/en/theme-builder.html)) | `registerTheme(name, json)`, built-in `dark`; option deep-merges over theme. | Named themes as JSON; deep-merge precedence. |
| Streamlit ([theming](https://docs.streamlit.io/develop/concepts/configuration/theming)) / Plotly ([templates](https://plotly.com/python/templates/)) | Flat keys (`primaryColor`, `backgroundColor`, `font`, `chartCategoricalColors`, `[theme.dark]`); Plotly composable templates `"plotly_white+brand"`, `separators`. | Small flat key set covers 90%; composition `base+overlay`. |

**Resolution order (all tools agree):** built-in preset → dashboard theme →
panel options → column cast. We adopt exactly that and document it.

---

## 3. Theming

### T1 — Theme model shared by renderer, web, exports (P0, M)

- **Problem:** colours, fonts and backgrounds are scattered over `DZ_COLORS`,
  literals in `lib.rs`, `look.rs`, `index.html`, `serve.rs` and `dashboard.rs`.
  The three page backgrounds disagree, and charts can't follow dark mode.
- **Model** (`src/theme.rs`, serde):

```json
{
  "name": "acme", "extends": "light",
  "palette":    ["#0b5fff", "#e11d48", "#16a34a", "#f59e0b", "#7c3aed", "#0891b2"],
  "sequential": ["#eef3ff", "#0b5fff"],
  "diverging":  ["#c0392b", "#f5f5f5", "#1e8449"],
  "colors": { "bg": "#f4f6f9", "card": "#ffffff", "ink": "#1f2937", "muted": "#6b7280",
              "grid": "#e5e7eb", "accent": "#0b5fff", "other": "#94a3b8",
              "positive": "#16a34a", "negative": "#dc2626", "warning": "#d97706" },
  "font": { "family": "Inter, system-ui, sans-serif", "display": "Inter Tight, sans-serif",
            "size": 13, "src": ["fonts/inter.woff2"] },
  "density": "comfortable", "radius": 10, "shadow": "sm", "border": 1,
  "chrome": { "header_bg": "card", "accent_bar": true, "gradient": false,
              "kpi_value": "ink", "table_zebra": true, "callout": "accent" },
  "brand": { "name": "ACME Analytics", "logo": "logo.svg", "logo_dark": "logo-dark.svg",
             "favicon": "favicon.svg", "title_suffix": " · ACME" },
  "locale": "de-DE",
  "series": { "ols": "#0b5fff", "Berlin": "#e11d48" },
  "defaults": { "legend": "bottom", "grid": "y" },
  "dark": { "colors": { "bg": "#0f1729", "card": "#162033", "ink": "#e5e7eb", "grid": "#26324a" } }
}
```

- **Implementation:**
  - `Theme::resolve(name|json) -> ResolvedTheme`, applying `extends` with a deep merge and selecting the `dark` variant.
  - `RenderOptions.brand` becomes `RenderOptions.theme: Arc<ResolvedTheme>`; `brand` stays as sugar for `palette[0]`/`accent`.
  - One function `theme::gg(&ResolvedTheme, base) -> Theme` replaces the `theme_minimal()` calls and `look.rs::house_theme` (`theme_update` on text, grid, plot/panel background, legend).
  - `dz_color(i)` reads `theme.palette`. The gradient literals (`lib.rs:2350, 2420`) read `theme.sequential`.
  - `ResolvedTheme::css_vars()` emits the full token set (see T8) for `web/index.html`, `src/bin/serve.rs` `STYLE`, `duckext/src/serve.rs` and the `src/bin/dashboard.rs` page CSS. The headless SVG (`src/dashboard.rs`) reads the same `ResolvedTheme` for card, title, KPI and table colours.
  - Exposed to wasm as `resolve_theme(json) -> json`.
- **Acceptance:**
  - Default theme `datazoo` renders **byte-identical SVG** to today (golden tests on every showcase).
  - One source of colour literals: lint test greps `src/lib.rs` for `#[0-9a-f]{6}` outside `theme.rs`.
  - Web, serve and CLI pages share `--bg` for the same theme.

### T2 — Built-in themes (P0 light/dark; P1 the rest; S each)

| Name | Purpose | Notes |
|---|---|---|
| `datazoo` (= `light`, default) | today's look | byte-identical |
| `dark` | dark cards, light ink, desaturated palette | contrast ≥ 3:1 for marks on `card` |
| `print` / `mono` | white, black ink, greys, dashes/shapes as secondary encoding | used by print CSS and PDF |
| `colorblind` | Okabe-Ito palette, viridis sequential, blue–orange diverging | the safe default for public dashboards |
| `high-contrast` | WCAG AAA text, thick grid-less axes, 1.15× font | accessibility |

Stacking (Observable style): `'datazoo+dark'` = light by default, dark when
the viewer prefers dark. `'acme+print'` = corporate colours in print mode.

### T3 — Selecting/defining a theme in SQL (P0, S)

```sql
SELECT 'dark'::THEME;                                    -- a built-in
SELECT 'acme+dark'::THEME;                               -- stacked
SELECT content::THEME FROM read_text('themes/acme.json');-- corporate theme from a file
SELECT to_json(t)::THEME FROM themes t WHERE name='acme';-- …or from a table
SELECT 'de-DE'::LOCALE;                                  -- locale (also a theme token)
```

- **Rules:**
  - `::THEME` is a dashboard-level directive (category `chrome`, `renders=false`). The value is a name, `a+b`, or a JSON object (leading `{`). The last one wins; lint warns on more than one.
  - **Macros / render-only build:** `SET VARIABLE anofox_theme = 'dark';` is read by every `anofox_plot_*` macro via `coalesce(theme, getvariable('anofox_theme'))`. Each macro gets a new `theme :=` option, and spec JSON gets a `"theme"` key (beside `"primary"`, `src/lib.rs:621`).
  - **CLI/serve:** `dashboard --theme NAME|file.json`, `serve --theme …`. The SQL `::THEME` overrides the flag.
  - **URL:** `?theme=dark` lets the viewer switch only between variants the dashboard allows (`light`/`dark`/`print`), never inject JSON. `?primary=` keeps working (documented now).
  - **Locked serving:** the theme is resolved at load and frozen into the snapshot. `read_text` for themes must pass the allow-list, or be resolved at `serve` start.
- **Acceptance:** sqllogictest per form; `serve` locked test with a file theme; wasm `plan()` exposes `theme`.

### T4 — Dark mode that includes the charts, and dark-correct export (P0, M)

- **Problem:** `body.dark .panel{background:#fff}` (`web/index.html:405`) is
  a workaround. PNG export guesses the background; HTML export loses dark.
- **Plan:**
  - The toggle and `prefers-color-scheme` set the variant and **re-render the panels through wasm** with the dark `ResolvedTheme`. This is cheap because results are cached, so no SQL re-runs. Delete the white-card override.
  - Persist the choice in `localStorage` (convenience only).
  - Export: PNG/SVG use `theme.colors.card`. `downloadHtml()` and `dashboard --html` embed the **current variant** plus the CSS vars, so the background is no longer hard-coded.
- **P2 option (upstream U4):** with ggplot-rs emitting `fill="var(--gg-bg,#fff)"`-style paints, a *single* SVG follows the page theme without re-rendering (thematic-style), and one exported HTML works in both modes.
- **Acceptance:**
  - e2e `scripts/showcase-e2e.mjs --theme dark` screenshots every tab.
  - An assertion that no panel SVG has a `#fff` background in dark.
  - Exported HTML opened standalone keeps the dark bg.

### T5 — Consistent series colours across a dashboard (P1, M)

- **Problem:**
  - Category colours are assigned by sorted order *per panel*, so "Berlin" can be blue in one chart and red in the next.
  - `look.rs` gives stable colours only to known model ids, and nobody can override it.
- **SQL:**

```sql
-- dashboard-wide colour map (a directive: CATEGORY key + COLORMAP value)
SELECT city::CATEGORY, hex::COLORMAP FROM (VALUES ('Berlin','#e11d48'),('Hamburg','#0b5fff')) t(city,hex);
-- or in the theme JSON: "series": {"Berlin": "#e11d48"}
-- or automatic: SELECT 'stable'::OPTS;  →  hash-stable colours (look.rs algorithm) for all levels
```

- **Implementation:**
  - `look.rs` maps model ids to **palette slots** (indices), not hex, so a theme swap keeps "ols = slot 0" but in the theme's colour.
  - The lookup order becomes `series` map (theme + `::COLORMAP`) → known-model slot → first-seen dashboard-wide assignment (browser keeps one `Map` per dashboard; headless the same in `dashboard.rs`) → hash.
  - `dz_scale` (`lib.rs:474`) calls the same resolver. A `"color_mode":"stable"|"order"` theme token chooses between this and the current per-panel order; the default stays `order` for back-compat.
- **Acceptance:** a 3-panel test where the same level gets the same colour; theme swap keeps slot identity; `#rrggbb` values still win.

### T6 — Fonts & density (P1, S)

- `font.family` and `font.size` go to every ggplot `ElementText` via `theme_update`. Upstream U2 adds a convenience `Theme::family()`; ggplot-rs 0.17.1 lacks a base family. The same font goes to the page CSS.
- `density` (`compact|comfortable|spacious`) scales the grid gap, card padding and base font size. These are **spacing tokens** (`--gap`, `--pad`) used by the browser, serve and the headless packer (`DashboardOptions.gap`).
- **Risk:** SVG text-width estimation depends on font metrics. Check ggplot-rs label layout with wider fonts (golden tests with `Inter` and `serif`).
- **Typography, including self-hosted fonts:**
  - Two families, `font.family` (body, tables, chart text) and `font.display` (dashboard title, panel titles, KPI values), replace the hard-wired Bricolage/Hanken (`web/index.html:9, 19-20`) and the CLI/serve `system-ui`.
  - `font.src` lists woff2 files next to the dashboard (or under `serve --assets`). They are served same-origin, so there is no Google-Fonts call in locked or offline mode.
  - The built-in themes default to a system stack (fast, private). The current Google fonts become the opt-in `datazoo-brand` look.
  - Exported HTML inlines `font.src` as `data:` URIs when the total is < 400 KB; otherwise it falls back to the system stack. The licence is the author's responsibility, noted in the docs.
  - Headless SVG writes `font-family` only. Rasterising the headless output (PNG/PDF) needs the font installed; the CLI warns if it is missing.

### T7 — Accessibility checks in `dashboard --check` (P1, S)

| Code | Rule |
|---|---|
| `theme/contrast-text` | ink vs card and bg < 4.5:1 (WCAG AA) |
| `theme/contrast-marks` | any palette colour vs card < 3:1 (WCAG 1.4.11), checked for light **and** dark |
| `theme/palette-cvd` | adjacent palette colours indistinguishable under simulated deuteranopia/protanopia (ΔE < 10). Hint: use `colorblind`. |
| `theme/unknown` | unknown theme name, bad JSON, unknown token (with "did you mean") |
| `theme/multiple` | more than one `::THEME` |
| `design/colour-only` (guide→lint) | category colour without legend or labels in `print`/`mono` |

Implemented in `src/lint.rs` with pure colour maths (relative luminance, CIEDE2000) and no new dependency.

### T8 — Whole-dashboard design tokens: page chrome follows the theme (P0, M)

- **Problem:**
  - About 250 colour literals in `web/index.html` bypass the tokens.
  - JS injects its own colours (sliders `web/app.js:1843-1864`, served nav `:1874-1893`, colorscale `:3243-3251`).
  - `serve.rs`, `bin/dashboard.rs` and the `dashboard.rs` SVG each hard-code another look (§1.2).
  - Today one theme cannot style charts and page together.
- **Token set** (CSS custom properties `--ax-*`, generated from `ResolvedTheme`; old names kept as aliases):

| Group | Tokens | Replaces |
|---|---|---|
| Surface | `--ax-bg`, `--ax-bg-pattern` (none\|dots\|gradient), `--ax-card`, `--ax-card-border`, `--ax-shadow`, `--ax-shadow-hover`, `--ax-radius`, `--ax-radius-sm` | page gradients `index.html:23-29`, `.panel` 191, radii 8–16px literals |
| Ink | `--ax-ink`, `--ax-muted`, `--ax-line`, `--ax-heading` | `#1f2430/#667085/#e6e9f1` in `dashboard.rs:218, 375, 457` |
| Accent | `--ax-accent`, `--ax-accent-soft` (derived 12% tint), `--ax-accent-ink` (contrast text on accent), `--ax-accent-bar` (solid\|gradient\|none) | the raw `rgba(42,157,143,…)`, `rgba(15,181,165,…)`, `--grad` |
| Semantic | `--ax-positive`, `--ax-negative`, `--ax-warning`, `--ax-info` | deltas `#0ca678/#e03131` (224, 238), badges 241-244, colorscale JS 3243 |
| Header | `--ax-header-bg`, `--ax-header-ink`, `--ax-logo-h` | `h1` 35-40 |
| Tabs | `--ax-tab-ink`, `--ax-tab-active`, `--ax-tab-indicator` (underline\|pill) | 305-316 |
| KPI | `--ax-kpi-value` (ink\|accent\|gradient), `--ax-kpi-size`, `--ax-kpi-caption` | gradient-clipped 220-224, SVG 34px |
| Callout | `--ax-callout-bg`, `--ax-callout-bar`, variants `info/tip/warn` | `.md-box` 204-205 |
| Table | `--ax-th-bg`, `--ax-th-ink`, `--ax-zebra` (transparent = off), `--ax-row-hover`, `--ax-row-selected`, `--ax-num-font` (tabular-nums) | 206-247, data bars `app.js:3027` |
| Inputs | `--ax-input-bg`, `--ax-input-border`, `--ax-focus-ring`, `--ax-slider` | 135, 169-178, 283-297, JS slider CSS |
| Overlay | `--ax-tip-bg`, `--ax-tip-ink`, `--ax-legend-off` | `.dp-tip` 318-325, `#c4c9d2` JS 4210 |
| Buttons | `--ax-btn-bg` (accent\|gradient\|neutral), `--ax-btn-ink`, `--ax-btn2-*` | 149-154, 259, 329-335 |
| Spacing | `--ax-gap`, `--ax-pad`, `--ax-font-size` (density) | grid gap 165, card padding |

- **Implementation:**
  1. Refactor `web/index.html` CSS to tokens only. A CI grep test allows hex literals only in `:root` and in the generated theme block.
  2. Move the JS-injected CSS into `index.html` on tokens.
  3. `serve.rs`, `duckext` serve and `bin/dashboard.rs` stop carrying their own `STYLE`. They `include_str!` one shared `web/theme-base.css` (structure, tokens only) plus `ResolvedTheme::css_vars()`.
  4. `dashboard.rs` SVG: `DashboardOptions { theme }` replaces `brand`/`background`, and every literal (296-299, 375-377, 402-413, 457-497) reads tokens.
- **Acceptance:**
  - The grep test passes.
  - Switching the theme changes bg, cards, header, tabs, KPIs, tables, inputs, tooltip and charts in **one** step (e2e screenshot diff on the showcase for each of `datazoo`, `dark`, `print`, a corporate fixture).
  - `datazoo` keeps today's pixels within a small tolerance.
  - Headless SVG and browser use identical card/ink colours (parsed-SVG vs computed-style test).

### T9 — Author branding (P0, S)

```sql
SELECT 'Sales cockpit'::DASHBOARD_TITLE;            -- <h1>, document.title, export title
SELECT 'ACME Analytics'::BRAND;                      -- replaces "anofox-visualization" in header/exports
SELECT 'assets/acme.svg'::LOGO;                      -- header logo (theme brand.logo / logo_dark for dark)
SELECT 'assets/favicon.svg'::FAVICON;
```

- All of these are also tokens in the theme `brand` object. The SQL directive wins.
- **Owner decision (resolved): the credit is mandatory.** "Powered by DataZoo GmbH" (a link to DataZoo GmbH) renders on every dashboard — browser, embed, exported HTML/PDF, headless dashboard SVG, `serve` and locked pages — under every theme. It follows the theme's colours and fonts so it stays readable (e.g. in dark mode), but its text and link never change. There is **no** `::CREDIT` role and no `brand.credit` token. Author branding (`::DASHBOARD_TITLE`, `::BRAND`, `::LOGO`, `::FAVICON`, themes) is shown *in addition to* the credit.
- The builder header `<h1>anofox-visualization</h1>` (`index.html:419`) shows `brand.name` only in embed and served mode. The authoring builder keeps the product name (see T12).
- `document.title` = `{DASHBOARD_TITLE}{title_suffix}`. The favicon is set as a `<link rel=icon>`; today there is none.
- Logos and favicons are resolved relative to the dashboard file and served same-origin; `data:` URIs are allowed. In locked mode they are read at startup, never from a viewer URL.
- Exports (`downloadHtml`, CLI HTML, headless SVG header) use the brand name and logo (inlined `data:`) and always carry the DataZoo credit, replacing the fixed "anofox-visualization dashboard" (`web/app.js:3610`, `src/bin/dashboard.rs:280-288`).

### T10 — Per-tab / per-section accents (P2, S)

```sql
SELECT 'Revenue'::TAB, '#0b5fff'::ACCENT;          -- tab-scoped --ax-accent (and palette[0])
SELECT 'Risk'::SECTION, 'warning'::ACCENT;         -- semantic token name or hex
```

- This sets `--ax-accent` on the tab pane or section container. Charts inside take it as `palette[0]` / brand, since the wasm render already gets `primary` per call (`src/wasm.rs:60-93`).
- Lint `theme/contrast-marks` runs per accent.
- It deliberately does not change the categorical palette, so series colours stay stable across tabs (T5).

### T11 — Print styles & PDF (P1, M)

- Fix the stale `@media print` (`web/index.html:276`, which targets `.layout`). Hide the sidebar, toolbar, editor, panel tools, filters (or print their current values as a "Filters: region = EU" line), and the served nav. The DataZoo credit always prints.
- Print forces the theme's `print` variant (or `print_variant` token): white, ink, no shadows or gradients, zebra kept. Charts are **re-rendered** with print tokens before `window.print()` (`beforeprint` event), so charts and page match.
- `@page { size: A4 landscape; margin: 12mm }` comes from `::PAGE_SIZE` (L6). A running header shows logo, title and date; the footer shows the credit and page number (CSS paged-media `@top-left` where supported, otherwise a fixed header).
- All tabs print sequentially, each starting on a new page (`break-before: page`). `break-inside: avoid` applies to `.panel` and `.group`.
- Acceptance: `showcase-e2e.mjs --print` uses `page.pdf()` and checks the page count, that there is no sidebar text, and a dark→print swap.

### T12 — Builder chrome vs dashboard theme (P1, S) — recommendation

- **Recommendation: the builder UI (sidebar, toolbar, editor, explore) stays neutral and product-branded.** It gets its own small `--ui-*` token set with only light and dark variants, following the OS or the 🌙 toggle.
- **Only the dashboard canvas (`.dash`) takes the dashboard theme.** The theme's CSS vars are scoped to `.dash` (and `:root` in embed or served mode).
- Reasons:
  1. The editor must stay readable regardless of a corporate palette.
  2. Previewing a dark dashboard inside a light builder is a feature.
  3. It matches Grafana and Superset, where the editor is product chrome.
- Embed / served / locked / export: there is no builder chrome, so the dashboard theme owns the whole page, including the served nav (move it from inline styles at `web/app.js:1874-1893` onto `--ax-*` tokens).
- The toolbar gets a theme preview dropdown (built-ins plus the dashboard's own) that does not edit the SQL. The 🌙 toggle switches the *dashboard's* light/dark variant, and the `--ui` chrome follows it.

---

## 4. Layout

| # | Item | Pri | Eff | SQL surface |
|---|---|---|---|---|
| L1 | Dashboard header: title, subtitle, logo, footer text (branding side: T9) | P0 | S | `SELECT 'Sales Q3'::DASHBOARD_TITLE;` `'…'::SUBTITLE` `'assets/logo.svg'::LOGO` `'© ACME'::FOOTER` |
| L2 | Section headings that don't break 2-up rows | P0 | S | `SELECT 'Revenue'::SECTION;` (+ optional description column `::MARKDOWN`). Always starts a new row (unlike `::LABEL`'s accidental break). |
| L3 | Row height control | P1 | S | `::HEIGHT` also takes `'sm'\|'md'\|'lg'\|'xl'\|'auto'` (tokens = 240/320/420/560px); rows equalise by default (`align-items:stretch`). `SELECT 'md'::ROWHEIGHT;` sets the default. |
| L4 | Headless parity: groups and sections in `dashboard`/static HTML | P1 | M | no new syntax. `dashboard.rs` draws `::GROUP` boxes with title and `::SECTION` headings; `::TAB` → stacked sections with tab name as heading (a report). |
| L5 | Sticky filter bar | P1 | S | `SELECT 'Filters'::FILTERBAR; …inputs…; SELECT 1::ENDGROUP;` (a `::GROUP` variant, `position:sticky`, collapses to a drawer on mobile) |
| L6 | Print/PDF page layout (styles: T11) | P1 | M | `SELECT 'A4 landscape'::PAGE_SIZE;` `SELECT 1::PAGEBREAK;`. Print CSS: `@page`, `break-inside:avoid` per panel, all tabs expanded, `print` theme variant, header/footer with title + date + page no. |
| L7 | Responsive spans | P2 | M | `::COL '12 md:6 xl:4'` (mobile-first; plain `6` keeps today's behaviour). Breakpoints are tokens sm 640 / md 960 / xl 1440. |
| L8 | Detail pane / drawer for drill-down | P2 | L | `SELECT 'Details'::DETAIL; …panels…; SELECT 1::ENDGROUP;`. Hidden until a click sets the selection variable, then opens as a right-side drawer (desktop) or sheet (mobile). |
| L9 | Dashboard-level small multiples (repeat) | P2 | L | `SELECT DISTINCT region::REPEAT FROM sales;` … `SELECT 1::ENDREPEAT;`. Each enclosed panel runs once per value with `getvariable('region')` bound. Cap via `max_repeat`. |
| L10 | Embedding | P2 | M | `?embed=1&panel=<n\|title>` renders a single panel; `<anofox-dashboard src="x.sql" theme="dark">` web component wrapping the wasm app; `serve` emits `frame-ancestors` from a flag. |
| L11 | Dashboard templates | P2 | S | `SELECT 'report'::TEMPLATE;` sets defaults: `report` = 1–2 cols, print-friendly, sections numbered; `wall` = dark, compact, no chrome, auto `::RELOAD 60`, large KPIs; `app` = today's default. A template is a theme overlay plus layout defaults. |
| L12 | Composition options for contract plots | P2 | S | macro options `ncol :=`, `legend :=` passed to `PlotGrid::ncol` / `legend_position` (`ggplot-rs compose.rs:308`). |

Notes:
- New layout directives are added only to `REGISTRY` (`src/roles.rs`), category `layout`/`chrome`, `renders=false`. They automatically show up in `--roles`, DOCS and wasm `roles_json()`.
- The `::GROUP`-is-always-full-width limitation (`dashboard-design.md` §7) is fixed by honouring `::COL` before `::GROUP` (P1, S). Then a group can sit beside a chart.

---

## 5. Customization

### C1 — One scalable per-panel options syntax: `::OPTS` (P0, M)

- **Recommendation:** do **not** add a role per knob. Add one `::OPTS` role.
  - Its value is `key=value` pairs separated by `;` (SQL-friendly, no quote escaping), or a JSON object.
  - Keys come from an **options registry** `src/options.rs` (`key`, type, allowed values, applies-to kinds, summary). As with `REGISTRY`, the parser, lint ("unknown key, did you mean …"), DOCS table, wasm `options_json()` and the skills all derive from it.
  - Existing roles (`::YFORMAT`, `::XSCALE`, `::FACET_NCOL`, `::ALPHA`, …) stay as aliases of option keys (`yformat=`, `xscale=` …), so there is no breakage.
  - Name: `::OPTIONS` is taken (alias of `::DROPDOWN`, `src/roles.rs:215`), hence `::OPTS` (alias `::CHART_OPTS`).
- **Scope:**
  - A column in the chart's statement applies to that panel.
  - A standalone `SELECT '…'::OPTS;` applies to the **next** panel (like `::COL`).
  - The theme's `defaults` object applies to all panels.
  - Precedence: theme defaults < standalone < in-statement < dedicated role.

```sql
SELECT month::XAXIS, region::CATEGORY, sum(rev)::LINECHART,
       'legend=bottom; ylim=0,; grid=y; subtitle=Net of returns; caption=Source: ERP' ::OPTS
FROM sales GROUP BY ALL ORDER BY month;

SELECT '{"legend":"none","palette":"colorblind"}'::OPTS;   -- next panel, JSON form
```

### C2 — Option keys (first wave P0/P1)

| Key | Values | Pri | Eff | Maps to |
|---|---|---|---|---|
| `legend` | `top\|bottom\|right\|left\|none\|inside:0.9,0.9` | P0 | S | `GGPlot::legend_position` (`ggplot-rs plot.rs:1787-1793`); replaces hard-coded `Top` (`src/lib.rs:2165…3108`) |
| `ylim` / `xlim` | `min,max` (either side empty = auto) | P0 | S | existing internal `with_limits` (`src/lib.rs:2106`). Named `*lim` because `::YMIN/::YMAX` are interval roles. |
| `ybreaks` / `xbreaks` | `0,25,50,100` or `n=5` | P1 | S | `with_breaks` |
| `subtitle`, `caption` | text | P0 | S | ggplot `labs(subtitle, caption)`; theme has the elements |
| `xlabel`, `ylabel` | text, or `none` | P0 | S | the same as macro `x_label/y_label` (`look.rs:36`), now for role charts |
| `grid` | `none\|x\|y\|xy` | P1 | S | `panel_grid_major_x/y` blank |
| `palette` | theme palette name or `#a,#b,…` | P1 | S | per-panel palette override |
| `colors` | `Berlin=#e11d48, Hamburg=#0b5fff` | P1 | S | per-panel series map (merges over T5 map); generalises gauge `::COLORS` |
| `order` | `data\|alpha\|value\|Berlin,Hamburg,…` | P1 | S | level order of CATEGORY/discrete x; today sorted (`lib.rs:474`) |
| `labels` | `on\|off\|inside\|outside`, `fmt` | P1 | S | generalises `::DATALABELS` |
| `yformat` / `xformat` | existing + **named formats** `eur`, `eur0k`, `pct1`, `int`, `compact`, `0.0`, `#,##0.00 €` | P0 | M | see C3 |
| `tooltip` | template `'{series}: {y:eur} ({x:date:MMM yy})'` | P2 | M | `web/app.js:3746-3862` tooltip builder |
| `stack`, `smooth`, `alpha`, `facet_ncol`, `xscale`, `yscale` | existing roles as keys | P1 | S | aliases |

### C3 — Locale-aware numbers and dates (P0, M) — de-DE

- **Problem:**
  - `NUM_LOCALE="en-US"` (`web/app.js:3298`) and Money KPI hard-coded `$` (`src/format.rs:102`).
  - Axis currency always prefixed (`src/lib.rs:2255-2285`).
  - ggplot-rs decimal mark is always `.` (`ggplot-rs scale/format.rs:43`).
- **SQL:** `SELECT 'de-DE'::LOCALE;` (or theme `locale`; `?locale=` viewer override in non-locked mode). `::MONEY` uses the locale's currency, or the theme `currency` token (`EUR`).
- **Behaviour (de-DE):**
  - `1234567.5` → `1.234.567,5`; `€` → `1.234,50 €` (suffix + NBSP); `percent` → `12,5 %`.
  - Dates `15.10.2026`, month names `Okt`.
  - Compact `1,2 Mio.`
- **Implementation:**
  - `src/format.rs` gets a small `Locale` table (decimal, group, currency pattern, % spacing, month names). Start with en-US, de-DE, de-CH, fr-FR and en-GB; no ICU dependency.
  - The browser uses `Intl.NumberFormat(locale)` for KPIs, tables and tooltips.
  - The axis formatters in `lib.rs` build closures from `Locale`. The `scales` helpers are used only where they suffice, because **upstream U1** (`decimal_mark` in `label_number`/`label_currency`, localisable `label_date`) is needed to stay on upstream helpers.
- **Acceptance:**
  - Unit table tests per locale (axis, KPI, table, tooltip).
  - Golden SVG with de-DE tick labels.
  - The same strings in browser and CLI for the same value (a shared fixture JSON consumed by Rust tests and a node test of `app.js` formatters).

### C4 — Reference bands & annotations from data (P1, M)

```sql
SELECT 90 AS lo::YBAND_LOWER, 110 AS hi::YBAND_UPPER, 'target corridor' AS t::LABEL …;   -- horizontal band
SELECT launch_date::XLINE, 'Launch'::LABEL FROM events;    -- labelled vertical line (label = text)
SELECT x::XAXIS, y::YAXIS, note::ANNOTATE FROM notes;      -- free text at (x, y)
```

- `annotate_text`, `annotate_rect` and `annotate_segment` exist upstream (`ggplot-rs plot.rs:1923-1939`).
- `::REFLINE`/`::XLINE` with a `::LABEL` column draw the label at the line end.
- Colours come from the theme's `muted`/`warning` tokens.

### C5 — Conditional formatting & thresholds (P1, M)

```sql
SELECT region, margin::PERCENT, '<0:negative; 0..0.05:warning; >0.05:positive'::THRESHOLDS FROM t;
```

- Applies to table cells, KPI tiles (value colour) and gauges (zones). It generalises `::COLORS`/`::LABELS` for gauges.
- Uses theme semantic colours, so the result is dark-mode and colour-blind safe. An icon (▲●▼) is added for the colour-only rule.

### C6 — Extension points for other anofox extensions (P1, S)

- **Problem:** anofox-forecast and anofox-statistics want consistent styling (train/test/future colours, model colours) without owning a theme.
- **Proposal:**
  - Contract-level defaults live in the theme under `"contract": { "split": {"train":"slot:0","test":"slot:2","future":"slot:5"}, "models": {"ets":"slot:3"} }`.
  - Values are **palette slots**, not hex, so they follow the theme.
  - An extension may ship a default fragment via a table macro `anofox_viz_style()` that returns JSON. `anofox_plot_*` merges it under the user theme. Precedence: built-in < extension fragment < user theme.
  - Documented in [integration-contract.md](integration-contract.md).

### Explicitly *not* planned

- **Free-form custom CSS:** XSS/CSP risk in served and locked mode, and it breaks headless parity. Tokens cover the need. Revisit only as `serve --css file` (operator-owned, never from dashboard SQL).
- Dual y-axes and 3D (`dashboard-design.md` §3).
- A GUI theme editor. At most a docs page that previews the built-in themes.

---

## 6. Cross-cutting

| Topic | Plan |
|---|---|
| Backwards compatibility | The default theme `datazoo` stays byte-identical (golden SVG on all `examples/` + showcases). `brand`/`set_brand`/`?primary=` map to `accent`/`palette[0]`. Existing roles become aliases of option keys. `::LABEL`-as-heading unchanged (`::SECTION` is new). Mark the deprecations in DOCS but do not remove them. |
| Registries & docs | New roles → `REGISTRY` (`src/roles.rs`). New `src/options.rs` → `--options` / `--options-md`, a DOCS table kept in sync by a test (as `markdown_table` does for roles). Built-in themes → `--themes` (name, swatches, contrast scores) and a generated DOCS section with SVG swatches. |
| Lint (`src/lint.rs`) | `theme/*` (T7); `opts/unknown-key`, `opts/bad-value`, `opts/not-applicable` (e.g. `legend` on a KPI); `layout/pagebreak-in-group`; `locale/unknown`; `design/colour-only`. |
| Skills (`plugins/anofox-visualization/skills/`) | `-dashboards`: `::THEME`, `::LOCALE`, `::SECTION`, `::OPTS`, header roles, templates. `-charts`: option keys table (generated from `options.rs`). `-serving`: `--theme`, locked-mode theme freezing, `?theme=`. `-statistics-plots`: `theme :=` macro option, `SET VARIABLE anofox_theme`. Update `docs/dashboard-design.md` §4–5 (accessibility now partly lintable). |
| Tests | (0) Token-only CSS grep test (no hex outside `:root`/generated block in `web/index.html`, `serve.rs`, `bin/dashboard.rs`, `dashboard.rs`). (1) Golden SVG *structure* per built-in theme: background fill, text fill, palette order, legend position, from parsed SVG rather than bytes. (2) Locale unit fixtures shared Rust↔JS. (3) Contrast/CVD unit tests. (4) sqllogictest for `::THEME`/`::OPTS`/`::LOCALE` and macro `theme :=`. (5) `scripts/showcase-e2e.mjs` gains `--theme` and `--locale` matrices with screenshots per tab, and fails if a dark-theme panel has a white background. (6) Print-CSS screenshot (`emulateMedia('print')`). |
| Upstream ggplot-rs asks | **U1** `decimal_mark` in `label_number`/`label_currency` + locale month names in `label_date` (P0 need, S). **U2** `Theme::family(&str)` / base family on `theme_*_base` (P1, S). **U3** contrast-aware `theme_dark_base` preset variant without grey panel (P2, S; can be done locally). **U4** opt-in CSS-variable paints in `render_svg_native` (P2, M). |
| Security | Theme JSON is data: validate hex/numbers, reject `url(`/`javascript:` and unlisted font URLs. Locked serving freezes the theme at start. `?theme=` accepts only variant names. |

---

## 7. Prioritised backlog (top items)

| # | Item | Pri | Eff | SQL surface |
|---|---|---|---|---|
| 1 | Whole-dashboard theme model: tokens → ggplot theme + CSS vars for charts **and** chrome, `datazoo` default unchanged (T1, T8) | P0 | M–L | spec `"theme"`; `dashboard/serve --theme` |
| 2 | Theme selection/definition (T3) | P0 | S | `SELECT 'acme+dark'::THEME;` `SELECT content::THEME FROM read_text('acme.json');` `SET VARIABLE anofox_theme='dark'` |
| 3 | Dark mode for charts + dark-correct export (T4) | P0 | M | `?theme=dark`, `prefers-color-scheme` |
| 4 | Locale numbers/dates, € suffix (C3) | P0 | M | `SELECT 'de-DE'::LOCALE;` |
| 5 | `::OPTS` + options registry (C1) | P0 | M | `'legend=bottom; ylim=0,'::OPTS` |
| 6 | Legend / limits / subtitle / caption / axis labels (C2 first wave) | P0 | S | `legend=`, `ylim=`, `subtitle=`, `caption=`, `ylabel=` |
| 7 | White-labelling + header + sections (T9, L1, L2) | P0 | S | `::DASHBOARD_TITLE ::BRAND ::LOGO ::FAVICON ::SECTION` |
| 8 | Built-in `print`, `colorblind`, `high-contrast` themes + contrast lint (T2, T7) | P1 | S | `SELECT 'colorblind'::THEME;` `dashboard --check` |
| 9 | Stable series colours across a dashboard (T5) | P1 | M | `SELECT k::CATEGORY, hex::COLORMAP FROM …;` theme `"series"` |
| 10 | Print styles/PDF + headless groups/sections (T11, L6, L4) | P1 | M | `'A4 landscape'::PAGE_SIZE`, `::PAGEBREAK` |
| 11 | Thresholds / conditional formatting (C5) | P1 | M | `'<0:negative; >0.05:positive'::THRESHOLDS` |
| 12 | Annotations from data (C4) | P1 | M | `::YBAND_LOWER/UPPER`, labelled `::XLINE`, `::ANNOTATE` |
| 13 | Typography (self-hosted fonts) & density tokens (T6); builder chrome stays neutral (T12) | P1 | S | theme `font.family/display/src`, `density` |
| 14 | Sticky filter bar, row heights (L5, L3) | P1 | S | `::FILTERBAR`, `'md'::HEIGHT`, `::ROWHEIGHT` |
| 15 | Responsive spans, detail drawer, repeat, embed, templates (L7–L11) | P2 | M–L | `::COL '12 md:6'`, `::DETAIL`, `::REPEAT`, `::TEMPLATE` |
| 16 | Tooltip templates, CSS-var SVG (C2, U4), per-tab accents (T10) | P2 | M | `tooltip='{series}: {y:eur}'`, `'Risk'::TAB, 'warning'::ACCENT` |

## 8. Roadmap

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M1 — Theme foundation, chrome & locale** (≈3–4 wk) | T1, T8 (chrome tokens, shared `theme-base.css`), T3, T4, T9, C3, light/dark built-ins, `dashboard/serve --theme`, upstream U1 | Golden default unchanged; token-only CSS grep test; dark e2e green (charts + chrome); a branded fixture shows the author's title/logo/brand **and** the DataZoo credit, and a test asserts the credit is present in every output under every built-in theme; de-DE fixtures identical in browser and CLI; DOCS + skills updated |
| **M2 — Customization & structure** (≈3 wk) | C1, C2 first wave, L1, L2, T2 rest, T6, T7 lint, T12, T5, L3, L5, L4, C6, upstream U2 | `--options` table generated; every option key has a render test; contrast lint on all examples; showcase uses header + sections + colormap |
| **M3 — Reports & advanced layout** (≈3–4 wk) | T11 + L6 print/PDF, C4, C5, T10, L7–L12, tooltip templates, U4 CSS-var SVG | Print screenshots A4; `report` and `wall` templates as examples; embed demo; single exported HTML works in light and dark |

**M1 status (branch `feat/theme-m1`):** implemented — T1, T8 (`web/theme-base.css`, token-only CSS test), T3, T4, T9 (author branding; the DataZoo credit is mandatory), T12 (neutral builder chrome), C3 (format::Locale + ggplot-rs 0.18 `GGPlot::locale`), built-ins `light`/`dark`/`print`/`colorblind`/`high-contrast`, `dashboard`/`serve --theme`, `dashboard --svg`, `anofox_serve_dashboards` `options.theme`, lint `theme/*`, `locale/unknown`, `chrome/bad-url`. Not in M1: the toolbar theme-preview dropdown (T12), `print` re-render on `beforeprint` (T11), font self-hosting (T6), headless groups/tabs (L4).

## 9. Open questions for the owner

1. **Default look:** keep the DataZoo palette as `datazoo` default, or switch the default to `colorblind` (Okabe-Ito) for new dashboards (an opt-in flag for old ones)?
2. **Syntax:** is `::OPTS 'k=v; …'` acceptable as *the* extension mechanism, or do you prefer dedicated roles for the top 5 knobs (`::LEGEND`, `::YLIM`, …) as well? This plan proposes the dedicated roles only as aliases.
3. **Corporate themes:** file (`read_text`) vs table vs a registered name (e.g. `anofox_register_theme('acme', json)` persisted in the DB)? Which one should locked serving support?
4. **Viewer control:** may viewers of a locked dashboard switch light/dark (`?theme=`) and locale (`?locale=`), or is the author's choice binding?
5. **Locale default:** dashboard author's `::LOCALE`, else browser `navigator.language`, else `en-US`? Or always `en-US` unless set, for reproducible exports?
6. **Stable colours by default:** turn on dashboard-wide stable category colours (T5) by default (visual change), or keep per-panel order unless `color_mode=stable`?
7. **PDF:** is browser print enough, or do we need a headless `dashboard --pdf` (adds a Chromium dependency, or an SVG→PDF crate)?
8. **Custom CSS:** confirm "no CSS from dashboard SQL"; is an operator-level `serve --css` wanted?
9. **Fonts:** which corporate fonts must work offline in exported HTML (embedding and licensing)?
10. **Upstream:** OK to file U1/U2 (and U4 later) against ggplot-rs, and pin the next minor version in M1?
11. ~~**Credit / white-labelling:** may any dashboard hide "Powered by DataZoo GmbH"?~~ **Resolved (owner):** no — the credit is mandatory in every output and under every theme; there is no `::CREDIT` role. Authors add their own branding alongside it. The builder itself stays product-branded (T12).
12. **Builder chrome:** agree that the editor/sidebar stay neutral (T12) and only the dashboard canvas follows the theme?
