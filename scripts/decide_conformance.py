#!/usr/bin/env python3
"""Conformance harness: anofox-decide output → our plots.

anofox_decide tags no function with `tags['anofox.contract'] = '1'` yet, and
none of its outputs is a contract schema: probabilities, choices and scores
are per-row scalars, the quality metrics single numbers. So the harness

  * discovers tagged producers anyway (any `decide_*` function with an
    `anofox.output` tag) and fails while one has no recipe here;
  * runs every decide output through the SQL bridge the decide examples use
    to a contract schema (metrics → `summary`, probabilities → `curve`: ROC,
    PR, calibration) and hands it to `anofox_plot(...)` and the schema's
    specific macro;
  * with `anofox_statistics` loadable (`--statistics`), also feeds decide's
    probabilities to its `roc_curve_by` / `pr_curve_by` /
    `calibration_curve_by`;
  * pins the raw outputs that do not plot (`decide_table`, `decide_answers`)
    as known gaps: they must fail with the clean "match no plottable schema"
    error, and a gap that starts to render fails too, so this list follows.
    The error must not name a "closest" schema.

Every call must return an SVG with no SQL error and no unexpected render
warning. Every SVG is written to the output directory, plus a PNG montage when
`rsvg-convert` and Pillow exist.

    python3 scripts/decide_conformance.py \
        --duckdb ./duckdb --ext ./anofox_visualization.duckdb_extension \
        --out /tmp/decide-viz

The DuckDB CLI must be able to `LOAD anofox_decide` (`INSTALL anofox_decide
FROM community` once). The answers come from the built-in `stub` model,
registered under five Platt calibrations so that a row's model, picked from
its label, gives probabilities with spread: offline and deterministic, it
exercises the output types and the plots, not answer quality. `--model
laya-multilingual` (after `CALL decide_download('laya-multilingual')`) scores
the same tickets with a real model next to the fixture. Telemetry is off
(DATAZOO_DISABLE_TELEMETRY=1). Exit status 1 when any case fails or a tagged
producer is not covered.
"""

from __future__ import annotations

import html
import json
import os
import re
import sys

from conformance_lib import (NO_SCHEMA, ext_path, parser, run_all, run_duckdb, svg_names, svgs_of,
                              write_montage)

# ── demo data ────────────────────────────────────────────────────────────────
# 90 support tickets in two groups (A, B): a team (billing / defect / other),
# whether a refund is asked for, and how frustrated the writer is (0..2).
TICKETS = """
CREATE TABLE tickets AS
SELECT i AS id, CASE WHEN i % 2 = 0 THEN 'A' ELSE 'B' END AS grp,
       ['billing', 'defect', 'other'][1 + i % 3] AS team,
       i % 3 = 0 AND i % 4 <> 1 AS refund,
       (i * 7) % 3 AS mood,
       concat_ws(' ',
           ['I was charged twice for my invoice.', 'The app crashes when I open the export dialog.',
            'Do you have an office in Munich?'][1 + i % 3],
           CASE WHEN i % 3 = 0 AND i % 4 <> 1 THEN 'Please refund the extra amount.' END,
           ['Thanks a lot.', 'This is annoying.', 'This is unacceptable, I am furious!'][1 + (i * 7) % 3]) AS text
FROM range(1, 91) t(i);
"""

# Five calibrated stubs: s0 … s4 answer sigmoid(-1.8) … sigmoid(1.8).
STUBS = ("SELECT bool_and(decide_register_model('s' || k, 'stub', "
         "MAP{'calibration': 'platt:1,' || ((k - 2) * 0.9)::VARCHAR})) FROM range(5) r(k)")

QUESTIONS = ('[{"id": "refund", "kind": "binary", "instruction": "A refund is requested."},'
             ' {"id": "team", "kind": "choice", "instruction": "Which team owns this?",'
             ' "options": ["billing", "defect", "other"]},'
             ' {"id": "mood", "kind": "score", "instruction": "How frustrated is the writer?",'
             ' "levels": ["calm", "annoyed", "furious"]}]')


