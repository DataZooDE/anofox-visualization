# What do we order this week, within budget, to sell the most over the next three weeks?

A cross-extension use-case dashboard: **forecast → optimize**.
[anofox-forecast](https://github.com/DataZooDE/anofox-forecast) forecasts
weekly demand per SKU, [anofox-optimize](https://github.com/DataZooDE/anofox-optimize)
picks the pallets to buy under the week's purchasing budget. It's the
"Stored results: forecast → optimize" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Optimize; the page says both extensions are used).

**Who asks:** the category buyer of a made-up cash & carry wholesaler, placing
the weekly order on Monday 19 Oct 2026 for 12 SKUs. Lead time 2 weeks + review
period 1 week, so the order covers three weeks — while summer lines (charcoal,
ice tea, sunscreen) fade and Christmas lines (mulled wine, gingerbread,
candles) ramp up. Today the buyer orders 3 × the last four weeks' sales and,
when the budget runs short, funds the top sellers first.

**The story (€25,000, 95 % service level):** today's rule serves an expected
80.7 % of the next three weeks' demand; the forecast with the same greedy rule
89.2 %; the forecast with `opt_knapsack_exact` 94.7 %, with €5,400 less stock
left on the shelf. 99 % service needs €32,500.

## How it is computed

1. **Forecast (anofox-forecast):** `ts_forecast_by(…, 'MFLES', 8, '1w',
   MAP{'seasonal_period': '52'})` on 104 weeks of sales per SKU. A backtest
   (the same model from 8 weeks earlier, against the 4-week average) gives each
   SKU's relative error, which sets the planning uncertainty (σ = error ×
   forecast). The model's own intervals assume one error for the whole year —
   far too wide for an off-season line — so the dashboard shows the
   backtest-based 90 % band instead.
2. **Need:** order-up-to level = forecast over the 3 covered weeks + z·σ at
   the service level (90 / 95 / 98 %), minus stock on hand and on order, in
   whole pallets.
3. **Value:** each pallet is worth margin × the extra units it is expected to
   sell (E[min(D, x)] for normal demand) — diminishing per extra pallet.
4. **Choose (anofox-optimize):** under budgets from €10,000 to €60,000:
   * today: last-4-weeks quantities, `opt_knapsack_greedy_value`,
   * forecast quantities, `opt_knapsack_greedy_value`,
   * forecast quantities, `opt_knapsack_exact` (proven optimum).
   Every plan is judged with the forecast: fill rate, expected margin, lost
   sales, stock left after 3 weeks.

## Stored results, not live

anofox-optimize has no loadable DuckDB-Wasm build yet (see
[../optimize-showcase/README.md](../optimize-showcase/README.md)), so both
steps run natively and the dashboard reads the results. The dashboard file
writes the extension names hyphenated and doesn't name the forecast macro: the
browser builder loads the forecast extension whenever a statement mentions it,
and the stored results don't need it.

| File | Role |
|---|---|
| `compute.sql` | Synthetic sales (hash-based noise, no files), the forecast and backtest, needs, pallet lots and the budget sweep. Tables `ir_*`. |
| `../../scripts/gen-optimize-cases.sh replenishment` | Runs `compute.sql` natively and writes `ir_skus`, `ir_sales`, `ir_forecast`, `ir_backtest`, `ir_need`, `ir_lots`, `ir_plan`, `ir_summary` to `web/data/optimize-replenishment/*.csv` (≈ 290 KB), plus `provenance.csv` with both extensions' versions. |
| `showcase.sql` | A link to `web/examples/optimize-replenishment.sql`, the dashboard. |

```sh
duckdb -unsigned -c "INSTALL anofox_optimize FROM 'https://get.erpl.io'; INSTALL anofox_forecast FROM 'https://get.erpl.io'"   # once
DUCKDB=duckdb scripts/gen-optimize-cases.sh replenishment
cd web && ../target/release/dashboard --check --strict examples/optimize-replenishment.sql
```

## Inputs and tabs

Inputs: purchasing budget (snapped to €2,500 steps), target service level,
plan (compared with today's rule) and the SKU to drill into.

1. **Forecast** — a year of sales and the 8-week forecast with its band;
   forecast vs. 3 × the last four weeks for every SKU.
2. **Accuracy** — the backtest: actual vs forecast vs 4-week average, error
   per SKU.
3. **Order plan** — pallets per SKU, today vs the plan; per-SKU fill rates,
   subtotalled by category.
4. **Budget** — demand served and stock left by budget for each rule; budget ×
   service level.
5. **Decision** — the order to place and how much budget 99 % service needs.
