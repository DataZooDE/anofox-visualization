# Plotting Python model results (notebook)

[`plot-results.ipynb`](plot-results.ipynb) answers four questions of a
synthetic online shop with ordinary Python libraries and plots every result
with the [`anofox_viz`](../../python) helper package:

| Question | Library | Plots |
|---|---|---|
| What drives basket value? | statsmodels OLS | coefficient forest, residual diagnostics (2×2) |
| Which customers will order again? | scikit-learn (logistic regression, gradient boosting) | ROC, precision–recall, calibration, hold-out metrics |
| Did the new checkout raise basket value? | scipy (Welch t, bootstrap), pingouin | test forests |
| How many orders next month? | statsforecast (AutoETS, seasonal naive) | forecast with nested 80 % / 95 % intervals and an events overlay; events timeline |

It ends by writing the tables to Parquet (`results/`), writing a dashboard
(`shop_report.sql`) that reads them, and running `dashboard --check` on it.
The committed notebook carries its outputs, so GitHub shows the plots.

## Run it

```sh
python -m venv .venv
.venv/bin/pip install "duckdb==1.5.6" -e "../../python" \
    statsmodels scikit-learn scipy pingouin statsforecast pyarrow jupyter
ANOFOX_VIZ_EXTENSION=/path/to/anofox_visualization.duckdb_extension .venv/bin/jupyter lab plot-results.ipynb
```

Without `ANOFOX_VIZ_EXTENSION`, `av.connect()` installs the extension
(community repository, then the project's release repository). Build one
from the repository root with
`SKIP_WASM=1 OUT=$PWD/anofox_visualization.duckdb_extension bash duckext/scripts/build-native.sh --release`.
The `dashboard --check` cell needs the `dashboard` binary (release CLI
archive or `cargo build --release --bin dashboard`; on `PATH`, in
`target/`, or in `$ANOFOX_DASHBOARD`) and a `duckdb` CLI on `PATH`; without
them it says it skipped.

`python/tests/test_notebook.py` executes the notebook in CI.