def fixture_sql(model: str | None) -> str:
    """`runs`: one row per (model_id, ticket) with every per-row decide output.

    The fixture models (fixture-a, fixture-b) pick a calibrated stub from the
    label plus a deterministic offset, so the probability tracks the label
    imperfectly; their choice is the first of the options rotated per row.
    """
    fixtures = []
    for name, noise in (("fixture-a", "(id // 3) % 3"), ("fixture-b", "(id * 5 + 1) % 4")):
        m = f"'s' || least(4, CASE WHEN refund THEN 2 ELSE 0 END + {noise})"
        rot = "list_slice(o || o, 1 + id % 3, 3 + id % 3)"
        fixtures.append(f"""
SELECT '{name}' AS model_id, id, grp, team, refund, mood,
       decide_probability(text, 'A refund is requested.', model := {m}) AS p_refund,
       decide_decision(text, 'A refund is requested.', 0.5, {m}) AS d_refund,
       decide_choice(text, 'Which team owns this?', {rot}, model := 'stub') AS team_hat,
       decide_choice_distribution(text, 'Which team owns this?', {rot}, model := 'stub') AS team_dist,
       decide_score(text, 'How frustrated is the writer?', ['calm', 'annoyed', 'furious'], model := 'stub') AS mood_hat
FROM tickets, (SELECT ['billing', 'defect', 'other'] AS o)""")
    if model:
        fixtures.append(f"""
SELECT '{model}', id, grp, team, refund, mood,
       decide_probability(text, 'A refund is requested.', model := '{model}'),
       decide_decision(text, 'A refund is requested.', 0.5, '{model}'),
       decide_choice(text, 'Which team owns this?', ['billing', 'defect', 'other'], model := '{model}'),
       decide_choice_distribution(text, 'Which team owns this?', ['billing', 'defect', 'other'], model := '{model}'),
       decide_score(text, 'How frustrated is the writer?', ['calm', 'annoyed', 'furious'], model := '{model}')
FROM tickets""")
    answers = "fixture-a" if not model else model
    amodel = "'stub'" if not model else f"'{model}'"
    return f"""{TICKETS}
{STUBS};
CREATE TABLE runs AS {' UNION ALL '.join(fixtures)};
-- decide_answers, unnested: one row per (ticket, question).
CREATE TABLE answers AS
SELECT '{answers}' AS model_id, id, grp, refund, team, mood,
       unnest(decide_answers(text, '{QUESTIONS}', {amodel}), recursive := true)
FROM tickets;
CREATE TABLE platt AS
SELECT model_id, decide_fit_calibration(p_refund, refund) AS spec FROM runs GROUP BY model_id;
"""


# ── bridges: decide output → a contract schema ───────────────────────────────
SUMMARY, CURVE = "summary", "curve"
SPECIFIC = {SUMMARY: "anofox_plot_summary(t)", CURVE: "anofox_plot_curve(t)"}

# The calibration bins and ROC of web/examples/decide-showcase.sql.
CALIBRATION = """SELECT 'calibration' AS curve_type, model_id, avg(p_refund) AS x,
       avg(refund::INTEGER) AS y, count(*) AS weight
FROM runs{where} GROUP BY model_id, least(floor(p_refund * 5), 4)"""
ROC = """WITH th AS (SELECT DISTINCT model_id, p_refund AS t FROM runs
            UNION ALL SELECT DISTINCT model_id, 1.01 FROM runs),
     pos AS (SELECT model_id, count(*) FILTER (WHERE refund) AS p, count(*) FILTER (WHERE NOT refund) AS n
             FROM runs GROUP BY model_id)
SELECT 'roc' AS curve_type, th.model_id, th.t AS threshold,
       count(*) FILTER (WHERE r.p_refund >= th.t AND NOT r.refund) / nullif(any_value(pos.n), 0) AS x,
       count(*) FILTER (WHERE r.p_refund >= th.t AND r.refund) / nullif(any_value(pos.p), 0) AS y
FROM th JOIN runs r USING (model_id) JOIN pos USING (model_id) GROUP BY th.model_id, th.t"""
PR = """WITH th AS (SELECT DISTINCT model_id, p_refund AS t FROM runs)
SELECT 'pr' AS curve_type, th.model_id, th.t AS threshold,
       count(*) FILTER (WHERE r.p_refund >= th.t AND r.refund) / count(*) FILTER (WHERE r.refund) AS x,
       count(*) FILTER (WHERE r.p_refund >= th.t AND r.refund) / nullif(count(*) FILTER (WHERE r.p_refund >= th.t), 0) AS y
FROM th JOIN runs r USING (model_id) GROUP BY th.model_id, th.t"""

