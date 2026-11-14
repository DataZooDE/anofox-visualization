# The anofox extensions in CI

Every producer of every anofox extension is rendered against this extension
on every push, so a change in a producer's output, or in a plot, is caught the
day it lands. Each harness calls the producers on small deterministic inputs,
hands the output to `anofox_plot(...)` and to its specific macro, and fails
on a SQL error, a missing SVG, an unexpected render warning, or a producer it
has no recipe for. The schemas are in the
[integration contract](plans/integration-contract.md); how to plot each
extension's results is on the [website](https://datazoode.github.io/anofox-visualization/integrate/).

## Conformance with anofox-statistics

`scripts/statistics_conformance.py` calls **every** contract producer of the
[anofox-statistics](https://github.com/DataZooDE/anofox-statistics) extension
(the 141 functions tagged `anofox.contract = '1'`: table macros directly, the
`*_agg` test aggregates through `unnest`), hands each output to
`anofox_plot(...)` and to its specific macro, and fails on a SQL error, a
missing SVG or an unexpected render warning. It writes every SVG (and a PNG
montage when `rsvg-convert` and Pillow are installed) to `--out`:

```bash
duckdb -unsigned -c "INSTALL anofox_statistics FROM 'https://get.erpl.io'"   # DuckDB v1.5.6
SKIP_WASM=1 OUT=/tmp/anofox_visualization.duckdb_extension duckext/scripts/build-native.sh --release
python3 scripts/statistics_conformance.py --duckdb duckdb --ext /tmp/anofox_visualization.duckdb_extension --out /tmp/stats-viz
```

CI runs it on every push (`.github/workflows/ci.yml`, job *anofox-statistics
conformance*).

## Conformance with anofox-decide

[anofox-decide](https://github.com/DataZooDE/anofox-decide) tags no contract
producer yet: its answers are per-row probabilities, choices and scores, its
quality metrics single numbers. `scripts/decide_conformance.py` runs every
decide output through the SQL that makes it a contract table (metrics →
`summary`, probabilities → ROC / PR / calibration `curve`, also through
anofox-statistics' `roc_curve_by` / `pr_curve_by` / `calibration_curve_by`),
hands it to `anofox_plot(...)` and to the specific macro, and fails on a SQL
error, a missing SVG or an unexpected render warning. The raw outputs no schema
takes (`decide_table`, `decide_answers`) must fail with the clean "match no
plottable schema" error, and a tagged decide producer without a recipe fails
the run. The answers come from decide's built-in `stub` model (offline);
`--model laya-multilingual` adds a real local model:

```bash
duckdb -c "INSTALL anofox_decide FROM community"                             # DuckDB v1.5.6
python3 scripts/decide_conformance.py --duckdb duckdb --ext /tmp/anofox_visualization.duckdb_extension --out /tmp/decide-viz
```

CI runs it on every push (job *anofox-decide conformance*).

## Conformance with anofox-tabular

[anofox-tabular](https://github.com/DataZooDE/anofox-tabular) does not tag its
functions for the contract yet, and none of its outputs matches a contract
schema as it comes. `scripts/tabular_conformance.py` calls **every** result
producer anyway (the 30 `anofox_tab_*` table functions other than the
config/status ones) on small deterministic tables. It reshapes each output into
the nearest schema (check results and profiles → `summary`, the time-aware checks
→ `events`, `outlier_tree` → `terms`) or a generic chart (diffs, PII counts,
anomaly scores), and hands it to `anofox_plot(...)` and the specific macro. It
fails on a SQL error, a missing SVG, an unexpected render warning or a
producer without a recipe. It also reports when a raw output starts to match a
schema:

```bash
duckdb -c "INSTALL anofox_tabular FROM community"   # signed, DuckDB v1.5.6
python3 scripts/tabular_conformance.py --duckdb duckdb --ext /tmp/anofox_visualization.duckdb_extension --out /tmp/tabular-viz
```

CI job *anofox-tabular conformance*.

## Conformance with anofox-forecast

`scripts/forecast_conformance.py` does the same for
[anofox-forecast](https://github.com/DataZooDE/anofox-forecast). That extension
does not tag its functions yet, so the producers are a curated registry: every
`ts_*` table macro and aggregate of the loaded build is either a producer (51
today: forecasts, backtests, conformal intervals, decompositions, changepoints,
peaks, periods, metrics, residual tests, features) or listed as out of scope
with a reason (fills, drops, CV splits, validators); an unlisted function fails
the run. A producer is *direct* when its output plots as returned and an
*adapter* when the harness reshapes it first (the reshaping SQL is in the
script — what you would write today). It runs on a three-series daily panel
(weekly season, a level shift, an exogenous driver) and accepts both builds:

```bash
duckdb -c "INSTALL anofox_forecast FROM community"                                  # the release
duckdb -unsigned -c "FORCE INSTALL anofox_forecast FROM 'https://get.erpl.io'"      # or main
python3 scripts/forecast_conformance.py --duckdb duckdb --ext /tmp/anofox_visualization.duckdb_extension --out /tmp/forecast-viz
```

Producer bugs of a released build are listed in the script (`KNOWN`) and
reported `XFAIL`. CI runs both builds (job *anofox-forecast conformance*).

## Conformance with anofox-optimize

[anofox-optimize](https://github.com/DataZooDE/anofox-optimize)'s solvers are
scalar functions that return a STRUCT of lists, so none of them is a contract
table. `scripts/optimize_conformance.py` treats every `anofox_optimize_*`
solver as a producer (38 in 8 families) and carries one recipe per family. It
calls each solver on a small deterministic instance and checks the solution
itself: bins and waves within capacity, and the objective recomputed from the
returned order or selection. The objectives then go through `anofox_plot(...)`
and `anofox_plot_summary(...)`, and the family's decision view is drawn: a
Gantt for schedules, stacked bins and waves, the route on a `::MAP`, and
selections and weights as bars. A solver without a recipe fails the run:

```bash
duckdb -unsigned -c "INSTALL anofox_optimize FROM 'https://get.erpl.io'"   # DuckDB v1.5.6
python3 scripts/optimize_conformance.py --duckdb duckdb --ext /tmp/anofox_visualization.duckdb_extension --out /tmp/optimize-viz
```

CI job *anofox-optimize conformance*.
