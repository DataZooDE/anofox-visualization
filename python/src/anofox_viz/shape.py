"""Turn Python model results into anofox contract tables.

Each function returns pandas DataFrames whose columns follow the long-table
schemas of the anofox integration contract (``terms``, ``obs``, ``summary``,
``prediction``, ``curve``, ``tests``), so the ``anofox_plot_*`` macros read
them as they are. Libraries other than numpy and pandas are imported only
inside the function that needs them.
"""

from __future__ import annotations

import math
from typing import Any, Iterable, Mapping

import numpy as np
import pandas as pd

__all__ = [
    "from_statsmodels",
    "from_tukeyhsd",
    "from_sklearn",
    "from_estimator",
    "from_scipy",
    "from_pingouin",
    "from_lifelines",
    "from_prophet",
    "from_statsforecast",
]


def _values(v: Any) -> np.ndarray:
    return np.asarray(v, dtype=float).ravel()


def _first(obj: Any, *names: str) -> Any:
    """The first attribute of ``obj`` that exists and is not None."""
    for n in names:
        v = getattr(obj, n, None)
        if v is not None:
            return v
    return None


def _default_model_id(res: Any) -> str:
    model = getattr(res, "model", res)
    return type(model).__name__.lower()


# ── statsmodels (and duck-typed linearmodels) ───────────────────────────────


def _terms(res: Any, model_id: str, alpha: float) -> pd.DataFrame:
    params = res.params
    if isinstance(params, pd.Series):
        names = [str(i) for i in params.index]
    else:
        names = list(getattr(res.model, "exog_names", None) or range(len(params)))
        names = [str(n) for n in names]
    out = pd.DataFrame({"model_id": model_id, "term": names, "estimate": _values(params)})
    se = _first(res, "bse", "std_errors")
    if se is not None:
        out["std_error"] = _values(se)
    stat = _first(res, "tvalues", "tstats")
    if stat is not None:
        out["statistic"] = _values(stat)
    p = _first(res, "pvalues")
    if p is not None:
        out["p_value"] = _values(p)
    try:
        # linearmodels takes `level`, statsmodels `alpha`.
        try:
            ci = res.conf_int(alpha=alpha)
        except TypeError:
            ci = res.conf_int(level=1 - alpha)
        ci = np.asarray(ci, dtype=float).reshape(len(out), 2)
        out["conf_low"], out["conf_high"] = ci[:, 0], ci[:, 1]
        out["conf_level"] = 1 - alpha
    except (AttributeError, NotImplementedError, ValueError):
        pass
    return out


def _obs(res: Any, model_id: str) -> pd.DataFrame | None:
    fitted = _first(res, "fittedvalues", "fitted_values")
    resid = _first(res, "resid", "resid_response", "resids")
    if fitted is None or resid is None:
        return None
    fitted, resid = _values(fitted), _values(resid)
    out = pd.DataFrame({"model_id": model_id, "row_id": np.arange(len(fitted))})
    endog = getattr(getattr(res, "model", None), "endog", None)
    if endog is not None and np.size(endog) == len(fitted):
        out["y"] = _values(endog)
    out["fitted"], out["residual"] = fitted, resid
    try:
        infl = res.get_influence().summary_frame()
        out["std_residual"] = _values(infl["standard_resid"])
        out["leverage"] = _values(infl["hat_diag"])
        out["cooks_d"] = _values(infl["cooks_d"])
    except Exception:  # no influence measures for this model class
        pass
    out["n_params"] = len(np.atleast_1d(res.params))
    return out


# statsmodels attribute → glance metric name (anofox-statistics spelling).
_METRICS = [
    ("rsquared", "r_squared"),
    ("rsquared_adj", "adj_r_squared"),
    ("prsquared", "pseudo_r_squared"),
    ("aic", "aic"),
    ("aicc", "aicc"),
    ("bic", "bic"),
    ("llf", "log_likelihood"),
    ("loglik", "log_likelihood"),
    ("deviance", "deviance"),
    ("null_deviance", "null_deviance"),
    ("fvalue", "f_statistic"),
    ("f_statistic", "f_statistic"),
    ("nobs", "n_observations"),
]


