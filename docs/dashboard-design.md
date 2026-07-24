# Dashboard design principles

The rules that make a dashboard *good*, not just *correct*. This is the single
source of truth cited by the `build-dashboard` skill and enforced (the checkable
subset) by `dashboard --check`. Rules are tagged **[lint]** (a `design/*` check
flags violations) or **[guide]** (judgement, not machine-checkable).

Distilled from Nielsen Norman Group (scanning patterns, visual hierarchy),
Edward Tufte & Stephen Few (data-ink, small multiples, single-screen), Cole
Nussbaumer Knaflic (*Storytelling with Data*), and Datawrapper.

---

## 1. Layout & information hierarchy

- **Inverted pyramid.** Order panels summary → trend → detail: a **KPI strip**
  first, then the **one primary chart** that answers the main question
  (full-width), then supporting **breakdowns** (2-up), then a **detail table**
  last. A reader must grasp the headline in ~5 seconds. **[guide]**
- **Most important, top-left.** Eyes enter top-left and scan in an F/Z pattern —
  put the highest-stakes number/chart there, not a filter or a logo. **[guide]**
- **≤ ~7 panels per view.** Past that, a page can't be scanned — split topics
  across `::TAB` pages. **[lint: too-many-panels]**
- **2-up is the default; full-width earns it.** Panels default to half-width;
  reserve `::COL 12` for time series, heatmaps, and wide multi-series scatter.
  Two related charts side by side beat two stacked. **[guide]**
- **Group what belongs together** in a `::GROUP` box (or a `::TAB`); leave
  breathing room between unrelated blocks. **[guide]**
- **Don't dump raw rows.** `::TABLE` paginates the view but loads every row into
  the browser; a few hundred rows is a heavy payload and usually a raw dump — cut
  to a top-N or aggregate (`::PAGED` only for very large / remote sources).
  **[lint: raw-table]**

## 2. KPI tiles

- **One strip, not scattered cards.** Wrap headline numbers in
  `::GROUP … ::ENDGROUP` so they render as a compact band. The browser
  auto-groups a run of bare tiles, but the static renderer does not — group them
  explicitly. **[lint: ungrouped-kpis]**
- **≤ 6 tiles per strip**, ordered by business priority (revenue before volume),
  not alphabetically. Beyond 6, move the rest into a detail panel. **[guide]**
- **Every tile = value + caption.** Use `::METRIC`/`::MONEY`/`::PERCENT`/
  `::COMPACT` for the number and a `::LABEL` for the caption; add `::DELTA` for a
  trend arrow vs a prior period. A number with no label is noise. **[guide]**

## 3. Chart selection

Pick the chart from the *question*, grounded in the column's cardinality
(`dashboard --describe`):

| The question | Chart |
|---|---|
| rank / compare categories | **sorted** `::BARCHART`; long labels → `::FLIP` (horizontal) |
| a value over time | `::LINECHART` (≤ ~6 series); `::AREACHART` for cumulative |
| part-to-whole | `::BARCHART_STACKED` / `_PERCENT`; `::DONUTCHART` only if ≤ 5–6 slices |
| distribution of a number | `::HISTOGRAM`, or `::BOXPLOT` split by a category |
| relationship of two numbers | `::SCATTER` (a 3rd measure → `::BUBBLE`) |
| two categories + a measure | `::HEATMAP` |
| progress to a goal | `::GAUGE` + `::RANGE` |
| geography (WKT) | `::MAP` |
| row-level detail | `::TABLE` (`::PAGED` when large) |

- **Sort bars by value** (`ORDER BY <measure> DESC`) unless the x-axis is
  temporal or a genuine ordinal — an unsorted ranking makes the reader do the
  work. **[lint: unsorted-bars]**
- **No pie/donut past ~6 slices** — angles that close are impossible to compare;
  use a sorted bar. **[lint: pie-slices]**
- **No 3D, no dual y-axes** — both distort comparison. Not offered by the
  framework; don't fake them. **[guide]**

## 4. Visual clarity

- **Title every chart with the takeaway.** `::TITLE 'Revenue up 12% YoY'` beats
  `'Revenue by month'`. An untitled chart makes the reader guess. **[lint: untitled-chart]**
- **Format the axis.** Money/percent axes get `::YFORMAT '€'` / a `_PERCENT`
  kind — bare numbers read as noise. **[guide]**
- **Direct labels ≤ 5 series; legend beyond.** Keep the legend out of the plot.
  **[guide]**
- **One accent, mute the rest.** Colour should highlight the point, not
  decorate every series. **[guide]**

## 5. Colour & accessibility

- **≤ ~7 categorical colours.** Past that the legend is unreadable — keep the top
  few and bucket the rest as "Other", or use small multiples. **[lint: many-series]**
- **`::CATEGORY` is discrete.** Band a continuous value into a `CASE`
  (`'low'/'mid'/'high'`) before colouring — a raw continuous column makes an ugly
  per-value legend. **[guide]**
- **Palette by data type:** sequential (light→dark) for ordered magnitudes,
  diverging for a meaningful midpoint (target, profit/loss), categorical for
  unordered groups. **[guide]**
- **Never colour alone.** Pair it with position, label, or shape so the chart
  survives grayscale and colour-blindness. **[guide]**

## 6. Anti-patterns (what `--check` flags, and the fix)

| Smell | Fix | Code |
|---|---|---|
| pie/donut with > 6 slices | sorted horizontal bar | `design/pie-slices` |
| bars in arbitrary order | `ORDER BY <measure> DESC` | `design/unsorted-bars` |
| chart with no title | `::TITLE '<takeaway>'` | `design/untitled-chart` |
| > 7 colour series | top-N + "Other", or small multiples | `design/many-series` |
| > 8 panels, no tabs | split with `::TAB` | `design/too-many-panels` |
| KPI tiles scattered | wrap in `::GROUP … ::ENDGROUP` | `design/ungrouped-kpis` |
| 100s of rows in one `::TABLE` | top-N or aggregate (`::PAGED` for remote) | `design/raw-table` |

Run `dashboard --check <file.sql>` to see these on your own dashboard; add
`--strict` to make them fail CI, `--no-design` to silence them. A clean,
well-composed reference dashboard lives at `examples/dashboards/sales-overview.sql`.
