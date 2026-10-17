#!/usr/bin/env python3
"""Conformance harness: every anofox-statistics contract producer → our plots.

Calls each of the functions anofox_statistics tags with
`tags['anofox.contract'] = '1'` (table macros directly, the `*_agg` test
aggregates through `unnest`), hands the output to `anofox_plot(...)` and to
the schema's specific macro, and checks that every call returns an SVG with
no SQL error and no unexpected render warning. Every SVG is written to the
output directory, plus a PNG montage when `rsvg-convert` and Pillow exist.

    python3 scripts/statistics_conformance.py \
        --duckdb ./duckdb --ext ./anofox_visualization.duckdb_extension \
        --out /tmp/stats-viz

The DuckDB CLI must be able to `LOAD anofox_statistics` (install it once with
`INSTALL anofox_statistics FROM 'https://get.erpl.io'` in `duckdb -unsigned`).
Exit status 1 when any case fails or a producer is not covered.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import html
import json
import os
os.environ["DATAZOO_DISABLE_TELEMETRY"] = "1"  # no usage telemetry from anofox extensions, whatever was inherited
import re
import shutil
import subprocess
import sys

# ── demo data ────────────────────────────────────────────────────────────────
# Two groups (A, B) of 60 rows: every family has a sensible response.
SETUP = """
CREATE TABLE d AS
SELECT i AS id, CASE WHEN i % 2 = 0 THEN 'A' ELSE 'B' END AS grp,
       i::DOUBLE AS x1, ((i * 3) % 7)::DOUBLE AS x2,
       1.0 + 0.5 * i - 0.3 * ((i * 3) % 7) + sin(i) AS y,
       round(exp(0.2 + 0.03 * i + 0.3 * sin(i)))::DOUBLE AS y_count,
       (sin(i) + 0.05 * i > 0.8)::DOUBLE AS y_bin,
       (sin(i) + 0.05 * i > 0.8) AS label,
       1.0 / (1.0 + exp(-(sin(i) + 0.05 * i - 0.8))) AS prob,
       exp(0.5 + 0.02 * i + 0.2 * cos(i)) AS y_pos,
       1.0 + (i % 4)::DOUBLE AS w,
       (2 + (i % 13) + (i % 3) * 2)::DOUBLE AS time,
       (i % 4 <> 0) AS event, (i % 4 <> 0)::DOUBLE AS event_d,
       (i % 3)::BIGINT AS g3,
       ((i // 2) % 2)::BIGINT AS g2,
       CASE i % 3 WHEN 0 THEN 'low' WHEN 1 THEN 'mid' ELSE 'high' END AS level3,
       (i % 3)::INTEGER AS r3, ((i * 7) % 3)::INTEGER AS c3,
       ((i // 3) % 2)::BIGINT AS b1, ((i // 2) % 2)::BIGINT AS b2,
       (i % 5)::BIGINT AS subj, (i % 3)::BIGINT AS rater,
       1.0 + 0.5 * i + 0.8 * sin(i) AS f1, 1.0 + 0.5 * i + 1.2 * cos(i) AS f2,
       ((i * 7) % 3 = 0)::BIGINT AS hit
FROM range(1, 61) t(i);
"""

TERMS, SUMMARY, OBS, PRED, CURVE, TEST = "terms", "summary", "obs", "prediction", "curve", "test"
SPECIFIC = {
    TERMS: "anofox_plot_terms(t)",
    SUMMARY: "anofox_plot_summary(t)",
    OBS: "anofox_plot_diagnostics(t)",
    PRED: "anofox_plot_prediction(t)",
    CURVE: "anofox_plot_curve(t)",
    TEST: "anofox_plot_tests(t)",
}

LINEAR = ["ols", "ridge", "elasticnet", "rls", "huber", "ransac", "theil_sen", "bls",
          "nnls", "pls", "quantile", "lars", "alm"]
GLM = {"poisson": "y_count", "binomial": "y_bin", "logistic": "y_bin",
       "negbinom": "y_count", "gamma": "y_pos", "tweedie": "y_count"}
X = "[x1, x2]"


def response(model: str) -> str:
    return GLM.get(model, "y")


def producer_sql(name: str) -> str | None:
    """The table a producer emits (both groups), or None when unknown."""
    # online / sequential fits: an order column before y
    m = re.fullmatch(r"(lm_dynamic|passive_aggressive)_(tidy|glance)_by", name)
    if m:
        return f"{name}('d', grp, id, y, {X})"
    if name == "lowess_by":
        return "lowess_by('d', grp, x1, y)"
    m = re.fullmatch(r"(\w+?)_(tidy|glance|augment|prediction|effects|rolling_tidy|path|cv)_by", name)
    if m:
        model, kind = m.groups()
        y = response(model)
        if kind == "prediction":
            if model == "isotonic":
                return f"isotonic_prediction_by('d', grp, x1, y)"
            extra = ", w" if model == "wls" else ""
            return f"{name}('d', grp, id, {y}, {X}{extra}, split := CASE WHEN id > 50 THEN 'test' END)"
        if kind == "rolling_tidy":
            extra = ", w" if model == "wls" else ""
            return f"{name}('d', grp, id, {y}, {X}{extra}, window_size := 15)"
        extra = {"wls": ", w", "aft": ", event_d"}.get(model, "")
        y = "time" if model == "aft" else y
        names = ", names := ['trend', 'cycle']" if kind in ("tidy", "path") else ""
        return f"{name}('d', grp, {y}, {X}{extra}{names})"
    if name == "quantile_process_by":
        return "quantile_process_by('d', grp, y, [x1, x2])"
    if name in ("acf_by", "pacf_by"):
        return f"{name}('d', grp, y, id)"
    if name in ("roc_curve_by", "pr_curve_by"):
        return f"{name}('d', grp, label, prob)"
    if name == "calibration_curve_by":
        return "calibration_curve_by('d', grp, label, prob, n_bins := 5)"
    if name == "kaplan_meier_by":
        return "kaplan_meier_by('d', grp, time, event)"
    if name in ("tukey_hsd_by", "pairwise_t_test_by", "dunn_test_by"):
        return f"{name}('d', grp, y, level3)"
    return None


# Test aggregates: the argument list per function (unnested per group).
AGG_ARGS = {
    "binom_test_agg": "hit", "prop_test_one_agg": "hit",
    "prop_test_two_agg": "hit, g2",
    "box_pierce_agg": "y ORDER BY id", "ljung_box_agg": "y ORDER BY id",
    "breusch_pagan_agg": "y - 0.5 * x1, [x1, x2]", "white_test_agg": "y - 0.5 * x1, [x1, x2]",
    "durbin_watson_agg": "y - 0.5 * x1 ORDER BY id",
    "chisq_gof_agg": "n, 1.0 / 3", "chisq_test_agg": "r3, c3", "fisher_exact_agg": "b1::INTEGER, b2::INTEGER",
    "g_test_agg": "r3::BIGINT, c3::BIGINT", "mcnemar_agg": "b1, b2", "cohen_kappa_agg": "b1, b2",
    "icc_agg": "y, subj, rater",
    "clark_west_agg": "y, f1, f2", "diebold_mariano_agg": "y, f1, f2",
    "dagostino_k2_agg": "y", "jarque_bera_agg": "y", "shapiro_wilk_agg": "y",
    "pearson_agg": "x1, y", "spearman_agg": "x1, y", "kendall_agg": "x1, y",
    "distance_cor_agg": "x1, y", "tost_correlation_agg": "x1, y",
    "tost_paired_agg": "f1, f2", "wilcoxon_signed_rank_agg": "f1, f2",
    "log_rank_test_agg": "time, event, level3",
    "energy_distance_agg": "y, g2::INTEGER", "mmd_agg": "y, g2::INTEGER",
    "tost_t_test_agg": "y, g2::INTEGER",
}
for _f in ("t_test_agg", "mann_whitney_u_agg", "brunner_munzel_agg", "permutation_t_test_agg",
           "yuen_agg"):
    AGG_ARGS[_f] = "y, g2"
for _f in ("brown_forsythe_agg", "kruskal_wallis_agg", "one_way_anova_agg"):
    AGG_ARGS[_f] = "y, level3"
for _f in ("tukey_hsd_agg", "pairwise_t_test_agg", "dunn_test_agg"):
    AGG_ARGS[_f] = "y, level3"


# Aggregates that need other rows than the demo table's.
AGG_FROM = {
    "chisq_gof_agg": "(SELECT grp, level3, count(*) AS n FROM d GROUP BY grp, level3)",
}


def agg_sql(name: str) -> str | None:
    args = AGG_ARGS.get(name)
    if args is None:
        return None
    src = AGG_FROM.get(name, "d")
    return f"(SELECT grp, unnest({name}({args}), recursive := true) FROM {src} GROUP BY grp)"


def cases(producers: list[tuple[str, str, str]]) -> list[dict]:
    """(function_name, function_type, output) → plot cases."""
    out = []
    for name, ftype, schema in producers:
        src = agg_sql(name) if ftype == "aggregate" else producer_sql(name)
        if src is None:
            out.append({"name": name, "schema": schema, "missing": True})
            continue
        frm = src if src.startswith("(") else src
        for which, call in (("auto", "anofox_plot(t)"), ("specific", SPECIFIC[schema])):
            out.append({
                "name": name, "schema": schema, "which": which,
                "sql": f"SELECT {call} FROM {frm} t",
                "source": src,
            })
    return out


# Extra cases beyond the per-producer pair: options, model comparison, and the
# non-contract partial residuals.
EXTRA = {
    "terms_two_models": "SELECT anofox_plot_terms(t) FROM (SELECT * FROM ols_tidy_by('d', grp, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM huber_tidy_by('d', grp, y, [x1, x2])) t WHERE grp = 'A'",
    "summary_two_models": "SELECT anofox_plot_summary(t) FROM (SELECT * FROM ols_glance_by('d', grp, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM huber_glance_by('d', grp, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM ridge_glance_by('d', grp, y, [x1, x2])) t WHERE grp = 'A'",
    "summary_metrics": "SELECT anofox_plot_summary(t, metrics := ['r_squared', 'aic']) FROM (SELECT * FROM ols_glance_by('d', grp, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM huber_glance_by('d', grp, y, [x1, x2])) t",
    "prediction_two_models": "SELECT anofox_plot_prediction(t) FROM (SELECT * FROM ols_prediction_by('d', grp, id, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM huber_prediction_by('d', grp, id, y, [x1, x2])) t WHERE grp = 'A'",
    "prediction_two_models_facets": "SELECT anofox_plot_prediction(t) FROM (SELECT * FROM ols_prediction_by('d', grp, id, y, [x1, x2]) UNION ALL BY NAME SELECT * FROM huber_prediction_by('d', grp, id, y, [x1, x2])) t",
    "prediction_future": "CREATE TABLE dfut AS SELECT * REPLACE (CASE WHEN id > 54 THEN NULL ELSE y END AS y) FROM d; SELECT anofox_plot_prediction(t) FROM ols_prediction_by('dfut', grp, id, y, [x1, x2], split := CASE WHEN id BETWEEN 49 AND 54 THEN 'test' END) t",
    "fit_predict_by": "SELECT anofox_plot_prediction(t, x := 'x1', facet := 'grp') FROM ols_fit_predict_by('d', grp, y, [x1, x2]) t",
    "obs_leverage": "SELECT anofox_plot_obs(t, kind := 'leverage') FROM ols_augment_by('d', grp, y, [x1, x2]) t WHERE grp = 'A'",
    "obs_cooks": "SELECT anofox_plot_obs(t, kind := 'cooks') FROM ols_augment_by('d', grp, y, [x1, x2]) t WHERE grp = 'A'",
    "curve_roc_calibration_grouped": "SELECT curve_type, anofox_plot_curve(t) FROM (SELECT * FROM roc_curve_by('d', grp, label, prob) UNION ALL SELECT * FROM calibration_curve_by('d', grp, label, prob, n_bins := 4)) t GROUP BY curve_type",
    "partial_component": "SELECT anofox_plot_partial(t) FROM ols_partial_residuals_by('d', grp, y, [x1, x2], names := ['trend', 'cycle']) t WHERE grp = 'A'",
    "partial_added_variable": "SELECT anofox_plot_partial(t, kind := 'added_variable') FROM ols_partial_residuals_by('d', grp, y, [x1, x2]) t WHERE grp = 'A'",
    "partial_auto": "SELECT anofox_plot(t) FROM poisson_partial_residuals_by('d', grp, y_count, [x1, x2]) t WHERE grp = 'A'",
    "tests_by_grouped": "SELECT grp, anofox_plot_tests(t) FROM tukey_hsd_by('d', grp, y, level3) t GROUP BY grp",
    "tests_alpha": "SELECT anofox_plot_tests(t, alpha := 0.01) FROM pairwise_t_test_by('d', grp, y, level3) t WHERE grp = 'A'",
    "tests_struct_column": "SELECT anofox_plot_tests(s) FROM (SELECT tukey_hsd_agg(y, level3) AS r FROM d) s",
    "compare_box": "SELECT anofox_plot_compare(r, value := 'y', factor := 'level3', tests := (SELECT list(t) FROM tukey_hsd_by('d', grp, y, level3) t WHERE t.grp = 'A')) FROM d r WHERE grp = 'A'",
    "compare_agg": "SELECT anofox_plot_compare(r, value := 'y', factor := 'level3', tests := tukey_hsd_agg(y, level3), kind := 'violin') FROM d r",
    "compare_points_grouped": "SELECT grp, anofox_plot_compare(r, value := 'y', factor := 'level3', tests := (SELECT list(t) FROM dunn_test_by('d', grp, y, level3) t WHERE t.grp = r.grp), kind := 'points') FROM d r GROUP BY grp",
    "compare_two_sample": "SELECT anofox_plot_compare(r, value := 'y', factor := 'g2', tests := t_test_agg(y, g2)) FROM d r",
    "contract_version": "SELECT CASE WHEN anofox_contract_version() = '1' THEN '<svg>ok</svg>' END AS v",
}

# Build warnings the harness accepts (none today).
ALLOWED_WARNINGS: list[str] = []


def run(duckdb: str, ext: str, sql: str, mode: str = "-list") -> tuple[bool, str]:
    script = f"LOAD anofox_statistics; LOAD '{ext}';\n{SETUP}\n{sql};"
    p = subprocess.run([duckdb, "-unsigned", "-noheader", mode, "-c", script],
                       capture_output=True, text=True, timeout=600)
    out = (p.stdout or "") + (p.stderr or "")
    return p.returncode == 0 and not p.stderr.strip(), out


def svgs_of(out: str) -> list[str]:
    """Every SVG cell of the `-json` results (grids nest <svg> elements)."""
    dec, i, cells = json.JSONDecoder(), 0, []
    while (j := out.find("[", i)) >= 0:
        try:
            rows, i = dec.raw_decode(out, j)
        except ValueError:
            i = j + 1
            continue
        cells += [v for r in rows if isinstance(r, dict) for v in r.values()
                  if isinstance(v, str) and v.startswith("<svg")]
    return cells


def check(ok: bool, out: str) -> tuple[str, list[str], str]:
    svgs = svgs_of(out) if ok else []
    if not svgs:
        return "FAIL", [], out.strip().replace("\n", " ")[:400]
    warnings = []
    for s in svgs:
        w = re.search(r'data-warnings="([^"]*)"', s)
        if w:
            warnings += json.loads(html.unescape(w.group(1)))
    bad = [w for w in warnings if not any(a in w for a in ALLOWED_WARNINGS)]
    if bad:
        return "WARN", svgs, "; ".join(bad)[:400]
    return "OK", svgs, ""


def montage(out_dir: str, names: list[str]) -> str | None:
    if not shutil.which("rsvg-convert"):
        return None
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        return None
    pngs = []
    png_dir = os.path.join(out_dir, "png")
    os.makedirs(png_dir, exist_ok=True)
    for n in names:
        svg = os.path.join(out_dir, n + ".svg")
        png = os.path.join(png_dir, n + ".png")
        r = subprocess.run(["rsvg-convert", "-b", "white", "-w", "480", svg, "-o", png],
                           capture_output=True)
        if r.returncode == 0:
            pngs.append((n, png))
    if not pngs:
        return None
    cols, tw, th = 6, 480, 330
    rows = (len(pngs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * tw, rows * (th + 18)), "white")
    draw = ImageDraw.Draw(sheet)
    for k, (n, png) in enumerate(pngs):
        im = Image.open(png).convert("RGB")
        im.thumbnail((tw, th))
        x, y = (k % cols) * tw, (k // cols) * (th + 18)
        sheet.paste(im, (x, y + 18))
        draw.text((x + 4, y + 2), n, fill=(180, 30, 30))
    path = os.path.join(out_dir, "montage.png")
    sheet.save(path)
    return path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--duckdb", default=os.environ.get("DUCKDB", "duckdb"))
    ap.add_argument("--ext", required=True, help="anofox_visualization extension file to LOAD")
    ap.add_argument("--out", default="statistics-conformance")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--only", help="regex: run only matching case names")
    ap.add_argument("--no-montage", action="store_true")
    a = ap.parse_args()
    ext = os.path.abspath(a.ext)
    os.makedirs(a.out, exist_ok=True)

    ok, out = run(a.duckdb, ext, "SELECT 'version=' || anofox_contract_version(); "
                  "SELECT 'statistics=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_statistics'; "
                  "SELECT 'visualization=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization'")
    print(out.strip())
    if not ok or "version=1" not in out:
        print("FAIL: anofox_contract_version() must be '1'")
        return 1

    ok, out = run(a.duckdb, ext, "SELECT function_name || '|' || function_type || '|' || tags['anofox.output'] "
                  "FROM (SELECT DISTINCT function_name, function_type, tags FROM duckdb_functions() "
                  "WHERE tags['anofox.contract'] = '1' AND tags['anofox.output'] IS NOT NULL) ORDER BY 1")
    producers = sorted({tuple(l.split("|")) for l in out.splitlines() if l.count("|") == 2})
    print(f"{len(producers)} contract producers")
    todo = cases(producers)
    missing = [c["name"] for c in todo if c.get("missing")]
    runs = [c for c in todo if not c.get("missing")]
    for k, sql in EXTRA.items():
        runs.append({"name": k, "which": "extra", "schema": "", "sql": sql})
    if a.only:
        runs = [c for c in runs if re.search(a.only, c["name"] + "." + c["which"])]

    def one(c):
        ok, out = run(a.duckdb, ext, c["sql"], "-json")
        return c, check(ok, out)

    results = []
    with cf.ThreadPoolExecutor(a.jobs) as pool:
        for c, (status, svgs, msg) in pool.map(one, runs):
            key = f"{c['name']}.{c['which']}"
            for k, s in enumerate(svgs):
                fn = key if len(svgs) == 1 else f"{key}.{k}"
                with open(os.path.join(a.out, fn + ".svg"), "w") as f:
                    f.write(s)
            results.append((key, status, msg, len(svgs)))
    names = []
    for key, status, msg, n in sorted(results):
        print(f"{status:4s} {key:44s} {msg}")
        names += [key] if n == 1 else [f"{key}.{k}" for k in range(n)]
    for m in missing:
        print(f"MISS {m}: no argument recipe in the harness")
    passed = sum(1 for r in results if r[1] == "OK")
    covered = len({c["name"] for c in todo if not c.get("missing")})
    print(f"\n{covered}/{len(producers)} producers covered, {passed}/{len(results)} plot calls OK")
    if not a.no_montage:
        names = [n for n in names if os.path.exists(os.path.join(a.out, n + ".svg"))]
        m = montage(a.out, [n for n in names if not n.startswith("contract_version")
                            and not n.endswith(".auto")])
        if m:
            print(f"montage: {m}")
    return 0 if passed == len(results) and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