def _summary(res: Any, model_id: str) -> pd.DataFrame:
    rows = []
    seen = set()
    for attr, metric in _METRICS:
        if metric in seen:
            continue
        try:
            v = getattr(res, attr)
            v = float(getattr(v, "stat", v))  # linearmodels wraps F as a test
        except Exception:
            continue
        if math.isfinite(v):
            rows.append((model_id, metric, v))
            seen.add(metric)
    return pd.DataFrame(rows, columns=["model_id", "metric", "value"])


def _prediction(res: Any, model_id: str, alpha: float, x: str | None) -> pd.DataFrame | None:
    try:
        frame = res.get_prediction().summary_frame(alpha=alpha)
    except Exception:
        return None
    model = res.model
    xs: Any = None
    if x is None:
        # One regressor besides the intercept: put the rows on it.
        names = [n for n in (getattr(model, "exog_names", None) or [])
                 if n not in ("Intercept", "const")]
        if len(names) == 1:
            x = names[0]
    if x is not None:
        data = getattr(getattr(model, "data", None), "frame", None)
        if data is None or x not in data or len(data) != len(frame):
            data = pd.DataFrame(model.exog, columns=model.exog_names)
        if x not in data:
            raise KeyError(f"from_statsmodels: x={x!r} is not a model variable")
        xs = _values(data[x])
    out = pd.DataFrame(
        {
            "model_id": model_id,
            "x": xs if xs is not None else np.arange(len(frame)),
            "y": _values(model.endog),
            "yhat": _values(frame["mean" if "mean" in frame else "predicted"]),
        }
    )
    lo = next((c for c in ("obs_ci_lower", "mean_ci_lower", "ci_lower") if c in frame), None)
    if lo is not None:
        hi = lo.replace("lower", "upper")
        out["yhat_lower"], out["yhat_upper"] = _values(frame[lo]), _values(frame[hi])
    return out.sort_values("x", kind="stable").reset_index(drop=True)


def from_statsmodels(
    res: Any,
    model_id: str | None = None,
    alpha: float = 0.05,
    x: str | None = None,
) -> dict[str, pd.DataFrame]:
    """Shape a fitted statsmodels result (OLS, WLS, GLM, Logit, …).

    Returns ``{"terms", "obs", "summary", "prediction"}`` (``obs`` and
    ``prediction`` only when the model provides them). ``prediction`` puts
    the rows on ``x`` (a model variable; by default the only regressor,
    else the row number); its band is the observation interval where statsmodels has one, else the
    mean's. A ``pairwise_tukeyhsd`` result returns ``{"tests": …}``.
    linearmodels results (``std_errors``, ``tstats``) work the same way.
    """
    if hasattr(res, "meandiffs") and hasattr(res, "groupsunique"):
        return {"tests": from_tukeyhsd(res)}
    mid = model_id or _default_model_id(res)
    out = {"terms": _terms(res, mid, alpha)}
    obs = _obs(res, mid)
    if obs is not None:
        out["obs"] = obs
    out["summary"] = _summary(res, mid)
    pred = _prediction(res, mid, alpha, x)
    if pred is not None:
        out["prediction"] = pred
    return out


def from_tukeyhsd(res: Any, method: str = "Tukey HSD") -> pd.DataFrame:
    """``statsmodels.stats.multicomp.pairwise_tukeyhsd`` → ``tests``."""
    t = pd.DataFrame(res.summary().data[1:], columns=res.summary().data[0])
    return pd.DataFrame(
        {
            "method": method,
            "group1": t["group1"].astype(str),
            "group2": t["group2"].astype(str),
            "estimate": t["meandiff"].astype(float),
            "p_adj": t["p-adj"].astype(float),
            "conf_low": t["lower"].astype(float),
            "conf_high": t["upper"].astype(float),
        }
    )


# ── scikit-learn ─────────────────────────────────────────────────────────────