# Each decide output, the bridge that makes it a contract table, its schema.
# Keyed by the decide function the bridge exercises.
BRIDGES: dict[str, tuple[str, str]] = {
    # decide_probability + the three metric aggregates → model comparison
    "decide_accuracy": (SUMMARY, """UNPIVOT (SELECT model_id, decide_accuracy(p_refund, refund) AS accuracy,
        decide_brier_score(p_refund, refund) AS brier, decide_ece(p_refund, refund) AS ece
    FROM runs GROUP BY model_id) ON accuracy, brier, ece INTO NAME metric VALUE value"""),
    "decide_brier_score": (SUMMARY, """SELECT model_id, grp AS id, 'brier' AS metric,
        decide_brier_score(p_refund, refund) AS value FROM runs GROUP BY ALL"""),
    "decide_ece": (SUMMARY, """SELECT model_id, 'ece' AS metric, decide_ece(p_refund, refund) AS value
    FROM runs GROUP BY model_id
    UNION ALL SELECT model_id, 'accuracy_at_0.7', decide_accuracy(p_refund, refund, 0.7) FROM runs GROUP BY model_id"""),
    "decide_probability": (CURVE, ROC),
    "decide_probability.pr": (CURVE, PR),
    "decide_probability.calibration": (CURVE, CALIBRATION.format(where="")),
    # decide_decision → precision / recall / F1 of the thresholded answer
    "decide_decision": (SUMMARY, """UNPIVOT (SELECT model_id,
        count(*) FILTER (WHERE d_refund AND refund) / nullif(count(*) FILTER (WHERE d_refund), 0) AS precision,
        count(*) FILTER (WHERE d_refund AND refund) / nullif(count(*) FILTER (WHERE refund), 0) AS recall,
        avg((d_refund = refund)::INTEGER) AS accuracy
    FROM runs GROUP BY model_id) ON precision, recall, accuracy INTO NAME metric VALUE value"""),
    # decide_fit_calibration → the Platt-scaled probabilities next to the raw ones
    "decide_fit_calibration": (CURVE, """WITH c AS (
        SELECT r.*, 1 / (1 + exp(-(split_part(split_part(spec, ':', 2), ',', 1)::DOUBLE
               * ln(greatest(least(p_refund, 0.9999), 0.0001) / (1 - greatest(least(p_refund, 0.9999), 0.0001)))
               + split_part(spec, ',', 2)::DOUBLE))) AS p_cal
        FROM runs r JOIN platt USING (model_id) WHERE model_id = 'fixture-a')
    SELECT 'calibration' AS curve_type, 'raw' AS model_id, avg(p_refund) AS x, avg(refund::INTEGER) AS y,
           count(*) AS weight FROM c GROUP BY least(floor(p_refund * 5), 4)
    UNION ALL
    SELECT 'calibration', 'Platt-calibrated', avg(p_cal), avg(refund::INTEGER), count(*)
    FROM c GROUP BY least(floor(p_cal * 5), 4)"""),
    # decide_choice / decide_choice_distribution / decide_score → per-question quality
    "decide_choice": (SUMMARY, """SELECT model_id, 'accuracy' AS metric, avg((team_hat = team)::INTEGER) AS value
    FROM runs GROUP BY model_id"""),
    "decide_choice_distribution": (SUMMARY, """SELECT model_id, 'log_loss' AS metric,
        avg(-ln(greatest(coalesce(team_dist[team], 0), 1e-6))) AS value FROM runs GROUP BY model_id"""),
    "decide_score": (SUMMARY, """SELECT model_id, 'mae' AS metric, avg(abs(mood_hat - mood)) AS value
    FROM runs GROUP BY model_id"""),
    # decide_answers (unnested) → one "model" per question
    "decide_answers": (SUMMARY, """SELECT question_id AS model_id, 'brier' AS metric,
        avg(power(probability - CASE question_id WHEN 'refund' THEN refund::INTEGER
                                                  WHEN 'team' THEN (choice = team)::INTEGER END, 2)) AS value
    FROM answers WHERE kind IN ('binary', 'choice') GROUP BY question_id"""),
    "decide_table": (SUMMARY, """SELECT question_id AS model_id, 'mean_probability' AS metric, avg(probability) AS value
    FROM tickets, decide_table(text, '""" + QUESTIONS + """', 'stub') GROUP BY question_id"""),
}

