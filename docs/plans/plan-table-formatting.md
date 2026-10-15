# Plan: table formatting

Status (2026-10-15): **T1–T4 done** in the browser (humanised headers, two-line
headers, inner sort icon; double-ruled totals with a legend, means in italics;
header rule, density toggle, numeric widths, key-column emphasis; minus sign,
muted zeros, per-column `::MONEY` / `::COMPACT` units, signed trends with a flat
band). Still open from T1–T4: units in the header, `::COLGROUP`, sticky totals in
Pages view, zebra off with bars, `chrome.negatives` / `chrome.zero` options.
Originally a proposal Follows the table rebuild (toolbar, totals, Scroll
view, `::ROWSELECT`, SQL-side filters/totals for `::PAGED`). Goal: tables that
read like a well-set report — clear headers, an unmistakable totals row, quiet
structure — with sensible defaults and a few opt-in controls. Every step is
browser-only (`web/app.js`, `web/theme-base.css`) unless noted; the static CLI
HTML / headless SVG table gets the same rules in a later step (T9).

## T1 · Clearer column names (default on)

Today the header is the raw column name (`load_pct`, `n0`, `avg_mae`).

- **Humanise raw names** when no `AS "Header"` alias is given: `snake_case` /
  `camelCase` → words, first letter capitalised, known abbreviations kept or
  expanded (`pct` → `%`, `avg` → `Avg`, `qty` → `Qty`, `id` → `ID`, `mae` →
  `MAE`): `load_pct` → "Load %", `avg_mae` → "Avg MAE". An explicit alias is
  never touched.
- **Units in the header, not in every cell**: a `::MONEY` column shows the
  currency once in the header ("Revenue, $") when every cell uses the same unit
  and the number is compact enough; cells then show plain numbers. Same for `%`.
- **Two-line headers** for long names: wrap at a word boundary (max 2 lines)
  instead of widening the column; full name in the tooltip.
- **Header alignment** follows the column (right for numbers — done), and the
  sort icon sits on the *inner* side so right-aligned headers don't jump.
- Opt-in grouping row: `::COLGROUP 'Backtest: ES | SN'` spans a label over
  adjacent columns (e.g. "SeasonalES" over MAE / RMSE) — a thin rule under the
  group label.

## T2 · Totals that stand apart (default on when shown)

- **Double rule above the totals** (the accounting convention): a 2 px rule in
  `--ax-ink` at 70 % (`border-top: 3px double`), no zebra, card-tinted
  background, bold numbers; the "TOTAL" label in small caps.
- **Σ / ⌀ markers** stay but move into the header tooltip + a legend line under
  the table ("Σ sum · ⌀ mean over 2,500 rows"), so the cells show clean numbers.
- **Sticky totals** in Pages view too (`position: sticky; bottom: 0` inside the
  scroll container), so a long page keeps its total in view.
- **Subtotals** (opt-in, `::SUBTOTAL` on a grouping column): when sorted by that
  column, a subtotal row after each group, with a single rule above it and a
  lighter background than the grand total.
- **Explicit aggregate per column** (opt-in): `::TOTAL 'sum' | 'mean' | 'median'
  | 'min' | 'max' | 'count' | 'none'` beside a column overrides the heuristic
  (amounts and counts → Σ, rates/shares/scores → ⌀). For `::PAGED` the SQL
  statistics request gains `median` / `min` / `max` per column.

## T3 · Quiet structure, clear hierarchy

- **Rules, not boxes**: hairlines between rows only; no vertical lines. A
  stronger rule under the header (1.5 px), the double rule above totals (T2).
- **Density** toggle in the toolbar (Comfortable 37 px / Compact 29 px rows),
  remembered per table; Compact is the default above 50 rows.
- **Zebra** stays theme-controlled (`chrome.table_zebra`), but off by default
  when in-cell bars are shown (bars already guide the eye).
- **First column emphasis**: the key column in 600 weight; a numeric key (rank)
  in muted tabular figures.
