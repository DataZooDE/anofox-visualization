# Statistics showcase: Northwind Grocers

Every statistics plot anofox-visualization draws for anofox-statistics results,
in modelling order, on synthetic data for a made-up grocery chain. It's the
"Statistics showcase" example of the [live demo](https://datazoode.github.io/anofox-visualization/).

`showcase.sql` is a link to `web/examples/statistics-showcase.sql`, the single
source the browser demo loads. It has two parts:

1. **Datasets.** Plain `CREATE OR REPLACE TABLE` statements build six small
   tables. The noise is a hash of fixed keys, so every run gives the same data,
   and no files are read.
2. **Dashboard.** Ten tabs of panels that call the anofox_statistics producers
   live (`ols_tidy_by`, `ols_augment_by`, `ridge_cv_by`, `kaplan_meier_by`,
   `tukey_hsd_by`, …). Each panel hands the producer's long table to an
   `anofox_plot_*` contract plot:

   ```sql
   SELECT anofox_plot_terms(t), 'Coefficients'::TITLE
   FROM ols_tidy_by('nw_sales', chain, sales, [price, promo]) t;
   ```

## Where it runs

- **Browser.** DuckDB-Wasm loads the anofox_statistics wasm extension on first
  use, from `web/localext/` (see `scripts/fetch-wasm-extensions.sh`). The
  dashboard planner rewrites each `anofox_plot_*(t)` call to
  `to_json(list(t))` and draws it with the wasm renderer.
- **Natively.** With a DuckDB that has anofox_statistics installed
  (`INSTALL anofox_statistics FROM 'https://get.erpl.io'`, run once), lint every
  panel from the repo root:

  ```sh
  cargo run --bin dashboard -- --check examples/statistics-showcase/showcase.sql
  ```

  This needs a `duckdb` on `PATH` that can load the extension. The extension is
  unsigned, so use a wrapper that runs `duckdb -unsigned "$@"`.

  `serve` and the `dashboard` renderer take the same file. Outside the
  dashboard tools, the same queries without their `::ROLE` columns run as the
  anofox_visualization extension's SQL macros and return SVG.

| Table | Rows | What it is |
|---|---|---|
| `nw_sales` | 624 | 6 stores × 104 weeks of sales (k€) with price, promo, season, temp and trend. Airport has 3 weeks entered at 2.5× by mistake, and Riverside has a clearance week at price 1.20. |
| `nw_online` | 169 | Online-shop weekly sales: 156 observed weeks plus 13 future weeks with `y` NULL. Residuals are AR(1), the price effect drifts from −10 to −25, and `split` is train/test/future. |
| `nw_mmm` | 200 | Marketing mix: 8 correlated spend channels, of which only tv, search, social and email drive revenue. |
| `nw_churn` | 1500 | Subscribers with plan, tenure, tickets and fee. Also `churned`, `p_logit` (a logistic fit), `p_rule` (a crude score) and `lifetime_months`/`event` (survival, censored). |
| `nw_orders` | 1200 | Basket value against discount and membership. The noise is skewed and grows with the discount, so the slope rises with tau. |
| `nw_ab` | 600 | Checkout experiment with 4 page variants: order value and conversion per session. |
| `nw_online_obs`, `nw_online_resid`, `nw_churn_scores`, `nw_tests` | | Helper tables: observed weeks, OLS training residuals, the two churn scores stacked long, and a view of the `*_test_agg` results. |
