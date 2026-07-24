# S&P 500 companies financials — example dashboard

A three-page, pure-SQL dashboard over the public
[S&P 500 companies financials](https://github.com/datasets/s-and-p-500-companies-financials)
dataset, served by `anofox_serve_dashboards`. No frontend code — each page is a
`.sql` file whose output columns are tagged with `::ROLE` casts.

It's a good tour of the framework on a small, familiar, **cross-sectional**
(non-time-series) dataset: KPI tiles, bar charts, histograms, box plots, a
scatter plot coloured by category, a searchable dropdown that drives a whole
page, and sortable tables.

## Data

Two published CSVs, joined in [`init.sql`](init.sql) into one tidy table `co`:

| File | Columns used |
|---|---|
| `data/constituents-financials.csv` | price, P/E, dividend yield, EPS, 52-week range, market cap, EBITDA, P/S, P/B |
| `data/constituents-gics.csv` | GICS sector, sub-industry, HQ state, founding year |

503 constituents; 466 match a GICS sector by ticker (the financials snapshot is
slightly older, so 37 fall into an `Other` bucket that the sector charts
exclude). Sources: the
[`s-and-p-500-companies-financials`](https://github.com/datasets/s-and-p-500-companies-financials)
and [`s-and-p-500-companies`](https://github.com/datasets/s-and-p-500-companies)
datasets.

## Pages

- **`dash/01_overview.sql`** — market KPIs; companies and market cap by sector;
  P/E and dividend-yield distributions; the 20 largest companies.
- **`dash/02_sectors.sql`** — a `::DROPDOWN` sector selector driving per-sector
  KPIs, a top-15 bar chart, a P/E-vs-yield scatter, and the full company table.
- **`dash/03_valuation.sql`** — P/E and yield box plots by sector, a
  size-vs-valuation scatter coloured by sector, and value/income screens.

## Run

```sh
# One-time: build the extension (produces /tmp/anofox_visualization.duckdb_extension)
../../duckext/scripts/build-native.sh

./serve.sh                     # → http://127.0.0.1:8123/
PORT=9000 ./serve.sh           # different port
EXT=/path/to/ext ./serve.sh    # a pre-built extension elsewhere
```

`serve.sh` builds the data layer from the CSVs (`init.sql`) and serves the
`dash/` folder read-only. The pages replan in the browser via DuckDB-Wasm; the
server only allow-lists their SQL.

## Refresh the data

```sh
cd data
curl -sSLO https://raw.githubusercontent.com/datasets/s-and-p-500-companies-financials/main/data/constituents-financials.csv
curl -sSL  https://raw.githubusercontent.com/datasets/s-and-p-500-companies/main/data/constituents.csv -o constituents-gics.csv
```
