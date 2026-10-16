# Forecast use case: will any substation exceed its firm capacity next week?

A grid-operations planner of a made-up city utility decides every Thursday
where to pre-arrange a load transfer, call demand response or postpone
maintenance for the coming week. The dashboard forecasts the next 168 hours of
load for six primary substations, **live** with
[anofox-forecast](https://github.com/DataZooDE/anofox-forecast), and compares
the peaks with each substation's firm (N-1) capacity.

`showcase.sql` is a link to `web/examples/forecast-energy-load.sql`, the single
source the browser demo loads.

- **Data.** 8 weeks of hourly load (6 × 1,344 rows), generated in SQL with
  hash-based noise (no files): residential / commercial / EV-charging daily
  profiles, weekends, load growth at the harbour and heat pumps that follow an
  autumn cooling trend.
- **Forecasting.** `ts_cv_folds_by` + `ts_cv_forecast_by` backtest
  SeasonalNaive, MSTL (24 h + 168 h seasonality) and AutoTheta over three
  rolling one-week folds; the most accurate method per substation is refit
  with `ts_forecast_by`. The models' own intervals cover everything but are far
  wider than the load, so the plan uses split-conformal intervals
  (`ts_conformal_quantile` of the backtest errors, calibrated on folds 1–2 and
  checked on fold 3 with `ts_coverage`). `ts_detect_peaks_by` finds next week's
  daily peaks.
- **Dashboard.** The question and who asks it, planning assumptions (interval
  level, alarm threshold), a KPI strip coloured by thresholds (with ⓘ
  definitions), then four tabs: capacity risk · substation drill-down (a
  50 / 80 / 95 % conformal fan revealed from the forecast start, the hours
  whose upper bound crosses firm capacity shaded) · backtest (the conformal interval as a formula) · decision (status,
  action and a link to its week per substation).

## Where it runs

- **Browser.** DuckDB-Wasm loads the anofox_forecast wasm extension on first
  use (`web/localext/`, see `scripts/fetch-wasm-extensions.sh`). A full run
  takes a few seconds; changing an input only re-queries the views.
- **Natively.** With a DuckDB that has anofox_forecast installed
  (`INSTALL anofox_forecast FROM 'https://get.erpl.io'`, run once) and a
  `duckdb` on `PATH` that runs `duckdb -unsigned "$@"`:

  ```sh
  cargo run --bin dashboard -- --check examples/forecast-energy-load/showcase.sql
  ```
