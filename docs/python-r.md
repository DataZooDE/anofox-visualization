# Plotting results from Python and R

The `anofox_plot_*` macros read **any** table whose columns follow the
[contract schemas](DOCS.md#plotting-model-output-contract-macros) — it does
not matter whether DuckDB, Python or R computed the numbers. Many library
tables already plot as they are: column names are compared with case, `.`,
spaces and symbols ignored, and the common statsmodels, R, broom and pingouin
spellings (`coef`, `std.error`, `P>|t|`, `[0.025`, `.fitted`, `.cooksd`,
`p-unc`, …) are read as the contract column (the
[alias table](DOCS.md#plotting-model-output-contract-macros), *Other column
names*). The rest is a rename or a reshape from wide to long.

Two ways to get the table to the renderer:

| | How | Use it for |
|---|---|---|
| **In-process** | the `duckdb` Python / R package loads the extension; a query returns the SVG | notebooks, scripts, reports |
| **Files** | Python / R writes Parquet (or CSV); a dashboard `.sql` reads it | the browser builder, `dashboard` CLI, locked serving |

- [Python: in-process](#python-in-process)
- [Python: recipes per library](#python-recipes-per-library) — statsmodels,
  scikit-learn, scipy, pingouin, lifelines, statsforecast, Prophet
- [The `anofox_viz` helper package](#the-anofox_viz-helper-package)
- [R: broom → DuckDB](#r-broom--duckdb)
- [Files: Parquet into a dashboard](#files-parquet-into-a-dashboard)

---

## Python: in-process

The extension is unsigned, so the connection must allow unsigned
extensions **before** the first `LOAD`. Use the extension file of a
[release](../README.md#from-a-release-full-toolkit) (or a
[source build](../README.md#from-source)); any `duckdb` Python package
≥ 1.2 loads it (the release is built against DuckDB v1.5.6).

```python
import duckdb
import pandas as pd

con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
con.execute("LOAD './anofox_visualization.duckdb_extension'")

coefs = pd.DataFrame({
    "term": ["(Intercept)", "price", "promo"],
    "estimate": [1.2, -0.8, 0.4],
    "conf_low": [0.9, -1.1, 0.1],
    "conf_high": [1.5, -0.5, 0.7],
})
svg = con.execute("SELECT anofox_plot_terms(r) FROM coefs AS r").fetchone()[0]
open("terms.svg", "w").write(svg)
```

DuckDB scans the pandas DataFrame `coefs` by its variable name; polars
DataFrames and Arrow tables work the same way (or `con.register("coefs",
df)`). In Jupyter, show the SVG with `IPython.display.SVG(svg)`.

**Name the row with an alias that is not a column.** The macros take the
whole row (`anofox_plot_terms(r)`), and a column with the alias's name wins
over the row: statsmodels' summary table has a column `t`, so `SELECT
anofox_plot_terms(t) FROM tbl t` passes the t statistic, not the row. The
error says so (*the argument is a column, not the row*); use `r` or
another name.

Options are macro arguments (`anofox_plot_terms(r, title := 'Price model')`),
`GROUP BY` gives one SVG per group:

```python
curves = pd.DataFrame({
    "curve_type": ["roc"] * 3 + ["pr"] * 3,
    "x": [0.0, 0.3, 1.0, 0.0, 0.6, 1.0],
    "y": [0.0, 0.8, 1.0, 1.0, 0.8, 0.5],
})
svgs = dict(con.execute(
    "SELECT curve_type, anofox_plot_curve(r) FROM curves AS r GROUP BY ALL").fetchall())
```

## Python: recipes per library

Each recipe is plain pandas; the [helper package](#the-anofox_viz-helper-package)
wraps the same steps. The snippets continue the session above (`con`).

### statsmodels

The coefficient table of `summary2()` plots as it is — `Coef.`, `Std.Err.`,
`t`, `P>|t|`, `[0.025` and `0.975]` are read as `estimate`, `std_error`,
`statistic`, `p_value`, `conf_low` and `conf_high`; only the term names
need to leave the index:

```python
import numpy as np
import statsmodels.formula.api as smf

rng = np.random.default_rng(1)
d = pd.DataFrame({"x1": rng.normal(size=100), "x2": rng.normal(size=100),
                  "g": rng.choice(["a", "b", "c"], 100)})
d["y"] = 1 + 2 * d.x1 - d.x2 + rng.normal(size=100)
res = smf.ols("y ~ x1 + x2", data=d).fit()

terms = res.summary2().tables[1].reset_index(names="term")
svg = con.execute("SELECT anofox_plot_terms(r) FROM terms AS r").fetchone()[0]
```

Residual diagnostics (`obs`): `fittedvalues`, `resid`, and the influence
frame's `standard_resid`, `hat_diag`, `cooks_d` are all read as they are:

```python
obs = pd.DataFrame({"fittedvalues": res.fittedvalues, "resid": res.resid}).join(
    res.get_influence().summary_frame())
svg = con.execute("SELECT anofox_plot_diagnostics(r) FROM obs AS r").fetchone()[0]
```

Predictions (`prediction`): `get_prediction().summary_frame()` names the fit
`mean`, which needs a rename to `yhat`; `mean_ci_lower` / `mean_ci_upper`
become the band (for the wider prediction interval, rename `obs_ci_lower` /
`obs_ci_upper` to `yhat_lower` / `yhat_upper`). Add the x position and the observed `y`:

```python
res1 = smf.ols("y ~ x1", data=d).fit()
pred = (res1.get_prediction().summary_frame(alpha=0.05)
        .rename(columns={"mean": "yhat"})
        .assign(x=d.x1, y=d.y))
svg = con.execute("SELECT anofox_plot_prediction(r) FROM pred AS r").fetchone()[0]
```

Model metrics (`summary`) are one row per metric:

```python
glance = pd.DataFrame({"model_id": "ols",
                       "metric": ["r_squared", "aic", "bic"],
                       "value": [res.rsquared, res.aic, res.bic]})
svg = con.execute("SELECT anofox_plot_summary(r) FROM glance AS r").fetchone()[0]
```

Several models in one forest: stack their tables with a `model_id` column
(`pd.concat([t1.assign(model_id="ols"), t2.assign(model_id="wls")])`).
`linearmodels` results have the same attributes (`params`, `std_errors`,
`pvalues`, `conf_int()`), so the same `DataFrame` works.

`pairwise_tukeyhsd` (`test`): `p-adj`, `lower` and `upper` are read as they
are; rename `meandiff` to `estimate` to draw the differences:

```python
from statsmodels.stats.multicomp import pairwise_tukeyhsd

tk = pairwise_tukeyhsd(d.y, d.g).summary()
tukey = pd.DataFrame(tk.data[1:], columns=tk.data[0]).rename(columns={"meandiff": "estimate"})
svg = con.execute("SELECT anofox_plot_tests(r) FROM tukey AS r").fetchone()[0]
```

### scikit-learn

Curves are long tables with a `curve_type`
(`roc`, `pr`, `calibration`, …); metrics are `summary` rows:

```python
from sklearn.calibration import calibration_curve
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, precision_recall_curve, roc_auc_score, roc_curve

X, label = d[["x1", "x2"]], (d.y > 1).astype(int)
score = LogisticRegression().fit(X, label).predict_proba(X)[:, 1]

fpr, tpr, thr = roc_curve(label, score)
prec, rec, _ = precision_recall_curve(label, score)
prob_true, prob_pred = calibration_curve(label, score, n_bins=8)
curve = pd.concat([
    pd.DataFrame({"curve_type": "roc", "x": fpr, "y": tpr, "threshold": thr}),
    pd.DataFrame({"curve_type": "pr", "x": rec, "y": prec}),
    pd.DataFrame({"curve_type": "calibration", "x": prob_pred, "y": prob_true}),
])
svgs = dict(con.execute(
    "SELECT curve_type, anofox_plot_curve(r) FROM curve AS r GROUP BY ALL").fetchall())

metrics = pd.DataFrame({"model_id": "logistic", "metric": ["auc", "accuracy"],
                        "value": [roc_auc_score(label, score),
                                  accuracy_score(label, score > 0.5)]})
svg = con.execute("SELECT anofox_plot_summary(r) FROM metrics AS r").fetchone()[0]
```

One `curve_type` per plot: `GROUP BY curve_type` (above) or filter it.

### scipy.stats

A test result becomes one `test` row; `confidence_interval()` gives the
interval where scipy has one:

```python
from scipy import stats

a, b, c = (d.y[d.g == k] for k in "abc")
rows = []
for name, (u, v) in {"b − a": (b, a), "c − a": (c, a)}.items():
    r = stats.ttest_ind(u, v)
    ci = r.confidence_interval()
    rows.append({"test_id": name, "method": "t-test",
                 "statistic": r.statistic, "p_value": r.pvalue,
                 "estimate": u.mean() - v.mean(),
                 "conf_low": ci.low, "conf_high": ci.high})
tests = pd.DataFrame(rows)
svg = con.execute("SELECT anofox_plot_tests(r) FROM tests AS r").fetchone()[0]
```

### pingouin

pingouin's `T`, `p-unc` / `p_unc`, `p-corr` and `p_val` are read as
`statistic`, `p_value` and `p_adj`; name the groups and split the `CI95`
list column:

```python
import pingouin as pg

pw = pg.pairwise_tests(data=d, dv="y", between="g").rename(columns={"A": "group1", "B": "group2"})
svg = con.execute("SELECT anofox_plot_tests(r) FROM pw AS r").fetchone()[0]
```

`pairwise_tests` reports no interval, so the plot shows −log10(p) bars.
`ttest` has one (`CI95`, of the mean difference); add the difference as
`estimate`, or the forest falls back to the t statistic:

```python
tt = pg.ttest(b, a)
tt[["conf_low", "conf_high"]] = tt.pop("CI95").tolist()
tt["estimate"] = b.mean() - a.mean()
svg = con.execute("SELECT anofox_plot_tests(r) FROM tt AS r").fetchone()[0]
```

### lifelines (Kaplan–Meier)

```python
from lifelines import KaplanMeierFitter

time, event = rng.exponential(5, 100), rng.integers(0, 2, 100)
parts = []
for grp in ["a", "b"]:
    m = (d.g == grp).to_numpy()
    km = KaplanMeierFitter().fit(time[m], event[m], label=grp)
    sf, ci = km.survival_function_, km.confidence_interval_survival_function_
    et = km.event_table.reindex(sf.index)
    parts.append(pd.DataFrame({
        "curve_type": "km", "model_id": grp, "x": sf.index, "y": sf.iloc[:, 0],
        "y_low": ci.iloc[:, 0], "y_high": ci.iloc[:, 1],
        "n_risk": et.at_risk, "n_event": et.observed, "n_censor": et.censored}))
km = pd.concat(parts)
svg = con.execute("SELECT anofox_plot_curve(r) FROM km AS r").fetchone()[0]
```

### statsforecast (wide → long)

`forecast()` puts each model in its own column (`AutoETS`,
`AutoETS-lo-95`, …); the `prediction` schema wants one row per model and
date. Rename `unique_id` to `id` for one panel per series:

```python
from statsforecast import StatsForecast
from statsforecast.models import AutoETS, Naive
from statsforecast.utils import generate_series

hist = generate_series(2, freq="D", min_length=60, max_length=60)
fc = StatsForecast(models=[AutoETS(season_length=7), Naive()], freq="D").forecast(
    df=hist, h=14, level=[95])
long = pd.concat(
    [fc[["unique_id", "ds"]].assign(model_id=m, yhat=fc[m],
                                    yhat_lower=fc[f"{m}-lo-95"], yhat_upper=fc[f"{m}-hi-95"])
     for m in ["AutoETS", "Naive"]]
    + [hist.assign(model_id=None)])  # the history: y, no yhat
long["id"] = long.pop("unique_id").astype(str)
svg = con.execute("SELECT anofox_plot_prediction(r) FROM long AS r").fetchone()[0]
```

### Prophet

`predict()` already uses the contract names (`ds`, `yhat`, `yhat_lower`,
`yhat_upper`); join the history for the observed `y`:

<!-- not run: Prophet is not installed in the docs test environment -->
```python
from prophet import Prophet

m = Prophet().fit(history)                       # columns ds, y
forecast = m.predict(m.make_future_dataframe(periods=30))
p = forecast[["ds", "yhat", "yhat_lower", "yhat_upper"]].merge(history, on="ds", how="left")
svg = con.execute("SELECT anofox_plot_prediction(r) FROM p AS r").fetchone()[0]
```

darts `TimeSeries` → `ts.to_dataframe()` (or `pd_dataframe()`), then the
same wide → long step as statsforecast.

---

## The `anofox_viz` helper package

[`python/`](../python) holds a small package that does the reshaping above
and the DuckDB round trip. It needs `duckdb`, `numpy` and `pandas`;
statsmodels, scikit-learn, scipy and the others are imported only when you
pass their results. It is not on PyPI yet — install it from the repository:

```sh
pip install "git+https://github.com/DataZooDE/anofox-visualization#subdirectory=python"
```

```python
import anofox_viz as av

av.connect("./anofox_visualization.duckdb_extension")   # plot() uses this connection

t = av.from_statsmodels(res)          # {"terms", "obs", "summary", "prediction"}
fig = av.plot(t, "terms")             # an SVG string; Jupyter shows it inline
fig.save("terms.svg")
av.plot(t, "diagnostics")             # the plot.lm 2×2
av.plot(t["obs"], "obs", kind="qq")   # options are macro arguments

c = av.from_sklearn(label, score, model_id="logistic")   # {"curve", "summary"}
roc = av.plot(c["curve"], "curve", by="curve_type")["roc"]   # one SVG per curve type
av.plot(av.from_scipy({"b − a": stats.ttest_ind(b, a)}), "tests")
```

| Function | Input | Returns |
|---|---|---|
| `from_statsmodels(res, model_id=None, alpha=0.05, x=None)` | a fitted result (OLS, WLS, GLM, Logit, …; linearmodels too); a `pairwise_tukeyhsd` result | `{"terms", "obs", "summary", "prediction"}` (`{"tests"}` for Tukey) |
| `from_tukeyhsd(res)` | `pairwise_tukeyhsd(...)` | `tests` |
| `from_sklearn(y_true, y_score, model_id="model", curves=("roc","pr","calibration"), n_bins=10)` | labels and scores of a binary classifier | `{"curve", "summary"}` (auc, average_precision, accuracy, brier, log_loss) |
| `from_estimator(est, X, y, model_id=None)` | a fitted estimator | a classifier: as `from_sklearn`; a regressor: `{"terms"` (linear models)`, "obs", "summary"}` |
| `from_scipy(result \| [results] \| {test_id: result}, method=None, **columns)` | `scipy.stats` test results | `tests` |
| `from_pingouin(df, method=None)` | a pingouin result table | `tests` |
| `from_lifelines(*kmf)` | fitted `KaplanMeierFitter`s | `curve` (km) |
| `from_statsforecast(df, level=None, history=None)` | `forecast()` / `cross_validation()` output | `prediction` (long) |
| `from_prophet(forecast, history=None, model_id="prophet")` | `predict()` output | `prediction` |
| `plot(data, chart=None, *, by=None, con=None, **options)` | a table (pandas, polars, Arrow) or a `from_*` dict | an `SVG` (`str` with `_repr_svg_` and `.save()`), or `{value: SVG}` with `by=` |
| `connect(extension=None, con=None)` | the extension file, else `$ANOFOX_VIZ_EXTENSION`, an installed extension, the community repository, the project's release repository | a DuckDB connection with the extension loaded |

`plot(..., chart="terms")` runs `anofox_plot_terms`; without `chart`,
`anofox_plot` picks the plot from the columns. Keyword options are the
macro's arguments (`title=`, `width=`, `alpha=`, `metrics=[...]`, `kind=`);
a DataFrame option is passed as a list of rows, so
`av.plot(d, "compare", value="y", factor="g", tests=av.from_tukeyhsd(tk))`
draws the significance brackets.

---

## R: broom → DuckDB

broom's names are the contract's with `.` instead of `_` (`std.error`,
`p.value`, `conf.low`) and a leading `.` on augment columns (`.fitted`,
`.resid`, `.hat`, `.cooksd`, `.std.resid`) — all read as they are, so
`tidy()` and `augment()` plot without renaming. `glance()` is one wide
row; pivot it to `metric, value` (metric *values* are not aliased: write
`r_squared`, `aic`, … so the summary plot knows which way is better).

```r
library(DBI)
library(broom)

con <- dbConnect(duckdb::duckdb(config = list(allow_unsigned_extensions = "true")))
dbExecute(con, "LOAD './anofox_visualization.duckdb_extension'")

fit <- lm(mpg ~ wt + hp + qsec, data = mtcars)

duckdb::duckdb_register(con, "terms", tidy(fit, conf.int = TRUE))
svg <- dbGetQuery(con, "SELECT anofox_plot_terms(r) AS svg FROM terms AS r")$svg
writeLines(svg, "terms.svg")

duckdb::duckdb_register(con, "obs", augment(fit))
svg <- dbGetQuery(con, "SELECT anofox_plot_diagnostics(r) AS svg FROM obs AS r")$svg

g <- glance(fit)[c("r.squared", "adj.r.squared", "sigma", "AIC", "BIC")]
duckdb::duckdb_register(con, "glance", data.frame(
  model_id = "lm", metric = tolower(gsub(".", "_", names(g), fixed = TRUE)),
  value = as.numeric(unlist(g))))
svg <- dbGetQuery(con, "SELECT anofox_plot_summary(r) AS svg FROM glance AS r")$svg
```

The same works for `glm()`, `lme4` (`broom.mixed::tidy`) and anything else
with a broom tidier. Base `summary(fit)$coefficients` is a matrix with
`Estimate`, `Std. Error`, `t value`, `Pr(>|t|)` — also read as they are once
the row names are a `term` column:

```r
co <- summary(fit)$coefficients
duckdb::duckdb_register(con, "co", data.frame(term = rownames(co), co, check.names = FALSE))
svg <- dbGetQuery(con, "SELECT anofox_plot_terms(r) AS svg FROM co AS r")$svg
```

Tests: `TukeyHSD()` → `tidy()` gives `contrast`, `estimate`, `conf.low`,
`conf.high`, `adj.p.value` (rename to `p_adj`); `tidy(t.test(...))` gives
one row with `estimate`, `statistic`, `p.value`, `conf.low`, `conf.high`.
Kaplan–Meier: `tidy(survfit(...))` gives `time`, `estimate`, `conf.low`,
`conf.high`, `n.risk`, `n.event`, `n.censor`, `strata`; rename in the query:

```sql
SELECT anofox_plot_curve(r) FROM (
  SELECT 'km' AS curve_type, strata AS model_id, time AS x, estimate AS y,
         "conf.low" AS y_low, "conf.high" AS y_high, "n.risk", "n.event", "n.censor"
  FROM km) r;
```

## Files: Parquet into a dashboard

When the plot belongs in a dashboard (browser builder, `dashboard` CLI,
locked serving), write the shaped table to Parquet and read it from the
`.sql` file:

```python
terms.to_parquet("terms.parquet")         # Python (pyarrow or fastparquet)
```

```r
arrow::write_parquet(tidy(fit, conf.int = TRUE), "terms.parquet")   # R; or write.csv()
```

```sql
-- model_report.sql
SELECT 'Model report'::DASHBOARD_TITLE;
SELECT anofox_plot_terms(r), 'Coefficients (95% CI)'::TITLE FROM 'terms.parquet' AS r;
```

`dashboard --check model_report.sql` validates it, `dashboard
model_report.sql out.html` renders it, and `serve` / the locked server read
the same path; the browser builder reads it from a URL (`FROM
'https://…/terms.parquet'`). Re-running the Python / R script refreshes the
file and the dashboard shows it on its next run. A `-- @requires
terms.parquet` line above the panel shows *Not available* instead of an error
while the file does not exist yet.
