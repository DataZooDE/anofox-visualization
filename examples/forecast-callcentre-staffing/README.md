# Forecast use case: how many agents do we need per half-hour for the next three weeks?

The workforce-management planner of a made-up customer-service centre publishes
the roster for the next three weeks every Friday. The dashboard forecasts daily
contacts per queue **live** with
[anofox-forecast](https://github.com/DataZooDE/anofox-forecast), turns them into
required agents per half-hour with Erlang C, and shows where the current shift
plan falls short.

`showcase.sql` is a link to `web/examples/forecast-callcentre-staffing.sql`,
the single source the browser demo loads.

- **Data.** 26 weeks of half-hourly contacts for three queues (Billing,
  Technical support, Sales; about 10,000 rows), generated in SQL with
  hash-based noise (no files): weekday pattern, intraday humps, a monthly
  bill-run spike in Billing, a growing sales campaign and one outage day.
- **Forecasting.** `ts_cv_folds_by` + `ts_cv_forecast_by` backtest
  SeasonalNaive, AutoETS, AutoTheta and MSTL over four rolling three-week
  folds, scored with `ts_mape`, `ts_bias` and `ts_rmse`; the best method per
  queue forecasts 21 days with `ts_forecast_by`. No method sees the bill run,
  so a calendar factor learned from the backtest errors is applied on those
  days, and the quantiles of actual ÷ forecast give the P50 / P80 / P90 plan.
- **Staffing.** The daily plan is spread over the half-hours with the last 8
  weeks' intraday profile and converted to agents with Erlang C in SQL
  (service level, answer time and shrinkage are inputs), then compared with a
  three-shift roster.
- **Dashboard.** The question and who asks it, staffing rules, a KPI strip,
  then four tabs: demand (the forecast revealed from the roster start, a strip
  of the bill runs and the storm outage under it; a click on a queue opens its
  gaps) · staffing gap (per queue; the short half-hours of the worst day
  shaded, Erlang C as a formula) · backtest · roster recommendation (per day
  and queue, what to add to which shift).

## Where it runs

- **Browser.** DuckDB-Wasm loads the anofox_forecast wasm extension on first
  use (`web/localext/`, see `scripts/fetch-wasm-extensions.sh`). A full run
  takes a few seconds; changing an input recomputes the Erlang tables only.
- **Natively.** With a DuckDB that has anofox_forecast installed
  (`INSTALL anofox_forecast FROM 'https://get.erpl.io'`, run once) and a
  `duckdb` on `PATH` that runs `duckdb -unsigned "$@"`:

  ```sh
  cargo run --bin dashboard -- --check examples/forecast-callcentre-staffing/showcase.sql
  ```