# anofox_statistics' curve producers fed decide probabilities (--statistics).
STATISTICS = {
    "roc_curve_by": "roc_curve_by('runs', model_id, refund, p_refund)",
    "pr_curve_by": "pr_curve_by('runs', model_id, refund, p_refund)",
    "calibration_curve_by": "calibration_curve_by('runs', model_id, refund, p_refund, n_bins := 5)",
}

# Raw decide outputs that no contract schema takes: anofox_plot must refuse
# them with the clean error (a gap that starts to render fails: update this).
GAPS = {
    "decide_table": "SELECT anofox_plot(t) FROM decide_table('Please refund me.', '" + QUESTIONS + "', 'stub') t",
    "decide_answers": "SELECT anofox_plot(t) FROM answers t WHERE id = 3",
    "decide_choice_distribution": "SELECT anofox_plot(t) FROM (SELECT id, unnest(map_keys(team_dist)) AS option, "
                                  "unnest(map_values(team_dist)) AS probability FROM runs WHERE id = 3) t",
}
GAP_ERROR = NO_SCHEMA
# …with no "closest" hint: decide's generic columns (kind, probability, …)
# are no near miss of any schema (#180).
GAP_NO_HINT = "; closest:"
# Gaps whose "closest" hint is a known false near miss, tracked elsewhere:
# decide_answers' `team` reads as a misspelling of `term` (#255).
HINT_KNOWN = {"decide_answers": "#255"}

# Extra cases: grouping, options.
EXTRA = {
    "summary_by_group": "SELECT grp, anofox_plot_summary(t) FROM (SELECT grp, model_id, 'brier' AS metric, "
                        "decide_brier_score(p_refund, refund) AS value FROM runs GROUP BY ALL) t GROUP BY grp",
    "summary_metrics": "SELECT anofox_plot_summary(t, metrics := ['brier', 'ece']) FROM ("
                       + BRIDGES["decide_accuracy"][1] + ") t",
    "calibration_group_a": "SELECT anofox_plot_curve(t) FROM (" + CALIBRATION.format(where=" WHERE grp = 'A'") + ") t",
    "pr_base_rate": "SELECT anofox_plot_curve(t, base_rate := (SELECT avg(refund::INTEGER) FROM tickets)) FROM ("
                    + PR + ") t",
    "contract_version": "SELECT CASE WHEN anofox_contract_version() = '1' THEN '<svg>ok</svg>' END AS v",
}

# Build warnings the harness accepts (none today).
ALLOWED_WARNINGS: list[str] = []


def run(duckdb: str, prelude: str, sql: str, mode: str = "-list") -> tuple[bool, str]:
    # anofox_decide prints "[anofox] …" notices (model downloads, banners) on stderr.
    return run_duckdb(duckdb, f"{prelude}\n{sql};", mode, timeout=1800, quiet=("[anofox]",))


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


def check_gap(ok: bool, out: str, name: str = "") -> tuple[str, list[str], str]:
    if ok and svgs_of(out):
        return "XPASS", svgs_of(out), "renders now: move it from GAPS to BRIDGES"
    if not ok and GAP_ERROR in out:
        if GAP_NO_HINT in out and name not in HINT_KNOWN:
            return "FAIL", [], "misleading closest hint (#180): " + out.strip().splitlines()[-1][:200]
        return "GAP", [], out.strip().splitlines()[-1][:200]
    return "FAIL", [], out.strip().replace("\n", " ")[:400]


