#!/usr/bin/env python3
"""Conformance harness: every anofox-tabular result producer → our plots.

anofox_tabular does not take part in the anofox contract yet: no function
carries `tags['anofox.contract']`, there is no `anofox_contract_version()`, and
none of its outputs matches a contract schema as it comes. So the producers are
every `anofox_tab_*` table function (plus anything tagged
`anofox.contract = '1'`, should upstream start tagging), each with a curated
recipe: the call on small deterministic tables (duplicates, NULLs, outliers,
schema drift) and an adapter that reshapes its output into the nearest
contract schema (summary / events / terms / prediction) or a generic chart.
The adapters name the columns they read, so an upstream rename fails here.

Per producer:
  raw      `anofox_plot(t)` on the output as it comes — expected to match no
           schema; one that renders is reported as NOTE (time to tag it)
  auto     `anofox_plot(t)` on the adapted table (contract adapters only)
  specific the schema's macro on the adapted table
  generic  a chart macro (`anofox_bar`, `anofox_xyc`, …) for shapes no
           schema covers (diff, PII counts, row-level scores)
and every plot call must return an SVG with no SQL error and no unexpected
render warning. Every SVG is written to the output directory, plus a PNG
montage when `rsvg-convert` and Pillow exist.

    python3 scripts/tabular_conformance.py \\
        --duckdb ./duckdb --ext ./anofox_visualization.duckdb_extension \\
        --out /tmp/tabular-viz

The DuckDB CLI must be able to `LOAD anofox_tabular` (install it once with
`INSTALL anofox_tabular FROM community`). Telemetry of anofox_tabular is
switched off for every process (DATAZOO_DISABLE_TELEMETRY=1), and the PII
scans use the pattern recognizers only (no NER model download). Exit status 1
when any plot call fails or a producer has no recipe.

`run`, `svgs_of`, `check_out` and `montage` repeat statistics_conformance.py
(and the forecast / decide / optimize harnesses); a shared helper module could
be factored out once all of them have landed.
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import html
import json
import os
import re
import shutil
import subprocess
import sys

# ── demo data ────────────────────────────────────────────────────────────────
# 120 orders over a month plus two duplicated rows; planted problems: NULL
# customers, an unknown status, malformed emails, missing phones, a negative
# and two 900 € amounts. `d_new` is the migrated copy (phone dropped, note
# added, one row removed, every tenth amount changed); `cust` misses c3.
SETUP = """
CREATE TABLE d AS
SELECT i AS id, CASE i % 3 WHEN 0 THEN 'web' WHEN 1 THEN 'app' ELSE 'shop' END AS channel,
       DATE '2026-06-01' + (i // 4)::INTEGER AS day,
       TIMESTAMP '2026-06-01 00:00' + to_hours(i * 6) AS ts,
       CASE WHEN i IN (7, 77) THEN 900.0 WHEN i = 33 THEN -5.0 ELSE 40 + (i * 37) % 60 + 0.5 * (i % 7) END AS amount,
       1 + (i * 7) % 5 AS qty,
       CASE WHEN i % 11 = 0 THEN NULL ELSE 'c' || (i % 40)::VARCHAR END AS customer,
       CASE WHEN i % 13 = 0 THEN 'unknown' WHEN i % 2 = 0 THEN 'shipped' ELSE 'delivered' END AS status,
       CASE WHEN i % 17 = 0 THEN 'bad-at-example' ELSE 'user' || i::VARCHAR || '@example.com' END AS email,
       CASE WHEN i % 19 = 0 THEN NULL ELSE '+49 30 ' || (1000000 + i * 1237)::VARCHAR END AS phone
FROM range(1, 121) t(i);
INSERT INTO d SELECT * REPLACE (id + 1000 AS id) FROM d WHERE id IN (5, 6);
CREATE TABLE cust AS SELECT DISTINCT customer AS customer_id FROM d WHERE customer IS NOT NULL AND customer <> 'c3';
CREATE TABLE d_new AS
SELECT * EXCLUDE (phone) REPLACE (CASE WHEN id % 10 = 0 THEN amount + 1 ELSE amount END AS amount), 'x' AS note
FROM d WHERE id <> 4;
CREATE TABLE daily AS SELECT day, count(*) AS n, sum(amount) AS revenue FROM d GROUP BY day;
CREATE TABLE checks (check_name VARCHAR, check_type VARCHAR, table_name VARCHAR, column_name VARCHAR, params VARCHAR,
    lower_threshold DOUBLE, upper_threshold DOUBLE, monitor_only BOOLEAN, identifier_column VARCHAR, filter_expr VARCHAR);
INSERT INTO checks VALUES
 ('rows', 'volume', 'd', NULL, NULL, 100, NULL, false, NULL, NULL),
 ('id unique', 'duplicate_count', 'd', 'id', NULL, NULL, 0, false, NULL, NULL),
 ('customer present', 'null_rate', 'd', 'customer', NULL, NULL, 0.05, false, NULL, NULL),
 ('amount positive', 'compliance', 'd', NULL, '{"expression": "amount > 0"}', 1.0, NULL, false, NULL, NULL),
 ('status known', 'values_in_set', 'd', 'status', '{"allowed_values": ["shipped","delivered"]}', 1.0, NULL, false, NULL, NULL),
 ('mean amount', 'agg', 'd', 'amount', '{"agg": "avg"}', 50, 150, false, NULL, NULL),
 ('rows per channel', 'volume', 'd', NULL, NULL, 30, NULL, true, 'channel', NULL);
"""

# Reference dates for the time-aware checks: one call each, stacked.
DAYS = ["2026-06-10", "2026-06-15", "2026-06-20", "2026-06-25", "2026-06-28", "2026-06-30"]


def per_day(call: str) -> str:
    """`call` with {day} filled per reference date, UNION ALL."""
    return "(" + " UNION ALL ".join(f"SELECT * FROM {call.format(day=f'DATE {d!r}')}" for d in DAYS) + ")"


def summary(src: str, metrics: list[str], model: str = "'d'") -> str:
    """One-row (or per-row with `model`) output → summary (model_id, metric, value)."""
    def col(m: str) -> str:  # "expr AS name", or a bare column
        expr, _, alias = m.partition(" AS ")
        return f"({expr})::DOUBLE AS {alias or expr}"
    cols = ", ".join(map(col, metrics))
    return (f"(SELECT model_id, metric, value FROM (UNPIVOT (SELECT {model} AS model_id, {cols} FROM {src}) "
            f"ON COLUMNS(* EXCLUDE (model_id)) INTO NAME metric VALUE value))")


def events(src: str, kind: str, value: str, threshold: str) -> str:
    """Per-reference-date check rows → events (ds, kind, severity, label, value, threshold)."""
    return (f"(SELECT reference_date AS ds, '{kind}' AS kind, "
            f"CASE status WHEN 'fail' THEN 'alarm' WHEN 'warn' THEN 'warning' END AS severity, "
            f"message AS label, ({value})::DOUBLE AS value, ({threshold})::DOUBLE AS threshold FROM {src})")


SUMMARY, EVENTS, TERMS, PRED, GENERIC = "summary", "events", "terms", "prediction", "generic"
SPECIFIC = {
    SUMMARY: "anofox_plot_summary(t)",
    EVENTS: "anofox_plot_events(t)",
    TERMS: "anofox_plot_terms(t)",
    PRED: "anofox_plot_prediction(t)",
}

# Module configuration / status functions: not analysis results, not plotted.
NOT_RESULTS = {"anofox_tab_email_config", "anofox_tab_pii_config", "anofox_tab_pii_status",
               "anofox_tab_phonenumber_status", "anofox_tab_postal_status", "anofox_ner_status"}

# producer → [(variant, source, schema, plot)]: the call, then the adapted
# table (schema in SPECIFIC) or, for GENERIC, the whole plot SELECT over `t`.
RECIPES: dict[str, list[tuple[str, str, str, str]]] = {}


def recipe(name: str, src: str, schema: str, plot: str, variant: str = "") -> None:
    RECIPES.setdefault(name, []).append((variant, src, schema, plot))


def check(name: str, call: str, metrics: list[str]) -> None:
    """A one-row data-quality check → summary of its measured numbers."""
    recipe(name, call, SUMMARY, summary(call, metrics))


check("anofox_tab_agg_check", "anofox_tab_agg_check('d', 'amount', 'avg', 50, 150)",
      ["value", "lower_threshold", "upper_threshold"])
check("anofox_tab_compliance", "anofox_tab_compliance('d', 'amount > 0', 1.0)",
      ["compliance_rate", "threshold"])
check("anofox_tab_distinct_count", "anofox_tab_distinct_count('d', 'channel', 2, 10)",
      ["distinct_count", "min_threshold", "max_threshold"])
check("anofox_tab_duplicate_count", "anofox_tab_duplicate_count('d', 'channel, amount, qty, day', 0)",
      ["duplicate_count", "total_count", "threshold"])
check("anofox_tab_freshness", "anofox_tab_freshness('d', 'ts', INTERVAL 1 DAY, TIMESTAMP '2026-07-15 00:00')",
      ["age_seconds", "epoch(threshold) - epoch(metric_value) AS seconds_past_due"])
check("anofox_tab_iqr", "anofox_tab_iqr('d', 'amount', 1.5)",
      ["q1", "q3", "lower_bound", "upper_bound", "outlier_count"])
check("anofox_tab_match_rate", "anofox_tab_match_rate('d', 'cust', 'customer', 'customer_id', 0.99)",
      ["match_rate", "threshold"])
check("anofox_tab_null_rate", "anofox_tab_null_rate('d', 'customer', 0.05)",
      ["null_rate", "threshold", "null_count"])
check("anofox_tab_occurrence", "anofox_tab_occurrence('d', 'channel', 'max', 1, 50)",
      ["occurrence", "distinct_values", "lower_threshold", "upper_threshold"])
check("anofox_tab_regex_match", "anofox_tab_regex_match('d', 'email', '^[^@]+@[^@]+\\.[a-z]+$', 0.98)",
      ["match_rate", "min_threshold", "max_threshold"])
check("anofox_tab_schema_check", "anofox_tab_schema_check('d_new', ['id', 'amount', 'phone'])",
      ["len(missing_columns) AS missing_columns"])
check("anofox_tab_values_in_set", "anofox_tab_values_in_set('d', 'status', ['shipped', 'delivered'], 1.0)",
      ["in_set_rate", "threshold", "len(sample_violations) AS sample_violations"])
check("anofox_tab_volume", "anofox_tab_volume('d', 100, 1000)",
      ["row_count", "min_threshold", "max_threshold"])
check("anofox_tab_zscore", "anofox_tab_zscore('d', 'amount', 3.0)",
      ["mean", "stddev", "outlier_rate"])
for _f, _cols, _x in (("anofox_tab_dbscan", "'amount'", []), ("anofox_tab_dbscan_mv", "'amount, qty'", ["n_columns"])):
    recipe(_f, f"{_f}('d', {_cols}, 5.0, 3)", SUMMARY,
           summary(f"{_f}('d', {_cols}, 5.0, 3)", ["cluster_count", "noise_count", "noise_rate", "largest_cluster_size"] + _x),
           "summary")
    recipe(_f, f"{_f}('d', {_cols}, 5.0, 3, 'clusters')", GENERIC,
           f"SELECT anofox_xyc(d.amount, d.qty, t.point_type, kind := 'SCATTER') FROM {_f}('d', {_cols}, 5.0, 3, 'clusters') t "
           "JOIN (SELECT *, row_number() OVER () AS row_id FROM d) d USING (row_id)", "clusters")

# run_checks: the suite → summary, one dot per (table, identifier) and a panel per check.
recipe("anofox_tab_run_checks", "anofox_tab_run_checks('checks')", SUMMARY,
       "(SELECT table_name || coalesce(' ' || identifier, '') AS model_id, check_name AS metric, value, "
       "lower_threshold AS conf_low, upper_threshold AS conf_high, status, run_ts, check_type, column_name, message "
       "FROM anofox_tab_run_checks('checks'))")

# Time-aware checks: one call per reference date → events on a timeline.
_mai = "anofox_tab_metric_anomaly_iqr('daily', 'day', 'revenue', 14, 1.5, 'both', {day})"
recipe("anofox_tab_metric_anomaly_iqr", per_day(_mai), EVENTS, events(per_day(_mai), "revenue IQR", "metric_value", "upper_bound"))
_rcc = "anofox_tab_rel_count_change('d', 'day', NULL, 7, -0.5, 0.5, {day})"
recipe("anofox_tab_rel_count_change", per_day(_rcc), EVENTS, events(per_day(_rcc), "row count change", "rel_change", "upper_threshold"))
_rvs = "anofox_tab_rolling_values_in_set('d', 'status', ['shipped', 'delivered'], 'day', 7, 0.95, {day})"
recipe("anofox_tab_rolling_values_in_set", per_day(_rvs), EVENTS, events(per_day(_rvs), "status in set", "in_set_rate", "threshold"))

# Anomaly scores per row: score against the value; the summary modes as summaries.
recipe("anofox_tab_isolation_forest", "anofox_tab_isolation_forest('d', 'amount', 100, 64, 0.05, 'scores', 42)", GENERIC,
       "SELECT anofox_xyc(t.value, t.anomaly_score, CASE WHEN t.is_anomaly THEN 'anomaly' ELSE 'normal' END, kind := 'SCATTER') "
       "FROM anofox_tab_isolation_forest('d', 'amount', 100, 64, 0.05, 'scores', 42) t", "scores")
recipe("anofox_tab_isolation_forest", "anofox_tab_isolation_forest('d', 'amount', 100, 64, 0.05, 'summary', 42)", SUMMARY,
       summary("anofox_tab_isolation_forest('d', 'amount', 100, 64, 0.05, 'summary', 42)",
               ["outlier_count", "total_count", "avg_value", "contamination"]), "summary")
recipe("anofox_tab_isolation_forest_mv", "anofox_tab_isolation_forest_mv('d', 'amount, qty', 100, 64, 0.05, 'scores', 42)", GENERIC,
       "SELECT anofox_xyc(d.amount, d.qty, CASE WHEN t.is_anomaly THEN 'anomaly' ELSE 'normal' END, kind := 'SCATTER') "
       "FROM anofox_tab_isolation_forest_mv('d', 'amount, qty', 100, 64, 0.05, 'scores', 42) t "
       "JOIN (SELECT *, row_number() OVER () AS row_id FROM d) d USING (row_id)", "scores")
recipe("anofox_tab_isolation_forest_mv", "anofox_tab_isolation_forest_mv('d', 'amount, qty', 100, 64, 0.05, 'summary', 42)", SUMMARY,
       summary("anofox_tab_isolation_forest_mv('d', 'amount, qty', 100, 64, 0.05, 'summary', 42)",
               ["outlier_count", "total_count", "n_columns", "contamination"]), "summary")
# outlier_tree: each outlier against its cluster's normal range (a forest row).
recipe("anofox_tab_outlier_tree", "anofox_tab_outlier_tree('d', 'channel,qty,amount', 'outliers')", TERMS,
       "(SELECT 'outlier_tree' AS model_id, column_name || ' #' || row_id AS term, TRY_CAST(outlier_value AS DOUBLE) AS estimate, "
       "lower_bound AS conf_low, upper_bound AS conf_high, z_score AS statistic, cluster_mean, explanation "
       "FROM anofox_tab_outlier_tree('d', 'channel,qty,amount', 'outliers'))", "outliers")
recipe("anofox_tab_outlier_tree", "anofox_tab_outlier_tree('d', 'channel,qty,amount', 'summary')", SUMMARY,
       summary("anofox_tab_outlier_tree('d', 'channel,qty,amount', 'summary')",
               ["total_rows", "outlier_count", "columns_analyzed", "clusters_evaluated", "max_depth_reached"]), "summary")

# Diffs: rows per diff type (bars).
for _f, _k in (("anofox_tab_diff_hashdiff", "'id'"), ("anofox_tab_diff_joindiff", "['id']")):
    recipe(_f, f"{_f}('d', 'd_new', {_k})", GENERIC,
           f"SELECT anofox_bar(diff_type, n) FROM (SELECT diff_type, count(*) AS n FROM {_f}('d', 'd_new', {_k}) GROUP BY ALL) t")

# PII: matches per column and type.
recipe("anofox_tab_pii_scan_table", "anofox_tab_pii_scan_table('d')", GENERIC,
       "SELECT anofox_xyc(column_name, match_count, pii_type) FROM anofox_tab_pii_scan_table('d') t")
recipe("anofox_tab_pii_audit_table", "anofox_tab_pii_audit_table('d')", GENERIC,
       "SELECT anofox_xyc(column_name, n, pii_type) FROM (SELECT column_name, pii_type, count(*) AS n, avg(confidence) AS c, "
       "max(end_pos - start_pos) AS w, any_value(masked_value) AS m FROM anofox_tab_pii_audit_table('d') GROUP BY ALL) t")

# Profiles → summary: a panel per statistic, a dot per column (or pair).
recipe("anofox_tab_profile_table", "anofox_tab_profile_table('d')", SUMMARY,
       "(SELECT model_id, metric, value FROM (UNPIVOT (SELECT column_name AS model_id, null_rate, distinct_rate, "
       "(len(top_values) > 0)::DOUBLE AS has_top_values, is_unique::DOUBLE AS is_unique, row_count::DOUBLE AS row_count "
       "FROM anofox_tab_profile_table('d')) ON COLUMNS(* EXCLUDE (model_id)) INTO NAME metric VALUE value))")
recipe("anofox_tab_profile_summary", "anofox_tab_profile_summary('d')", SUMMARY,
       summary("anofox_tab_profile_summary('d')", ["row_count", "column_count", "numeric_columns", "string_columns",
                                                    "temporal_columns", "total_null_rate", "duplicate_row_count"]))
recipe("anofox_tab_profile_correlations", "anofox_tab_profile_correlations('d')", SUMMARY,
       "(SELECT model_id, metric, value FROM (UNPIVOT (SELECT column_a || ' × ' || column_b AS model_id, pearson, spearman, n::DOUBLE AS n "
       "FROM anofox_tab_profile_correlations('d')) ON COLUMNS(* EXCLUDE (model_id)) INTO NAME metric VALUE value))")


def cases(producers: list[str]) -> list[dict]:
    """Producer names → plot cases (raw probe, auto, specific | generic)."""
    out = []
    for name in producers:
        if name in NOT_RESULTS:
            continue
        if name not in RECIPES:
            out.append({"name": name, "missing": True})
            continue
        for variant, src, schema, plot in RECIPES[name]:
            key = name.removeprefix("anofox_tab_") + (f"[{variant}]" if variant else "")
            raw = src if src.startswith("(") else src
            base = {"name": name, "key": key, "schema": schema}
            out.append({**base, "which": "raw", "sql": f"SELECT anofox_plot(t) FROM {raw} t"})
            if schema == GENERIC:
                out.append({**base, "which": "generic", "sql": plot})
                continue
            for which, call in (("auto", "anofox_plot(t)"), ("specific", SPECIFIC[schema])):
                out.append({**base, "which": which, "sql": f"SELECT {call} FROM {plot} t"})
    return out


# Extra cases: options, overlays, and the shapes the examples and Explore use.
EXTRA = {
    # metric_anomaly_iqr as a band: the day's metric against its IQR bounds, the flags as events.
    "metric_anomaly_band": "SELECT anofox_plot_prediction(p, events := (SELECT list(e) FROM "
    + events(per_day(_mai), "revenue IQR", "metric_value", "upper_bound") + " e WHERE e.severity IS NOT NULL)) FROM "
    "(SELECT 'IQR baseline' AS model_id, reference_date AS ds, metric_value AS y, (q1 + q3) / 2 AS yhat, "
    "lower_bound AS yhat_lower, upper_bound AS yhat_upper FROM " + per_day(_mai) + ") p",
    # All three time-aware checks on one timeline, a lane per check.
    "events_all_checks": "SELECT anofox_plot_events(t) FROM (SELECT * FROM "
    + events(per_day(_mai), "revenue IQR", "metric_value", "upper_bound") + " UNION ALL SELECT * FROM "
    + events(per_day(_rcc), "row count change", "rel_change", "upper_threshold") + " UNION ALL SELECT * FROM "
    + events(per_day(_rvs), "status in set", "in_set_rate", "threshold") + ") t",
    # run_checks as Explore data reads it (web/explore.js tabularChecksSql): these columns must exist.
    "run_checks_explore_columns": "SELECT anofox_plot_summary(t, metrics := ['status known', 'customer present']) FROM "
    "(SELECT 'd' AS model_id, check_name AS metric, value, check_type, lower_threshold, upper_threshold, status, message "
    "FROM anofox_tab_run_checks('checks')) t",
    # run_checks: pass/fail per check type (stacked bars), the status strip a dashboard shows.
    "run_checks_status": "SELECT anofox_xyc(check_type, n, status) FROM (SELECT check_type, status, count(*) AS n "
    "FROM anofox_tab_run_checks('checks') GROUP BY ALL) t",
    # Several single checks side by side: one summary, a panel per metric kind.
    "checks_side_by_side": "SELECT anofox_plot_summary(t) FROM (SELECT 'null_rate' AS model_id, 'rate' AS metric, null_rate AS value FROM "
    "anofox_tab_null_rate('d', 'customer', 0.05) UNION ALL SELECT 'compliance', 'rate', compliance_rate FROM "
    "anofox_tab_compliance('d', 'amount > 0', 1.0) UNION ALL SELECT 'match_rate', 'rate', match_rate FROM "
    "anofox_tab_match_rate('d', 'cust', 'customer', 'customer_id', 0.99) UNION ALL SELECT 'in_set', 'rate', in_set_rate FROM "
    "anofox_tab_values_in_set('d', 'status', ['shipped', 'delivered'], 1.0)) t",
    # The aliases are the same functions: a short name renders the same way.
    "alias_null_rate": "SELECT anofox_plot_summary(t) FROM " + summary("null_rate('d', 'customer', 0.05)", ["null_rate", "threshold"]) + " t",
}

# Build warnings the harness accepts (none today).
ALLOWED_WARNINGS: list[str] = []

# PII: the pattern recognizers only. NAME / ORGANIZATION / LOCATION / MISC use
# an NER model that anofox_tabular downloads from Hugging Face on first use.
PII_TYPES = ("EMAIL,CREDIT_CARD,US_SSN,IBAN,DE_TAX_ID,IP_ADDRESS,URL,MAC_ADDRESS,UK_NINO,US_PASSPORT,PHONE,"
             "CRYPTO_ADDRESS,API_KEY")

# The error anofox_plot gives a table that follows no schema.
NO_SCHEMA = "match no plottable schema"


def run(duckdb: str, ext: str, sql: str, mode: str = "-list", ext_dir: str | None = None) -> tuple[bool, str]:
    pre = f"SET extension_directory = '{ext_dir}'; " if ext_dir else ""
    script = (f"{pre}LOAD anofox_tabular; LOAD '{ext}'; SET anofox_telemetry_enabled = false; "
              f"SET anofox_pii_enabled_types = '{PII_TYPES}';\n{SETUP}\n{sql};")
    env = {**os.environ, "DATAZOO_DISABLE_TELEMETRY": "1"}
    p = subprocess.run([duckdb, "-unsigned", "-noheader", mode, "-c", script],
                       capture_output=True, text=True, timeout=600, env=env)
    # anofox_tabular logs its module registration ("[anofox] [info] …") to stderr.
    err = "\n".join(l for l in (p.stderr or "").splitlines() if not l.startswith("[anofox] [info]"))
    return p.returncode == 0 and not err.strip(), (p.stdout or "") + err


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


def check_out(ok: bool, out: str) -> tuple[str, list[str], str]:
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


def check_raw(ok: bool, out: str) -> tuple[str, list[str], str]:
    """The raw output: no schema is the expected answer, a render a NOTE."""
    if not ok and NO_SCHEMA in out:
        return "NOSCHEMA", [], ""
    status, svgs, msg = check_out(ok, out)
    return ("NOTE" if status == "OK" else status), svgs, msg or "raw output renders: tag it and add a contract case"


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
    ap.add_argument("--out", default="tabular-conformance")
    ap.add_argument("--extension-directory", help="DuckDB extension_directory holding anofox_tabular")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--only", help="regex: run only matching case names")
    ap.add_argument("--no-montage", action="store_true")
    a = ap.parse_args()
    ext = os.path.abspath(a.ext)
    os.makedirs(a.out, exist_ok=True)

    def q(sql: str, mode: str = "-list") -> tuple[bool, str]:
        return run(a.duckdb, ext, sql, mode, a.extension_directory)

    ok, out = q("SELECT 'tabular=' || extension_version || ' (' || installed_from || ')' FROM duckdb_extensions() WHERE extension_name = 'anofox_tabular'; "
                "SELECT 'visualization=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization'; "
                "SELECT 'contract=' || anofox_contract_version()")
    print(out.strip())
    if not ok or "tabular=" not in out:
        print("FAIL: cannot LOAD anofox_tabular and the visualization extension")
        return 1

    # Every anofox_tab_* table function, plus anything tagged for the contract.
    ok, out = q("SELECT DISTINCT function_name || '|' || (tags['anofox.contract'] IS NOT NULL) FROM duckdb_functions() "
                "WHERE function_type = 'table' AND (function_name LIKE 'anofox_tab_%' OR function_name = 'anofox_ner_status' "
                "OR (tags['anofox.contract'] = '1' AND function_name NOT LIKE 'anofox_plot%')) ORDER BY 1")
    rows = [l.split("|") for l in out.splitlines() if l.count("|") == 1]
    producers = sorted({n for n, _ in rows})
    tagged = sorted({n for n, t in rows if t == "true"})
    results_ = [p for p in producers if p not in NOT_RESULTS]
    print(f"{len(producers)} table functions ({len(results_)} result producers, "
          f"{len(producers) - len(results_)} config/status), {len(tagged)} tagged anofox.contract")
    todo = cases(producers)
    missing = [c["name"] for c in todo if c.get("missing")]
    runs = [c for c in todo if not c.get("missing")]
    for k, sql in EXTRA.items():
        runs.append({"name": k, "key": k, "which": "extra", "schema": "", "sql": sql})
    if a.only:
        runs = [c for c in runs if re.search(a.only, c["key"] + "." + c["which"])]

    def one(c):
        ok, out = q(c["sql"], "-json")
        return c, (check_raw if c["which"] == "raw" else check_out)(ok, out)

    results = []
    with cf.ThreadPoolExecutor(a.jobs) as pool:
        for c, (status, svgs, msg) in pool.map(one, runs):
            key = f"{c['key']}.{c['which']}"
            for k, s in enumerate(svgs):
                fn = key if len(svgs) == 1 else f"{key}.{k}"
                with open(os.path.join(a.out, fn + ".svg"), "w") as f:
                    f.write(s)
            results.append((key, status, msg, len(svgs), c))
    names = []
    for key, status, msg, n, _ in sorted(results, key=lambda r: r[0]):
        if status != "NOSCHEMA":
            print(f"{status:4s} {key:48s} {msg}")
        names += [key] if n == 1 else [f"{key}.{k}" for k in range(n)]
    for m in missing:
        print(f"MISS {m}: no recipe in the harness")
    for n in sorted(NOT_RESULTS & set(producers)):
        print(f"SKIP {n}: module configuration / status, not a result")

    # Coverage: producer → schema → plot calls OK / raw detection.
    plots = [r for r in results if r[4]["which"] not in ("raw", "extra")]
    raw = {r[4]["key"]: r[1] for r in results if r[4]["which"] == "raw"}
    print(f"\n{'producer':40s} {'schema':10s} {'plots':6s} raw anofox_plot")
    for key in sorted({r[4]["key"] for r in plots}):
        mine = [r for r in plots if r[4]["key"] == key]
        good = sum(1 for r in mine if r[1] == "OK")
        print(f"{key:40s} {mine[0][4]['schema']:10s} {good}/{len(mine):<4d} "
              f"{'no schema' if raw.get(key) == 'NOSCHEMA' else raw.get(key)}")
    failed = [r for r in results if r[1] not in ("OK", "NOSCHEMA", "NOTE")]
    passed = sum(1 for r in results if r[1] == "OK" and r[4]["which"] != "raw")
    total = sum(1 for r in results if r[4]["which"] != "raw")
    covered = len({c["name"] for c in todo if not c.get("missing")})
    noschema = sum(1 for s in raw.values() if s == "NOSCHEMA")
    print(f"\n{covered}/{len(results_)} producers covered, {passed}/{total} plot calls OK, "
          f"{noschema}/{len(raw)} raw outputs match no contract schema")
    if not a.no_montage:
        names = [n for n in names if os.path.exists(os.path.join(a.out, n + ".svg"))]
        m = montage(a.out, [n for n in names if not n.endswith(".auto") and ".raw" not in n])
        if m:
            print(f"montage: {m}")
    return 0 if not failed and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
