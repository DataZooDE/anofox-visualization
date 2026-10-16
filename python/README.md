# anofox-viz (Python)

Shape statsmodels, scikit-learn, scipy, pingouin, lifelines, statsforecast
and Prophet results into the tables of the anofox integration contract, and
plot them with the `anofox_plot_*` macros of the
[anofox_visualization](https://github.com/DataZooDE/anofox-visualization)
DuckDB extension.

```python
import anofox_viz as av
import statsmodels.formula.api as smf

av.connect("./anofox_visualization.duckdb_extension")

res = smf.ols("y ~ x1 + x2", data=df).fit()
t = av.from_statsmodels(res)          # {"terms", "obs", "summary", "prediction"}
av.plot(t, "terms")                   # an SVG; Jupyter shows it inline
av.plot(t, "diagnostics").save("diagnostics.svg")
```

Requires `duckdb`, `numpy` and `pandas`; the modelling libraries are
imported only by the function that reads their results. Not on PyPI yet:

```sh
pip install "git+https://github.com/DataZooDE/anofox-visualization#subdirectory=python"
```

The functions, the extension lookup (`connect()`) and the recipes without
this package are in [docs/python-r.md](../docs/python-r.md).

## Tests

```sh
python -m venv .venv && .venv/bin/pip install -e ".[test]"
.venv/bin/pytest                                     # shaping + argument tests
ANOFOX_VIZ_EXTENSION=/path/to/anofox_visualization.duckdb_extension .venv/bin/pytest
```

With `ANOFOX_VIZ_EXTENSION` set, the tests also render every plot, run
the Python snippets of `docs/python-r.md` (a snippet whose library is not
installed is skipped) and execute
[`examples/python/plot-results.ipynb`](../examples/python/plot-results.ipynb)
(needs `nbclient`, `ipykernel`, `pingouin` and `statsforecast`). Build the extension with
`SKIP_WASM=1 OUT=$PWD/anofox_visualization.duckdb_extension bash duckext/scripts/build-native.sh --release`
from the repository root.