def from_sklearn(
    y_true: Any,
    y_score: Any,
    model_id: str = "model",
    curves: Iterable[str] = ("roc", "pr", "calibration"),
    n_bins: int = 10,
    threshold: float = 0.5,
) -> dict[str, pd.DataFrame]:
    """Binary classifier scores → ``{"curve", "summary"}``.

    ``curve`` stacks ROC, precision–recall and calibration rows
    (``curve_type`` tells them apart; plot one type at a time).
    ``summary`` has auc, average_precision, accuracy (at ``threshold``),
    brier and log_loss.
    """
    from sklearn import metrics
    from sklearn.calibration import calibration_curve

    y = np.asarray(y_true).ravel()
    s = _values(y_score)
    parts = []
    curves = list(curves)
    if "roc" in curves:
        fpr, tpr, th = metrics.roc_curve(y, s)
        parts.append(pd.DataFrame({"curve_type": "roc", "x": fpr, "y": tpr, "threshold": th}))
    if "pr" in curves:
        prec, rec, th = metrics.precision_recall_curve(y, s)
        # One threshold fewer than points: the last point (recall 0) has none.
        th = np.append(th, np.nan)
        parts.append(pd.DataFrame({"curve_type": "pr", "x": rec, "y": prec, "threshold": th}))
    if "calibration" in curves:
        prob_true, prob_pred = calibration_curve(y, s, n_bins=n_bins)
        edges = np.linspace(0, 1, n_bins + 1)
        counts = np.bincount(np.clip(np.digitize(s, edges[1:-1]), 0, n_bins - 1), minlength=n_bins)
        weight = counts[counts > 0].astype(float)
        parts.append(
            pd.DataFrame(
                {"curve_type": "calibration", "x": prob_pred, "y": prob_true, "weight": weight}
            )
        )
    curve = pd.concat(parts, ignore_index=True) if parts else pd.DataFrame()
    if not curve.empty:
        curve.insert(0, "model_id", model_id)
    pred = (s >= threshold).astype(int)
    summary = pd.DataFrame(
        [
            (model_id, "auc", metrics.roc_auc_score(y, s)),
            (model_id, "average_precision", metrics.average_precision_score(y, s)),
            (model_id, "accuracy", metrics.accuracy_score(y, pred)),
            (model_id, "brier", metrics.brier_score_loss(y, s)),
            (model_id, "log_loss", metrics.log_loss(y, np.clip(s, 1e-15, 1 - 1e-15))),
        ],
        columns=["model_id", "metric", "value"],
    )
    return {"curve": curve, "summary": summary}


def from_estimator(
    estimator: Any, X: Any, y: Any, model_id: str | None = None, **kwargs: Any
) -> dict[str, pd.DataFrame]:
    """A fitted scikit-learn estimator and data.

    A classifier (``predict_proba``) goes through :func:`from_sklearn`. A
    regressor gives ``obs`` (fitted, residual), ``summary`` (r_squared,
    rmse, mae) and, for linear models (``coef_``), ``terms`` without
    intervals.
    """
    from sklearn import metrics

    mid = model_id or type(estimator).__name__.lower()
    if hasattr(estimator, "predict_proba"):
        proba = estimator.predict_proba(X)
        return from_sklearn(y, np.asarray(proba)[:, -1], model_id=mid, **kwargs)
    y = _values(y)
    fitted = _values(estimator.predict(X))
    out: dict[str, pd.DataFrame] = {}
    coef = getattr(estimator, "coef_", None)
    if coef is not None and np.ndim(coef) == 1:
        names = getattr(estimator, "feature_names_in_", None)
        if names is None:
            names = [f"x{i}" for i in range(len(coef))]
        terms = [str(n) for n in names]
        est = list(_values(coef))
        intercept = getattr(estimator, "intercept_", None)
        if intercept is not None and np.ndim(intercept) == 0:
            terms.insert(0, "Intercept")
            est.insert(0, float(intercept))
        out["terms"] = pd.DataFrame({"model_id": mid, "term": terms, "estimate": est})
    out["obs"] = pd.DataFrame(
        {
            "model_id": mid,
            "row_id": np.arange(len(y)),
            "y": y,
            "fitted": fitted,
            "residual": y - fitted,
        }
    )
    out["summary"] = pd.DataFrame(
        [
            (mid, "r_squared", metrics.r2_score(y, fitted)),
            (mid, "rmse", math.sqrt(metrics.mean_squared_error(y, fitted))),
            (mid, "mae", metrics.mean_absolute_error(y, fitted)),
        ],
        columns=["model_id", "metric", "value"],
    )
    return out


