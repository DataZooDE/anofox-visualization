"""Shaping functions: contract column names and values, no extension needed."""

import numpy as np
import pandas as pd
import pytest

import anofox_viz as av

sm = pytest.importorskip("statsmodels.formula.api")


def test_statsmodels_ols(data):
    res = sm.ols("y ~ x1 + x2", data).fit()
    t = av.from_statsmodels(res)
    assert set(t) == {"terms", "obs", "summary", "prediction"}

    terms = t["terms"]
    assert list(terms.columns[:3]) == ["model_id", "term", "estimate"]
    assert {"std_error", "statistic", "p_value", "conf_low", "conf_high"} <= set(terms)
    assert terms.term.tolist() == ["Intercept", "x1", "x2"]
    assert terms.model_id.iloc[0] == "ols"
    np.testing.assert_allclose(terms.estimate, res.params)
    np.testing.assert_allclose(terms.conf_low, res.conf_int()[0])

    obs = t["obs"]
    assert {"row_id", "y", "fitted", "residual", "std_residual", "leverage", "cooks_d"} <= set(obs)
    assert len(obs) == len(data) and obs.n_params.iloc[0] == 3

    s = t["summary"].set_index("metric").value
    assert s["r_squared"] == pytest.approx(res.rsquared)
    assert s["aic"] == pytest.approx(res.aic)

    p = t["prediction"]
    assert {"x", "y", "yhat", "yhat_lower", "yhat_upper"} <= set(p)
    assert (p.yhat_lower <= p.yhat).all() and (p.yhat <= p.yhat_upper).all()


def test_statsmodels_x_and_model_id(data):
    res = sm.ols("y ~ x1", data).fit()
    t = av.from_statsmodels(res, model_id="m1", x="x1", alpha=0.1)
    assert t["terms"].conf_level.iloc[0] == pytest.approx(0.9)
    p = t["prediction"]
    assert p.model_id.iloc[0] == "m1"
    assert p.x.is_monotonic_increasing
    with pytest.raises(KeyError):
        av.from_statsmodels(res, x="nope")


def test_statsmodels_logit_and_glm(data):
    logit = av.from_statsmodels(sm.logit("label ~ x1 + x2", data).fit(disp=0))
    assert "pseudo_r_squared" in logit["summary"].metric.tolist()
    assert logit["prediction"].yhat.between(0, 1).all()
    import statsmodels.api as sma

    glm = av.from_statsmodels(sm.glm("y ~ x1", data, family=sma.families.Gaussian()).fit())
    assert {"fitted", "residual", "cooks_d"} <= set(glm["obs"])
    assert "deviance" in glm["summary"].metric.tolist()


def test_tukeyhsd(data):
    from statsmodels.stats.multicomp import pairwise_tukeyhsd

    tk = pairwise_tukeyhsd(data.y, data.g)
    t = av.from_statsmodels(tk)
    assert set(t) == {"tests"}
    tests = t["tests"]
    assert tests[["group1", "group2"]].values.tolist() == [["a", "b"], ["a", "c"], ["b", "c"]]
    assert {"estimate", "p_adj", "conf_low", "conf_high", "method"} <= set(tests)


def test_sklearn_scores(data):
    pytest.importorskip("sklearn")
    score = 1 / (1 + np.exp(-2 * data.x1))
    t = av.from_sklearn(data.label, score, model_id="clf")
    c = t["curve"]
    assert set(c.curve_type) == {"roc", "pr", "calibration"}
    roc = c[c.curve_type == "roc"]
    assert roc.x.iloc[0] == 0 and roc.x.iloc[-1] == 1
    cal = c[c.curve_type == "calibration"]
    assert cal.weight.sum() == len(data)
    s = t["summary"].set_index("metric").value
    from sklearn.metrics import roc_auc_score

    assert s["auc"] == pytest.approx(roc_auc_score(data.label, score))
    only = av.from_sklearn(data.label, score, curves=["roc"])
    assert set(only["curve"].curve_type) == {"roc"}


def test_sklearn_estimators(data):
    pytest.importorskip("sklearn")
    from sklearn.linear_model import LinearRegression, LogisticRegression

    X = data[["x1", "x2"]]
    reg = av.from_estimator(LinearRegression().fit(X, data.y), X, data.y)
    assert reg["terms"].term.tolist() == ["Intercept", "x1", "x2"]
    assert reg["terms"].model_id.iloc[0] == "linearregression"
    np.testing.assert_allclose(reg["obs"].fitted + reg["obs"].residual, data.y)
    assert set(reg["summary"].metric) == {"r_squared", "rmse", "mae"}
    clf = av.from_estimator(LogisticRegression().fit(X, data.label), X, data.label)
    assert set(clf) == {"curve", "summary"}


