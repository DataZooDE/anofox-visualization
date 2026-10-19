# Tabular showcase: can we switch on the new CRM?

A data-quality report built with
[anofox-tabular](https://github.com/DataZooDE/anofox-tabular): a made-up online
shop moves its customer master to a new CRM and checks 600 customers and 4,509
orders (Jun–Sep 2026) before switching over. It's the "Can we switch on the new CRM?" example
of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Tabular).

## Stored results, not live

anofox_tabular has **no DuckDB-Wasm build** (get.erpl.io serves only native
platforms for it), so the browser demo can't run it. The computation runs
natively and the dashboard reads its results:

| File | Role |
|---|---|
| `data.sql` | The synthetic inputs (hash-based noise, no files), with planted problems: malformed emails and VAT IDs, missing phones, duplicate and negative orders, orphan customers, a feed outage on 12 Aug and a pricing bug on 3–5 Sep. |
| `compute.sql` | Runs the extension: a `run_checks` suite stored as rows, the same order checks once per week (a per-week `filter_expr`), one `rel_count_change` / `metric_anomaly_iqr` check per day, `profile_table`, `email_is_valid` / `phonenumber_is_valid` / `vat_is_valid` on the legacy and the new CRM, `isolation_forest_mv`, `outlier_tree` and `diff_joindiff`; one result table per panel (`tq_*`). |
| `../../scripts/gen-showcase-data.sh tabular` | Runs `compute.sql` natively and writes the tables to `web/data/tabular/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/tabular-showcase.sql`, the single source of the dashboard. It only reads the CSVs, and says on the page that the results are stored. |

Regenerate (DuckDB v1.5.6, from the repository root):

```sh
duckdb -c "INSTALL anofox_tabular FROM community"   # once (signed; FORCE INSTALL replaces a get.erpl.io install)
DUCKDB=duckdb scripts/gen-showcase-data.sh tabular
cd web && ../target/release/dashboard --check ../examples/tabular-showcase/showcase.sql
```

## The dashboard

The question, four filters and a KPI strip. The filters only
re-slice the stored tables:

- **Checks on** (all tables / orders / customers, buttons) — the check panels and KPIs;
- **Countries** — field validity, the legacy-vs-new comparison and the diff;
- **Order dates** — the weekly rule runs, the daily checks and the anomalies;
- **Outlier cut-off** — the isolation-forest score above which an order counts
  as an outlier (the stored scores are thresholded in SQL).

Clicking a channel bar filters the flagged-order list; clicking a diff type
filters the migration list — each chart has its own selection, so the two
don't interfere.

1. **Overview** — share of passing check results (gauge), rows to fix per
   failing check, and the failing checks with their weekly trend (sparkline)
   and number of failing weeks.
2. **Rules** — the order checks week by week: which ones fail when, the rate
   checks against the 99 % line, weekly volume against its floor, and every
   check result.
3. **Over time** — orders and revenue per day with the days the time-aware
   checks flag (the outage, a false alarm a week later, the pricing bug); a
   strip under the orders chart marks which check fired on which day.
4. **Anomalies** — order amounts by quantity, the score distribution, flagged
   orders by channel and the `outlier_tree` explanation of each.
5. **Profiles** — validity of email, phone and VAT ID by country, legacy vs. new
   CRM (the migration fixes nothing), and the `profile_table` column profile.
6. **Migration** — added, removed and changed customers by type, field and
   country, and every differing row.
