# Tabular showcase: can we switch on the new CRM?

A data-quality report built with
[anofox-tabular](https://github.com/DataZooDE/anofox-tabular): a made-up online
shop moves its customer master to a new CRM and checks 600 customers and 4,509
orders (Jun–Sep 2026) before switching over. It's the "Stored results: CRM
data quality" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Tabular).

## Stored results, not live

anofox_tabular has **no DuckDB-Wasm build** (get.erpl.io serves only native
platforms for it), so the browser demo can't run it. The computation runs
natively and the dashboard reads its results:

| File | Role |
|---|---|
| `data.sql` | The synthetic inputs (hash-based noise, no files), with planted problems: malformed emails and VAT IDs, missing phones, duplicate and negative orders, orphan customers, a feed outage on 12 Aug and a pricing bug on 3–5 Sep. |
| `compute.sql` | Runs the extension: a `run_checks` suite stored as rows, one `rel_count_change` / `metric_anomaly_iqr` check per day, `profile_table`, `email_is_valid` / `phonenumber_is_valid` / `vat_is_valid`, `isolation_forest_mv`, `outlier_tree` and `diff_joindiff`; one result table per panel (`tq_*`). |
| `../../scripts/gen-showcase-data.sh tabular` | Runs `compute.sql` natively and writes the tables to `web/data/tabular/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/tabular-showcase.sql`, the single source of the dashboard. It only reads the CSVs, and says on the page that the results are stored. |

Regenerate (DuckDB v1.5.6, `-unsigned`, from the repository root):

```sh
duckdb -unsigned -c "INSTALL anofox_tabular FROM 'https://get.erpl.io'"   # once
DUCKDB=duckdb scripts/gen-showcase-data.sh tabular
cd web && ../target/release/dashboard --check ../examples/tabular-showcase/showcase.sql
```

## Tabs

1. **Overview** — 7 of 14 check results fail; rows to fix per failing check.
2. **Rules** — the `run_checks` results and how far each failing rate misses.
3. **Over time** — orders and revenue per day with the days the time-aware
   checks flag (the outage, a false alarm a week later, the pricing bug).
4. **Anomalies** — isolation-forest scores and `outlier_tree` explanations: the
   flagged orders are the pricing-bug orders.
5. **Profiles** — `profile_table` null / distinct rates and validity of email,
   phone and VAT ID by country.
6. **Migration diff** — added, removed and changed customers and which field
   changed.
