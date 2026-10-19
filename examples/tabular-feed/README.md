# Is today's supplier feed safe to load?

A use-case dashboard for [anofox-tabular](https://github.com/DataZooDE/anofox-tabular):
monitoring an incoming data feed. A made-up wholesaler, Nordwind, sends a
home-and-garden shop its full price and stock list every night. At 07:00 the
data engineer on duty decides whether to load it into the product catalogue:
a bad load puts wrong prices on the website within the hour, holding it back
keeps yesterday's prices for another day. It's the "Is today's supplier
feed safe to load?" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Tabular).

## Stored results, not live

anofox_tabular has **no DuckDB-Wasm build**, so the browser demo can't run it.
The computation runs natively and the dashboard reads its results:

| File | Role |
|---|---|
| `data.sql` | 60 nightly deliveries (17 Aug – 15 Oct 2026, ~1,200 SKUs each), synthetic and deterministic, stacked into one staging table the way `read_csv([...], union_by_name = true)` would (a column missing from a file loads as NULL). Planted: a cut-off file (29 Aug), duplicated SKUs (8 Sep), USD prices (16–18 Sep), a late file (21 Sep), prices in cents (29 Sep – 1 Oct), a re-sent stale file (8 Oct) and today's new export format (a renamed and a new column, 8 % blank EANs, seven prices without their decimal point, late). |
| `compute.sql` | The data contract as one `run_checks` suite: `volume`, `duplicate_count`, `null_rate`, `regex_match`, `values_in_set`, `compliance` and `agg` with `identifier_column = delivery_date` (one result per file), plus one `freshness`, `rel_count_change` and `metric_anomaly_iqr` row per delivery (their reference date / time). `monitor_only` marks the warnings. Also: per-column fill per delivery (`null_rate`), `schema_check` and `profile_table` on today's and yesterday's file, and an `isolation_forest` score for every day-over-day price move. |
| `../../scripts/gen-tabular-cases.sh feed` | Runs `compute.sql` natively and writes the tables to `web/data/tabular-feed/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/tabular-feed.sql`, the single source of the dashboard. It only reads the CSVs, and says on the page that the results are stored. |

Regenerate (DuckDB v1.5.6, from the repository root):

```sh
duckdb -c "INSTALL anofox_tabular FROM community"   # once (signed; FORCE INSTALL replaces a get.erpl.io install)
DUCKDB=duckdb scripts/gen-tabular-cases.sh feed
cd web && ../target/release/dashboard --check --strict ../examples/tabular-feed/showcase.sql
```

## The dashboard

It opens with the question and who asks it, then the inputs and a KPI strip
for the selected delivery (decision, blocking checks and warnings against the
night before, rows against the 7-day mean, missing EANs, arrival time; a
failing blocking check or more missing EANs than allowed turns its tile red). The
inputs only re-slice the stored tables:

- **Delivery to judge** — every panel about "the delivery" (default: today);
- **Missing values allowed (%)** — re-judges the completeness checks from
  their stored null rates (raise it to 10 % and today's blank EANs pass);
- **Unusual price move from score (%)** — the isolation-forest cut-off;
- **History** — the date range of the history charts.

1. **Overview** — failing checks per night by kind with the incidents
   labelled, the verdicts over 60 nights and every delivery with a finding
   (its link opens that night's go / no-go).
2. **Schema & completeness** — empty values per column over the last 14
   deliveries (today's file has a different shape), `schema_check` and
   `profile_table` for today vs. yesterday.
3. **Volume & prices** — rows per delivery against the 7-day mean, the mean
   price against its 28-day IQR band, price changes per night (none on the
   re-sent 8 Oct file) and the unusual price moves of the selected delivery.
4. **Go / no-go** — the decision in one sentence and the data contract check
   by check, failing ones first.
