# Golden baseline — S&P 500 dashboard

Reviewed-good renders of `../sp500.sql`, one per `::TAB`. They are the visual
baseline: what the dashboard is *supposed* to look like after passing both gates
(`dashboard --check` for structure, [`docs/dashboard-visual-review.md`](../../../docs/dashboard-visual-review.md)
for appearance).

Use them to catch appearance regressions after a renderer or dashboard change —
re-shoot, then eyeball (or have a vision agent) diff against these.

## Regenerate

```sh
scripts/shoot.sh --dash examples/sp500-dashboard/single --init init.sql \
  --workdir examples/sp500-dashboard \
  --tabs "Market overview,Sector explorer,Valuation & screens" \
  --out examples/sp500-dashboard/golden
```

(Needs the built extension at `/tmp/anofox_visualization.duckdb_extension` and a
headless Chrome.)

## What "good" looks like here

- KPIs read `503` / `53.8T` / `1.92` — locale-stable, no `53,8 Bio.`.
- 11 sectors carry 11 distinguishable colours (no repeated hue).
- Flipped-bar category names are whole (not clipped to "on Technology").
- Box-plot sector labels are vertical and legible (no overlapping smear).