# ── scipy.stats ──────────────────────────────────────────────────────────────


def _scipy_row(result: Any, method: str | None, conf_level: float) -> dict[str, Any]:
    row: dict[str, Any] = {
        "method": method or type(result).__name__.removesuffix("Result") or "test",
    }
    stat = _first(result, "statistic", "correlation")
    if stat is not None:
        row["statistic"] = float(stat)
    p = _first(result, "pvalue", "p_value")
    if p is not None:
        row["p_value"] = float(p)
    df = getattr(result, "df", None)
    if df is not None and np.ndim(df) == 0:
        row["df1"] = float(df)
    ci = getattr(result, "confidence_interval", None)
    if callable(ci):
        try:
            c = ci(confidence_level=conf_level)
            lo, hi = float(c.low), float(c.high)
        except Exception:  # some results need data scipy no longer has
            return row
        row["conf_low"], row["conf_high"] = lo, hi
        # The interval's estimate: a t-test's mean difference (scipy keeps it
        # as `_estimate`), else the statistic when it lies inside (Pearson's
        # r, a binomial proportion), else the midpoint (symmetric t interval).
        est = getattr(result, "_estimate", None)
        if est is None or np.ndim(est) != 0:
            s = row.get("statistic")
            est = s if s is not None and lo <= s <= hi else (lo + hi) / 2
        row["estimate"] = float(est)
    return row


def from_scipy(
    result: Any,
    method: str | None = None,
    conf_level: float = 0.95,
    **columns: Any,
) -> pd.DataFrame:
    """``scipy.stats`` test result(s) → ``tests``.

    ``result`` is one result (``ttest_ind``, ``mannwhitneyu``, ``shapiro``,
    ``pearsonr``, …), a list of them, or a ``{test_id: result}`` mapping.
    The interval comes from ``confidence_interval()`` when the result has
    one (``estimate`` is then the statistic, e.g. Pearson's r). Extra
    keyword arguments become constant columns (``group1="a"``).
    """
    if isinstance(result, Mapping):
        rows = [{"test_id": str(k), **_scipy_row(v, method, conf_level)} for k, v in result.items()]
    elif isinstance(result, (list, tuple)) and result and not np.isscalar(result[0]):
        rows = [_scipy_row(r, method, conf_level) for r in result]
    else:
        rows = [_scipy_row(result, method, conf_level)]
    out = pd.DataFrame(rows)
    for k, v in columns.items():
        out[k] = v
    return out


# ── pingouin ─────────────────────────────────────────────────────────────────

_PINGOUIN = {
    "A": "group1",
    "B": "group2",
    "diff": "estimate",
    "T": "statistic",
    "U_val": "statistic",
    "W_val": "statistic",
    "F": "statistic",
    "r": "estimate",
    "p_unc": "p_value",
    "p-unc": "p_value",
    "p_val": "p_value",
    "p-val": "p_value",
    "p_corr": "p_adj",
    "p-corr": "p_adj",
    "p_tukey": "p_adj",
    "p-tukey": "p_adj",
    "pval": "p_value",
    "dof": "df1",
    "ddof1": "df1",
    "ddof2": "df2",
}


def from_pingouin(df: pd.DataFrame, method: str | None = None) -> pd.DataFrame:
    """A pingouin result table (``ttest``, ``pairwise_tukey``,
    ``pairwise_tests``, ``corr``, ``anova``, …) → ``tests``.

    Renames A/B, diff, T, p-unc / p-corr / p-tukey and splits the
    ``CI95`` list column into ``conf_low`` / ``conf_high``; without a
    ``diff`` or ``r`` column, ``estimate`` is the interval's midpoint (a
    t-test's mean difference).
    """
    out = df.reset_index(drop=True).copy()
    ci = next((c for c in out.columns if str(c).upper().startswith("CI")), None)
    if ci is not None:
        bounds = np.array([np.asarray(v, dtype=float).ravel()[:2] for v in out[ci]])
        out = out.drop(columns=[ci])
        out["conf_low"], out["conf_high"] = bounds[:, 0], bounds[:, 1]
        level = "".join(ch for ch in str(ci) if ch.isdigit())
        if level:
            out["conf_level"] = float(level) / 100
    if "conf_low" in out and not any(c in out for c in ("diff", "r", "estimate")):
        # A t-test's interval is of the difference, centred on it.
        out["estimate"] = (out["conf_low"] + out["conf_high"]) / 2
    rename = {}
    for c in out.columns:
        target = _PINGOUIN.get(str(c))
        if target and target not in out.columns and target not in rename.values():
            rename[c] = target
    out = out.rename(columns=rename)
    if "method" not in out:
        out.insert(0, "method", method or "pingouin")
    for c in ("group1", "group2"):
        if c in out:
            out[c] = out[c].astype(str)
    return out


