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
| `compute.sql` | Builds the synthetic inputs (hash-based noise, no files) and calls the solvers over **parameter sweeps** — four container sizes, 20 carrier bookings (250–5,000 kg), four wave sizes, every schedule rule, every (holdings, cap) portfolio setting — so the dashboard's planning inputs have stored results to pick from. One result table per panel (`os_*`). |
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

## Planning inputs

The header holds six inputs: container size, carrier booking (snapped to the
nearest stored 250 kg step), wave size, the packing-line rule to compare with
today's, and the portfolio's holdings limit and cap per asset. They only
select among the stored sweeps — nothing is re-solved in the browser. A KPI
strip compares the optimiser with today's rule at the chosen inputs.

## Tabs

1. **Overview** — the gain over today's rule per decision, and which solver
   family (with which guarantee) handles it.
2. **Containers** — fill gauges, containers used vs the volume lower bound,
   the fill profile and containers needed per container size (next fit needs
   46 containers at 1,000 units, every decreasing packer 39 = the bound).
3. **Carrier** — saving vs booked capacity for three rules, the shipments each
   plan loads at the chosen booking, and where the plans disagree.
4. **Pick waves** — priority-weighted mean wave (5.73 → 3.85 at 120 lines),
   the wave loads of both plans, and a rule × wave-size heatmap.
5. **Lines** — *Packing line*: run plans of today's rule and the selected rule,
   lateness by rule, job by job (exact schedule: 998 → 771 job-minutes).
   *Paint line*: cumulative changeovers (303 → 63 minutes), the changeover
   matrix and the four sequences.
6. **Reserve fund** — return vs risk per (holdings, cap) setting, the weights at
   the selected setting, a Sharpe heatmap and the best settings.
