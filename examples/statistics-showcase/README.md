# Statistics showcase: what sets the price of a home?

Every statistics plot anofox-visualization draws for anofox-statistics results,
in modelling order, on a synthetic property market in a made-up city
(2023–2025). It's the "What sets the price of a home?" example of the [live demo](https://datazoode.github.io/anofox-visualization/).

`showcase.sql` is a link to `web/examples/statistics-showcase.sql`, the single
source the browser demo loads. It has two parts:

1. **Datasets.** Plain `CREATE OR REPLACE TABLE` statements build three small
   tables (plus helper tables). The noise is a hash of fixed keys, so every run gives the same data,
   and no files are read.
2. **Dashboard.** Ten tabs of panels that call the anofox_statistics producers
   live (`ols_tidy_by`, `ols_augment_by`, `ridge_cv_by`, `kaplan_meier_by`,
   `tukey_hsd_by`, …). Each panel hands the producer's long table to an
   `anofox_plot_*` contract plot:

   ```sql
   SELECT anofox_plot_terms(t), 'Coefficients'::TITLE
   FROM ols_tidy_by('pm_sales', market, price_k, [area_10, age_10, condition]) t;
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
| `pm_sales` | 900 | Home sales, one per day from Jan 2023 to Dec 2025: price (k€) against living area, rooms, age, condition (1–5), distance to the centre, garden, district (5, dummy-coded with North Park as reference) and agency (4). The price of a m² drifts from 2.4 to 3.6 k€ over the three years, the noise grows with the living area, and neighbouring sales share a market mood (≈ AR(1)). Sales 137, 482 and 771 are Hillside villas keyed in at 2.5× their price (`flagged`), and sale 600 is a 620 m² estate (high leverage, priced on the model). `split` puts one in five sales into a `holdout`. |
| `pm_flats` | 320 | Three-room flats with ten correlated amenities. Only garage (+20 k€), heat pump (+16), renovated kitchen (+12) and balcony (+8) move the price. |
| `pm_listings` | 1,400 | Listings followed to 31 Dec 2025: asking-price premium, photos, district, agency, viewings (Poisson), days on the market (Weibull, still-listed homes censored), `sold_30` (sold within 30 days, for listings online at least 30 days), `p_logit` (a logistic fit) and `p_rule` (a rule of thumb). |
| `pm_homes`, `pm_resid`, `pm_listings_30`, `pm_scores`, `pm_tests` | | Helper tables: the sales without the flagged rows and the estate (plus € per m²), the price model's residuals in sale-date order, the listings with a 30-day outcome, the two 30-day scores stacked long, and a view of the `*_test_agg` results. |