def main() -> int:
    ap = parser(__doc__, "decide-conformance")
    ap.add_argument("--model", help="a real registered decide model to score next to the fixture "
                                    "(e.g. laya-multilingual)")
    ap.add_argument("--statistics", choices=("auto", "yes", "no"), default="auto",
                    help="feed decide's probabilities to anofox_statistics' curve producers "
                         "(auto: when it loads)")
    ap.add_argument("--extension-dir", help="DuckDB extension_directory holding anofox_decide "
                                            "(default: DuckDB's own)")
    a = ap.parse_args()
    ext = ext_path(a.ext)
    if ext is None:
        return 2
    out_dir = os.path.abspath(a.out)
    os.makedirs(out_dir, exist_ok=True)
    setdir = f"SET extension_directory = '{os.path.abspath(a.extension_dir)}'; " if a.extension_dir else ""
    load = f"{setdir}LOAD anofox_decide; LOAD '{ext}';"

    ok, out = run(a.duckdb, load, "SELECT 'version=' || anofox_contract_version(); "
                  "SELECT 'decide=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_decide'; "
                  "SELECT 'visualization=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization'; "
                  "SELECT 'duckdb=' || library_version FROM pragma_version()")
    print(out.strip())
    if not ok or "version=1" not in out:
        print("FAIL: anofox_contract_version() must be '1'")
        return 1

    # Tagged producers (none today). The anofox_decide_* aliases are skipped.
    ok, out = run(a.duckdb, load, "SELECT DISTINCT function_name || '|' || function_type || '|' || tags['anofox.output'] "
                  "FROM duckdb_functions() WHERE tags['anofox.output'] IS NOT NULL "
                  "AND function_name LIKE 'decide\\_%' ESCAPE '\\' ORDER BY 1")
    if not ok:
        print(f"FAIL: discovery: {out.strip()}")
        return 1
    producers = sorted({tuple(l.split("|")) for l in out.splitlines() if l.count("|") == 2})
    print(f"{len(producers)} tagged contract producers in anofox_decide")
    missing = [p[0] for p in producers]  # none has a recipe yet: a tagged one needs one

    stats = a.statistics != "no"
    if stats:
        ok, _ = run(a.duckdb, f"{setdir}LOAD anofox_statistics;", "SELECT 1")
        if not ok and a.statistics == "yes":
            print("FAIL: --statistics yes but anofox_statistics does not load")
            return 1
        stats = ok
    print(f"anofox_statistics bridges: {'on' if stats else 'off'}")

    # Score the tickets once into a database file every case reads.
    db = os.path.join(out_dir, "fixture.duckdb")
    for f in (db, db + ".wal"):
        if os.path.exists(f):
            os.remove(f)
    ok, out = run(a.duckdb, f"ATTACH '{db}' AS fx; USE fx; {load}", fixture_sql(a.model))
    if not ok:
        print(f"FAIL: fixture: {out.strip()[:800]}")
        return 1
    prelude = (f"{setdir}{'LOAD anofox_statistics; ' if stats else ''}{load} "
               # not USE fx: the C-API build's plot macros live in the default
               # database (BUILD.md, "macro caveat"), so search both.
               f"ATTACH '{db}' AS fx (READ_ONLY); SET search_path = 'fx.main,memory.main';")

    runs = []
    for name, (schema, src) in BRIDGES.items():
        for which, call in (("auto", "anofox_plot(t)"), ("specific", SPECIFIC[schema])):
            runs.append({"name": name, "which": which, "kind": "case",
                         "sql": f"SELECT {call} FROM ({src}) t"})
    if stats:
        for name, src in STATISTICS.items():
            for which, call in (("auto", "anofox_plot(t)"), ("specific", SPECIFIC[CURVE])):
                runs.append({"name": f"statistics.{name}", "which": which, "kind": "case",
                             "sql": f"SELECT {call} FROM {src} t"})
    for name, sql in GAPS.items():
        runs.append({"name": f"gap.{name}", "which": "auto", "kind": "gap", "sql": sql, "gap": name})
    for name, sql in EXTRA.items():
        runs.append({"name": name, "which": "extra", "kind": "case", "sql": sql})
    if a.only:
        runs = [c for c in runs if re.search(a.only, c["name"] + "." + c["which"])]

    def one(c):
        ok, out = run(a.duckdb, prelude, c["sql"], "-json")
        return c, (check_gap(ok, out, c["gap"]) if c["kind"] == "gap" else check(ok, out))

    results = run_all(runs, one, a.jobs, out_dir)
    names = svg_names(results)
    for key, status, msg, n, c in results:
        print(f"{status:5s} {key:44s} {msg}")
    for m in missing:
        print(f"MISS  {m}: tagged producer without a recipe in the harness")
    good = {"OK", "GAP"}
    passed = sum(1 for r in results if r[1] in good)
    gaps = sum(1 for r in results if r[1] == "GAP")
    print(f"\n{len(BRIDGES)} decide bridges, {len(producers)} tagged producers, "
          f"{passed}/{len(results)} calls as expected ({gaps} known gaps)")
    keep = lambda n: not n.startswith("contract_version") and not n.endswith(".auto")
    if not a.no_montage:
        write_montage(out_dir, names, keep, cols=5)
    return 0 if passed == len(results) and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
