"""plot(): argument handling without the extension, rendering with it."""

import numpy as np
import pandas as pd
import pytest

import anofox_viz as av
from anofox_viz.plot import _literal

TERMS = pd.DataFrame({"term": ["a", "b"], "estimate": [1.0, -0.5],
                      "conf_low": [0.5, -1.0], "conf_high": [1.5, 0.0]})


def test_literal():
    assert _literal(None) == "NULL"
    assert _literal(True) == "true"
    assert _literal(3) == "3"
    assert _literal(0.5) == "0.5"
    assert _literal("it's") == "'it''s'"
    assert _literal(["aic", "bic"]) == "['aic', 'bic']"
    assert _literal(np.float64(1.5)) == "1.5"
    assert _literal(np.int64(2)) == "2" and _literal(np.bool_(False)) == "false"
    with pytest.raises(ValueError):
        _literal(float("nan"))
    with pytest.raises(TypeError):
        _literal(object())


def test_svg_repr(tmp_path):
    s = av.SVG("<svg/>")
    assert s._repr_svg_() == "<svg/>"
    assert s.save(tmp_path / "a.svg").read_text() == "<svg/>"
    assert repr(s) == "<SVG, 6 characters>" and str(s) == "<svg/>"


def test_argument_errors():
    with pytest.raises(ValueError, match="chart must be one of"):
        av.plot(TERMS, "forest")
    with pytest.raises(ValueError, match="pick a table"):
        av.plot({"terms": TERMS})
    with pytest.raises(ValueError, match="no 'curve' table"):
        av.plot({"terms": TERMS}, "curve")
    with pytest.raises(TypeError):
        av.plot([1, 2, 3])


def test_connect_names_the_file_name_rule(tmp_path, monkeypatch):
    # DuckDB takes the entry point from the file name (#201, #230).
    bad = tmp_path / "viz.duckdb_extension"
    bad.write_bytes(b"")
    with pytest.raises(ValueError, match=r"must be named anofox_visualization\.duckdb_extension"):
        av.connect(bad)
    with pytest.raises(ValueError, match="must be named"):
        av.connect(str(tmp_path / "anofox_visualization-v1.duckdb_extension"))
    monkeypatch.setenv("ANOFOX_VIZ_EXTENSION", str(bad))
    with pytest.raises(ValueError, match="must be named"):
        av.connect()
    monkeypatch.delenv("ANOFOX_VIZ_EXTENSION")
    # The canonical names pass the check and reach DuckDB (here: file not found).
    for name in ("anofox_visualization.duckdb_extension",
                 "anofox_visualization.v1.linux_amd64.duckdb_extension"):
        with pytest.raises(Exception) as e:
            av.connect(tmp_path / name)
        assert "must be named" not in str(e.value)


# ── with the extension ──────────────────────────────────────────────────────


def _is_svg(s):
    return isinstance(s, av.SVG) and s.startswith("<svg")


def test_plot_kinds(con, data):
    sm = pytest.importorskip("statsmodels.formula.api")
    t = av.from_statsmodels(sm.ols("y ~ x1 + x2", data).fit())
    for chart in ("terms", "summary", "prediction", "diagnostics", "obs"):
        assert _is_svg(av.plot(t, chart, con=con)), chart
    assert _is_svg(av.plot(t["terms"], con=con))  # anofox_plot picks terms
    qq = av.plot(t["obs"], "obs", kind="qq", con=con)
    assert _is_svg(qq) and qq != av.plot(t["obs"], "obs", con=con)


def test_plot_options(con):
    svg = av.plot(TERMS, "terms", con=con, title="Coefficients", width=500, height=300,
                  reference=0)
    assert 'width="500"' in svg and "Coefficients" in svg


def test_plot_raw_library_columns(con, data):
    """statsmodels' own column names plot through the extension's aliases."""
    sm = pytest.importorskip("statsmodels.formula.api")
    raw = sm.ols("y ~ x1 + x2", data).fit().summary2().tables[1].reset_index(names="term")
    assert "t" in raw.columns  # the column that would hide a table alias `t`
    assert _is_svg(av.plot(raw, "terms", con=con))


def test_plot_by_and_frames(con, data):
    pytest.importorskip("sklearn")
    curve = av.from_sklearn(data.label, 1 / (1 + np.exp(-data.x1)))["curve"]
    out = av.plot(curve, "curve", by="curve_type", con=con)
    assert sorted(out) == ["calibration", "pr", "roc"]
    assert all(_is_svg(v) for v in out.values())
    pl = pytest.importorskip("polars")
    pa = pytest.importorskip("pyarrow")
    assert _is_svg(av.plot(pl.from_pandas(TERMS), "terms", con=con))
    assert _is_svg(av.plot(pa.Table.from_pandas(TERMS), "terms", con=con))


def test_plot_tests_and_compare(con, data):
    from statsmodels.stats.multicomp import pairwise_tukeyhsd

    tests = av.from_tukeyhsd(pairwise_tukeyhsd(data.y, data.g))
    assert _is_svg(av.plot(tests, "tests", con=con, alpha=0.01))
    svg = av.plot(data, "compare", con=con, value="y", factor="g", tests=tests)
    assert _is_svg(svg)


def test_plot_error_names_the_problem(con):
    with pytest.raises(Exception, match="estimate"):
        av.plot(TERMS.drop(columns="estimate"), "terms", con=con)


def test_registered_tables_are_dropped(con):
    av.plot(TERMS, "terms", con=con)
    names = [r[0] for r in con.execute("SHOW TABLES").fetchall()]
    assert not [n for n in names if n.startswith("anofox_")]