def test_scipy(data):
    stats = pytest.importorskip("scipy.stats")
    a, b = data.y[data.g == "a"], data.y[data.g == "b"]
    r = stats.ttest_ind(a, b)
    one = av.from_scipy(r, group1="a", group2="b")
    assert one.method.iloc[0] == "Ttest" and one.group1.iloc[0] == "a"
    assert one.p_value.iloc[0] == pytest.approx(r.pvalue)
    assert one.estimate.iloc[0] == pytest.approx(a.mean() - b.mean())
    assert one.conf_low.iloc[0] < one.estimate.iloc[0] < one.conf_high.iloc[0]

    many = av.from_scipy({"t": r, "normal": stats.shapiro(data.y)}, method="test")
    assert many.test_id.tolist() == ["t", "normal"]
    assert np.isnan(many.conf_low.iloc[1])
    assert len(av.from_scipy([r, r])) == 2

    pr = av.from_scipy(stats.pearsonr(data.x1, data.y))
    assert pr.estimate.iloc[0] == pytest.approx(pr.statistic.iloc[0])


def test_pingouin_style_tables():
    # pairwise_tukey and ttest output, as pingouin returns them.
    tukey = pd.DataFrame(
        {"A": ["a", "a"], "B": ["b", "c"], "diff": [0.3, -0.1], "se": [0.2, 0.2],
         "T": [1.5, -0.5], "p-tukey": [0.3, 0.9]}
    )
    t = av.from_pingouin(tukey, method="Tukey HSD")
    assert {"group1", "group2", "estimate", "statistic", "p_adj"} <= set(t)
    assert t.method.iloc[0] == "Tukey HSD"
    ttest = pd.DataFrame(
        {"T": [2.0], "dof": [38], "p_val": [0.05], "CI95": [np.array([0.1, 1.2])]},
        index=["T-test"],
    )
    t = av.from_pingouin(ttest)
    assert t.conf_low.iloc[0] == 0.1 and t.conf_high.iloc[0] == 1.2
    assert t.conf_level.iloc[0] == 0.95 and t.p_value.iloc[0] == 0.05


def test_lifelines_like():
    class KM:  # the attributes from_lifelines reads from a KaplanMeierFitter
        label = "A"
        idx = pd.Index([0.0, 2.0, 5.0], name="timeline")
        survival_function_ = pd.DataFrame({"A": [1.0, 0.8, 0.5]}, index=idx)
        confidence_interval_survival_function_ = pd.DataFrame(
            {"lo": [1.0, 0.6, 0.3], "hi": [1.0, 0.9, 0.7]}, index=idx
        )
        event_table = pd.DataFrame(
            {"at_risk": [10, 10, 6], "observed": [0, 2, 3], "censored": [0, 0, 1]}, index=idx
        )

    c = av.from_lifelines(KM())
    assert c.curve_type.unique().tolist() == ["km"]
    assert c.columns.tolist() == [
        "curve_type", "model_id", "x", "y", "y_low", "y_high", "n_risk", "n_event", "n_censor",
    ]


def test_prophet_like():
    hist = pd.DataFrame({"ds": pd.date_range("2024-01-01", periods=3), "y": [1.0, 2.0, 3.0]})
    fc = pd.DataFrame(
        {"ds": pd.date_range("2024-01-01", periods=5), "yhat": [1.0, 2, 3, 4, 5],
         "yhat_lower": [0.5] * 5, "yhat_upper": [5.5] * 5, "trend": [0.0] * 5}
    )
    p = av.from_prophet(fc, hist)
    assert "trend" not in p
    assert p.split.tolist() == ["train"] * 3 + ["forecast"] * 2
    assert p.y.isna().sum() == 2


def test_statsforecast_like():
    wide = pd.DataFrame(
        {"unique_id": ["s1", "s1"], "ds": [3, 4], "AutoETS": [3.0, 3.5],
         "AutoETS-lo-95": [2.0, 2.2], "AutoETS-hi-95": [4.0, 4.8], "Naive": [2.0, 2.0]}
    )
    long = av.from_statsforecast(wide, level=95)
    assert sorted(long.model_id.unique()) == ["AutoETS", "Naive"]
    assert {"id", "ds", "yhat", "yhat_lower", "yhat_upper"} <= set(long)
    assert long[long.model_id == "Naive"].yhat_lower.isna().all()
    hist = pd.DataFrame({"unique_id": ["s1"] * 2, "ds": [1, 2], "y": [1.0, 2.0]})
    withh = av.from_statsforecast(wide, level=95, history=hist)
    assert withh.split.tolist()[:2] == ["train", "train"] and len(withh) == 6


def test_statsmodels_single_regressor_is_x(data):
    p = av.from_statsmodels(sm.ols("y ~ x1", data).fit())["prediction"]
    np.testing.assert_allclose(p.x, np.sort(data.x1))
    p2 = av.from_statsmodels(sm.ols("y ~ x1 + x2", data).fit())["prediction"]
    assert p2.x.tolist() == list(range(len(data)))


def test_pingouin_ttest_estimate_is_the_midpoint():
    tt = pd.DataFrame({"T": [2.0], "p-val": [0.05], "CI95%": [[0.2, 1.0]]})
    t = av.from_pingouin(tt)
    assert t.estimate.iloc[0] == pytest.approx(0.6)


def test_pingouin_bf10_is_numeric():
    # pairwise_tests returns BF10 as text; as text it would split the plot into panels.
    pw = pd.DataFrame({"A": [0, 0], "B": [10, 20], "T": [-3.9, -7.1],
                       "p-unc": [0.001, 1e-9], "BF10": ["156.665", "2.648e+09"]})
    t = av.from_pingouin(pw)
    assert t.BF10.tolist() == [156.665, 2.648e9]
