# Which run order gets week 43's orders out on time with the least changeover?

A use-case dashboard for [anofox-optimize](https://github.com/DataZooDE/anofox-optimize):
production scheduling with sequence-dependent changeovers. It's the
"Which line runs which order, with the least changeover?" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Optimize).

**Who asks:** the production planner of a made-up soft-drinks bottler, on
Friday, before releasing next week's plan to the shift leaders. 42 orders on
three filling lines (returnable glass, PET, cans), each with a truck slot; a
quarter are key-account promotions (weight 3). A lighter flavour after a
heavier one needs a full clean (80 min), a new bottle format 45 min more,
sugar-free its own rinse; the next lot of the same product only 10 min.

**The story (as booked, today's changeover times):** today's due-date order
misses 5 trucks and spends 57 h on changeovers. `opt_schedule_exact` (the
proven optimum for weighted lateness, ≤ 14 jobs per line) misses 3, all
standard orders, with 41 h of changeovers. Sequencing only for the fewest
changeovers (`opt_sequence_best_of`) cleans least (25 h) but misses 14
trucks. If a heatwave adds 10 %, even the optimum misses 8 trucks — 4 with
the SMED project's 40 % faster changeovers.

## Stored results, not live

The browser demo doesn't run anofox_optimize: its DuckDB-Wasm build doesn't
load yet (see [../optimize-showcase/README.md](../optimize-showcase/README.md)).
The computation runs natively, the dashboard reads its results.

| File | Role |
|---|---|
| `compute.sql` | Synthetic order book (hash-based noise, no files), the changeover rule, and the solver runs: 6 rules (`opt_schedule_edd` = today, `_wspt`, `_atcs`, `_local_search`, `_exact`, and `opt_sequence_best_of` for the fewest changeovers) × 3 lines × 8 scenarios (4 demand levels × 2 changeover settings). Tables `pp_*`. |
| `../../scripts/gen-optimize-cases.sh production` | Runs `compute.sql` with a native DuckDB and writes `pp_orders`, `pp_scenarios`, `pp_algos`, `pp_gantt`, `pp_matrix` to `web/data/optimize-production/*.csv` (≈ 360 KB), plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/optimize-production.sql`, the dashboard. It only reads the CSVs and says on the page that the results are stored. |

Regenerate (DuckDB v1.5.6, started `-unsigned`):

```sh
duckdb -unsigned -c "INSTALL anofox_optimize FROM 'https://get.erpl.io'"   # once
DUCKDB=duckdb scripts/gen-optimize-cases.sh production
```

Lint it natively (CSV paths are relative to `web/`):

```sh
cd web && ../target/release/dashboard --check --strict examples/optimize-production.sql
```

## Inputs and tabs

Inputs: sequencing rule (compared with today's due-date order), demand
scenario, changeover times, and the line to drill into. They only pick among
stored runs. KPIs: orders on their truck, key-account orders on time, share of
line time spent filling, changeover hours saved, line hours left before
Saturday 06:00.

1. **Overview** — weighted lateness by rule and line; the changeover-vs-lateness
   trade-off; per-line table (today vs selected).
2. **Run plan** — Gantt of one line, today vs the selected rule (changeovers as
   separate bars), and the sequence as the shift leader's list.
3. **Orders** — all 42 orders, subtotalled by line, rescued / still-late badges.
4. **Changeovers** — hours by kind (full clean, rinse, format parts, …) and the
   line's changeover matrix.
5. **Scenarios** — late orders by rule × demand; weighted lateness vs demand
   with and without the SMED project.
6. **Decision** — the recommended rule (least weighted lateness at the chosen
   scenario) and its late orders with the customer to call.
