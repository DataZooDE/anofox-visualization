# Optimize showcase: one Friday at a distribution centre

Six decisions of a made-up distribution centre, each solved with one
[anofox-optimize](https://github.com/DataZooDE/anofox-optimize) family and
compared with the family's deliberately weak baseline ("what we do today"):
packing cartons into containers (`opt_pack_*`), choosing shipments for a cheap
carrier (`opt_knapsack_*`), cutting pick waves (`opt_wave_*`), scheduling the
packing line against due dates (`opt_schedule_*`), sequencing the paint line
(`opt_sequence_*`) and picking a portfolio for the reserve fund
(`opt_portfolio_*`). It's the "Stored results: distribution centre" example of
the [live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Optimize).

## Stored results, not live

The browser demo **does not run anofox_optimize**. The extension's v1.5.6
DuckDB-Wasm build exists on get.erpl.io but doesn't load in DuckDB-Wasm
(`LOAD` fails with `t is not a function`): it links the PostHog telemetry
client, which imports OpenSSL / httplib symbols the DuckDB-Wasm runtime
doesn't provide. anofox-statistics compiles telemetry out under Emscripten;
anofox-optimize doesn't yet.

So the computation runs natively and the dashboard reads its results:

| File | Role |
|---|---|
| `compute.sql` | Builds the synthetic inputs (hash-based noise, no files) and calls the solvers; one result table per panel (`os_*`). |
| `../../scripts/gen-showcase-data.sh optimize` | Runs `compute.sql` with a native DuckDB and writes the tables to `web/data/optimize/*.csv`, plus `provenance.csv` (extension and DuckDB version). |
| `showcase.sql` | A link to `web/examples/optimize-showcase.sql`, the single source of the dashboard. It only reads the CSVs, and says on the page that the results are stored. |

Regenerate the stored results (DuckDB v1.5.6, started `-unsigned`):

```sh
duckdb -unsigned -c "INSTALL anofox_optimize FROM 'https://get.erpl.io'"   # once
DUCKDB=duckdb scripts/gen-showcase-data.sh optimize
```

Lint the dashboard natively (the CSV paths are relative to `web/`):

```sh
cd web && ../target/release/dashboard --check ../examples/optimize-showcase/showcase.sql
```

## Tabs

1. **Overview** — today's rule vs the optimiser for every decision.
2. **Containers** — 150 cartons: next fit needs 46 containers, every decreasing
   packer 39 = the volume lower bound (provably optimal here).
3. **Carrier** — saving vs booked capacity: the exact knapsack beats "biggest
   savers first" by about a third at 2 t.
4. **Pick waves** — priority-weighted mean wave 5.73 → 3.85; express orders
   move to the first waves.
5. **Lines** — a Gantt-like plan of the packing line (exact schedule: weighted
   lateness 998 → 771) and the paint-line changeovers (303 → 63 minutes).
6. **Reserve fund** — return vs risk per (holdings, cap) setting; the
   optimiser's portfolios earn more per unit of risk than the naive top-return
   rule.