# ── lifelines ────────────────────────────────────────────────────────────────


def from_lifelines(*fitters: Any) -> pd.DataFrame:
    """Kaplan–Meier fitter(s) → ``curve`` (``curve_type = 'km'``).

    One curve per fitter, named by its ``label``; the band, the number at
    risk, events and censored counts come along.
    """
    parts = []
    for f in fitters:
        sf = f.survival_function_
        ci = f.confidence_interval_survival_function_
        et = f.event_table.reindex(sf.index)
        parts.append(
            pd.DataFrame(
                {
                    "curve_type": "km",
                    "model_id": str(getattr(f, "label", None) or sf.columns[0]),
                    "x": _values(sf.index),
                    "y": _values(sf.iloc[:, 0]),
                    "y_low": _values(ci.iloc[:, 0]),
                    "y_high": _values(ci.iloc[:, 1]),
                    "n_risk": _values(et["at_risk"]),
                    "n_event": _values(et["observed"]),
                    "n_censor": _values(et["censored"]),
                }
            )
        )
    return pd.concat(parts, ignore_index=True)


# ── forecasting ──────────────────────────────────────────────────────────────


def from_prophet(
    forecast: pd.DataFrame, history: pd.DataFrame | None = None, model_id: str = "prophet"
) -> pd.DataFrame:
    """Prophet's ``predict()`` frame (+ the training frame for ``y``) →
    ``prediction``. Rows after the history are marked ``split = 'forecast'``.
    """
    cols = [c for c in ("ds", "yhat", "yhat_lower", "yhat_upper") if c in forecast]
    out = forecast[cols].copy()
    out.insert(0, "model_id", model_id)
    if history is not None:
        out = out.merge(history[["ds", "y"]], on="ds", how="left")
        last = pd.to_datetime(history["ds"]).max()
        out["split"] = np.where(pd.to_datetime(out["ds"]) > last, "forecast", "train")
    return out


def from_statsforecast(
    df: pd.DataFrame,
    level: int | None = None,
    history: pd.DataFrame | None = None,
) -> pd.DataFrame:
    """statsforecast's wide ``forecast()`` / ``cross_validation()`` frame
    → long ``prediction`` (one ``model_id`` per model column).

    ``level`` picks the ``<model>-lo-<level>`` / ``-hi-`` interval
    columns. ``history`` (``unique_id, ds, y``) adds the observed
    values before the forecast.
    """
    df = df.reset_index() if "unique_id" not in df and df.index.name == "unique_id" else df
    keys = [c for c in ("unique_id", "ds", "cutoff", "y") if c in df]
    models = [c for c in df.columns if c not in keys and "-lo-" not in c and "-hi-" not in c]
    parts = []
    for m in models:
        p = df[keys].copy()
        p["model_id"] = m
        p["yhat"] = df[m].astype(float)
        if level is not None and f"{m}-lo-{level}" in df:
            p["yhat_lower"] = df[f"{m}-lo-{level}"].astype(float)
            p["yhat_upper"] = df[f"{m}-hi-{level}"].astype(float)
        parts.append(p)
    out = pd.concat(parts, ignore_index=True)
    if history is not None:
        h = history[[c for c in ("unique_id", "ds", "y") if c in history]].copy()
        h["split"] = "train"
        out["split"] = "forecast" if "cutoff" not in out else "test"
        out = pd.concat([h, out], ignore_index=True)
    if "unique_id" in out:
        out = out.rename(columns={"unique_id": "id"})
    return out
