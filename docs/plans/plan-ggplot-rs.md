# Plan: ggplot-rs (upstream)

0.16.0 (being released now): XSS-safe SVG, no hangs/panics on degenerate data,
rlib-only + optional plotters (minimal tree: 3 crates), wasm bindings split
out, linear-time categories, `data-series`/`data-value`/`data-domain`,
positioned fragments, warnings API, candlestick/OHLC, `coord_radar`,
`CoordPolar::with_span` (gauges), `geom_calendar`, resolution-sized tiles,
±Inf rect bounds, theme/primary-colour fixes, sorted discrete palettes.

## 0.17 — Statistical graphics ("regression release")

| # | Item | Why (consumer) |
|---|---|---|
| U1 | Data-mapped `geom_hline/vline/abline` (`yintercept`, `xintercept`, `slope`, `intercept` aesthetics; per facet/group) | per-model zero lines, ACF bounds, identity line |
| U2 | `stat_qq(distribution = normal\|t(df)\|exp\|halfnormal)`, `stat_qq_line`, `stat_qq_band` (pointwise/KS envelope, as qqplotr) | QQ of residuals with envelope |
| U3 | `geom_text_repel` / `geom_label_repel` (deterministic force layout, max-overlaps, seed) | label top-k influential points |
| U4 | Cook's-distance contour helper (`stat_cooks_contour(p, levels = [0.5, 1])`) on top of `stat_function` | residuals-vs-leverage panel |
| U5 | Composition: patchwork-style `PlotGrid` for the native SVG path (`a \| b`, `a / b`, shared title/caption, panel tags a–d, collected legend, per-cell widths) built on `render_svg_native_at` | 2×2 diagnostics as one SVG |
| U6 | `geom_smooth(method = "glm", family = binomial(logit)\|gamma\|negbin)`, `se` bands — via the `regression` feature, **pinned to the same anofox-regression version as anofox-statistics** so SQL fits and plotted smooths agree numerically | GLM fit lines, binned residuals |
| U7 | Step ribbon (`geom_ribbon(direction = "hv")`), censor marks (`geom_point(shape = '+')` helper), `stat_ecdf` DKW band, `geom_errorbarh` | Kaplan–Meier, ECDF, horizontal CIs |
| U8 | `geom_pointrange` dodge for multi-model forests; `position_dodge2` with `reverse` | multi-model coefficient forest |
| U9 | `ggpubr` brackets from a precomputed table (`geom_bracket(data = test_table)` with `group1/group2/p_adj` columns, `label = "p = {p_adj}"`) — no recomputation | plotting `test` contract output |

## Contract helpers (optional feature `autoplot`)

A small module that knows the [integration contract](integration-contract.md)
column names and returns ready `GGPlot`s from a `DataFrame`:
`autoplot_terms`, `autoplot_obs` (2×2, uses U1–U5), `autoplot_prediction`,
`autoplot_curve` (by `curve_type`), `autoplot_summary`, `autoplot_test`.
anofox-visualization's `anofox_plot_*` macros then become thin wrappers, and
the browser (wasm) builder gets the same plots for free. Golden SVG-structure
tests per helper.

## Quality / process

- Proptest/fuzz target over random `Value` columns for every geom:
  no panic, terminates, well-formed XML, no `NaN`/`inf` attributes, all
  attributes escaped (extends `tests/robustness.rs`).
- CI: `cargo tree` budget for the minimal feature set (already added),
  wasm size budget for `crates/ggplot-rs-wasm`, MSRV job.
- Follow-ups from the 0.16 work: data-space `data-domain` for transformed
  scales; `#rrggbb` level names as literal colours; proper guides for plain
  `coord_polar`; NaN sanitising in the canvas backend; discrete-x jitter (map
  categories before position adjustment); fix the two broken doc links
  (`build.rs:28`, `plot.rs:364`) and run `cargo doc -D warnings` in CI.