- **Column widths**: numeric columns shrink to content (`width: 1%` +
  `white-space: nowrap`), text columns take the rest; a max width with ellipsis
  for text outside Scroll view only when > 60 ch (tooltip shows all).

## T4 · Numbers that read at a glance

- **Negative numbers**: a true minus sign (−) and, opt-in per theme
  (`chrome.negatives: "red" | "parens"`), red or accounting parentheses.
- **Zero** as a muted "0" (or "–" with `chrome.zero: "dash"`), so non-zero
  values stand out.
- **Compact per column**: when a column's values span > 10⁶, use the same unit
  for all cells (all "M" or all "K", never mixed "$4M" / "$5.9M") — fixes the
  current mixed precision of `::MONEY` (decimals fixed per column, like plain
  numbers already are).
- **Deltas** (`::TREND`): a "+" sign on positive values, unit from the header
  (`%` / `pp`), and an optional neutral band (`|x| < 0.5` → grey, no arrow).

## T5 · Conditional emphasis

- `::COLORSCALE` gains a direction: `::COLORSCALE_HIGH_GOOD` (green high) vs the
  current low-is-good; and a diverging mode around 0 for deltas.
- **Thresholds**: `::FLAG '> 0.98'` (or a `::REFLINE`-style value) marks cells
  outside a target with the warning colour + an icon, so status never relies on
  colour alone (accessibility rule from the dataviz guide).
- **Top / bottom n** highlight in a column (`::HIGHLIGHT 'top 3'`).

## T6 · Header statistics, visible on demand

- The tooltip stats (min / max / mean / empty / distinct) become an optional
  **summary strip** under the header (toolbar → "Column stats"): a mini
  histogram (numbers) or top-3 values with shares (categories) per column —
  computed client-side for `::TABLE`, by the SQL statistics request for `::PAGED`
  (extend it with a 10-bin histogram: `histogram(…)` / `width_bucket`).

## T7 · Accessibility and print

- Real `<caption>` (the panel title) and `scope="col"` on headers; `aria-sort` on
  the sorted header; row selection exposes `aria-selected`.
- Status never colour-only: badges keep their dot + text; trends keep the arrow.
- **Print / PDF**: the toolbar, filter row, check marks and pager hidden; all
  rows of a client table printed (not just the page); totals repeated at the end;
  header repeated on every printed page (`thead { display: table-header-group }`).

## T8 · Export follows the formatting choice

- CSV keeps raw values (machine-readable) — unchanged.
- **Excel** (`::DOWNLOAD_XLSX` and a toolbar entry): number formats, the bold
  totals row with its double rule, frozen header — so the sheet looks like the
  table.

## T9 · One table renderer for every output

The CLI's static HTML and `dashboard --svg` draw tables in Rust (`src/page.rs`,
headless SVG). Move the formatting decisions (column profile, decimals, units,
totals rule, humanised names) into `src/format.rs` / a `table` module, expose
them through wasm, and have both the browser and the static renderers use them —
the same split as `page_sql` (one builder, every output).

## Order and size

| Step | What | Size | Default |
|---|---|---|---|
| T2 | double-ruled, sticky totals; Σ/⌀ legend | S | on |
| T1 | humanised names, units in header, 2-line headers | M | on (alias wins) |
| T3 | rules, density toggle, widths | S | on |
| T4 | minus sign, per-column compact units, deltas | M | on |
| T5 | colour-scale direction, thresholds, top-n | M | opt-in roles |
| T2b | `::TOTAL`, `::SUBTOTAL` (+ SQL median/min/max) | M | opt-in |
| T6 | column-stats strip (+ SQL histogram) | M | opt-in |
| T7 | a11y + print | S | on |
| T8 | formatted Excel export | M | — |
| T9 | shared Rust table formatter for CLI / SVG | L | — |

Every step: a sample in "Formatting & paging", the e2e (`showcase-e2e.mjs`) in
light and dark, a sqllogictest / unit test for any new role, and the DOCS
"Tables" section.
