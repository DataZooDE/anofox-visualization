#!/usr/bin/env python3
"""Conformance harness: every anofox-optimize producer → our plots.

anofox_optimize tags no function with `anofox.contract` (its solvers are
scalar functions returning a STRUCT of parallel lists, not contract tables),
so the producers are every `anofox_optimize_*` solver in `duckdb_functions()`
— 38 in 8 families — and the harness carries a recipe per family. A solver
without a recipe fails the run, so upstream additions can't slip through.

Each producer is called on a small deterministic instance and checked three
ways:

* `check`: the return type is the family's documented STRUCT and the
  solution is consistent (bins and waves within capacity, the objective
  recomputed from the returned order / selection, the index bases);
* `auto` / `summary`: its objectives as the contract `summary` schema
  (`model_id` = the algorithm, `metric`, `value`) through `anofox_plot(...)`
  and `anofox_plot_summary(...)`;
* `view`: the family's decision view — a real Gantt for `schedule`
  (`anofox_plot_schedule`, #176), `anofox_render` with the roles the other
  optimize examples use for everything else (stacked bins and waves, the
  route on a `::MAP`, selections and weights as bars).

Every plot must return an SVG with no SQL error and no unexpected render
warning. Every SVG is written to the output directory, plus a PNG montage
when `rsvg-convert` and Pillow exist.

    python3 scripts/optimize_conformance.py \
        --duckdb ./duckdb --ext ./anofox_visualization.duckdb_extension \
        --out /tmp/optimize-viz

The DuckDB CLI must be able to `LOAD anofox_optimize` (install it once with
`INSTALL anofox_optimize FROM 'https://get.erpl.io'` in `duckdb -unsigned`).
anofox_optimize's usage telemetry is switched off for every run
(DATAZOO_DISABLE_TELEMETRY=1). Exit status 1 when any case fails or a
producer is not covered.

The runner, the SVG check, the montage and the arguments come from
`scripts/conformance_lib.py`.
"""

from __future__ import annotations

import os
import re
import sys

from conformance_lib import check as _check, ext_path, parser, run_all, run_duckdb, svg_names, write_montage

# ── demo instances ───────────────────────────────────────────────────────────
# Small and deterministic: every solver (exact ones included) finishes in
# milliseconds, so no time limits are needed.
SETUP = """
-- packing / waves: 12 cartons, 10 units per container or wave
CREATE TABLE items AS
SELECT i AS item, (2 + (i * 5) % 7)::DOUBLE AS size, (1 + (i * 3) % 4)::DOUBLE AS priority
FROM range(1, 13) t(i);
-- knapsack: 10 shipments, 20 kg on the cheap carrier
CREATE TABLE ship AS
SELECT i AS item, (3 + (i * 7) % 11)::DOUBLE AS value, (2 + (i * 5) % 6)::DOUBLE AS weight
FROM range(1, 11) t(i);
-- sequencing: 7 stops on a small map (lon/lat), setup = straight-line distance
CREATE TABLE stops AS
SELECT i AS node, 13.30 + 0.03 * ((i * 3) % 7) AS lon, 52.45 + 0.02 * ((i * 5) % 7) AS lat
FROM range(0, 7) t(i);
-- scheduling: 6 jobs, sequence-dependent setups; matrix row/col 0 = the virtual start
CREATE TABLE jobs AS
SELECT i AS job, (4 + (i * 3) % 5)::DOUBLE AS p, (8 + 6 * i)::DOUBLE AS due, (1 + i % 3)::DOUBLE AS w
FROM range(0, 6) t(i);
-- assortment: 6 products, at most 3 on the shelf
CREATE TABLE prods AS
SELECT i AS product, (2 + (i * 3) % 5)::DOUBLE AS margin, (60 + 15 * ((i * 2) % 5))::DOUBLE AS demand
FROM range(0, 6) t(i);
-- portfolio: 4 assets, at most 3 holdings, 50 % per position
CREATE TABLE assets AS
SELECT i AS asset, [0.12, 0.09, 0.07, 0.05][i + 1] AS ret, [0.20, 0.15, 0.10, 0.06][i + 1] AS vol
FROM range(0, 4) t(i);

-- anofox_optimize's index base: 0 before v2026.10.26 (bins and orders 0-based, UBIGINT), 1 from then on
-- (every index a 1-based BIGINT, the SQL convention, anofox-optimize#7). The harness accepts both.
CREATE MACRO ob() AS (SELECT list_min(anofox_optimize_pack_next_fit([1.0], 2.0).assignment)::BIGINT);
CREATE MACRO setup_seq() AS (
    SELECT list(round(sqrt((a.lon - b.lon) ^ 2 + (a.lat - b.lat) ^ 2) * 100, 3) ORDER BY a.node, b.node)
    FROM stops a, stops b);
CREATE MACRO setup_sched() AS (
    SELECT list(CASE WHEN b = 0 THEN 0.0 WHEN a = 0 THEN 1.0
                     WHEN a = b THEN 0.0 ELSE (1 + (a * 7 + b * 3) % 4)::DOUBLE END ORDER BY a, b)
    FROM range(0, 7) x(a), range(0, 7) y(b));
CREATE MACRO subst() AS (
    SELECT list(CASE WHEN a = b THEN 0.0 WHEN a // 2 = b // 2 THEN 0.5 ELSE 0.05 END ORDER BY a, b)
    FROM range(0, 6) x(a), range(0, 6) y(b));
CREATE MACRO cov() AS (
    SELECT list(CASE WHEN a.asset = b.asset THEN a.vol * a.vol ELSE 0.3 * a.vol * b.vol END
                ORDER BY a.asset, b.asset)
    FROM assets a, assets b);
"""

# family → (arguments, documented return type, objective fields)
FAMILIES = {
    "pack": ("(SELECT list(size ORDER BY item) FROM items), 10.0",
             'STRUCT(bins_used UBIGINT, "assignment" UBIGINT[])', ["bins_used"]),
    "knapsack": ("(SELECT list(value ORDER BY item) FROM ship), (SELECT list(weight ORDER BY item) FROM ship), 20.0",
                 "STRUCT(selected BOOLEAN[], total_value DOUBLE, total_weight DOUBLE)",
                 ["total_value", "total_weight"]),
    "wave": ("(SELECT list(size ORDER BY item) FROM items), (SELECT list(priority ORDER BY item) FROM items), 10.0",
             "STRUCT(wave UBIGINT[], avg_weighted_wave DOUBLE)", ["avg_weighted_wave"]),
    "sequence": ("setup_seq(), 7",
                 'STRUCT("order" UBIGINT[], total_setup DOUBLE)', ["total_setup"]),
    "schedule": ("(SELECT list(p ORDER BY job) FROM jobs), (SELECT list(due ORDER BY job) FROM jobs), "
                 "(SELECT list(w ORDER BY job) FROM jobs), setup_sched(), 6",
                 'STRUCT("order" UBIGINT[], total_weighted_tardiness DOUBLE, makespan DOUBLE)',
                 ["total_weighted_tardiness", "makespan"]),
    "assortment_recapture": ("(SELECT list(margin ORDER BY product) FROM prods), "
                             "(SELECT list(demand ORDER BY product) FROM prods), subst(), 3",
                             "STRUCT(listed BOOLEAN[], captured_margin DOUBLE)", ["captured_margin"]),
    "assortment": ("(SELECT list(margin ORDER BY product) FROM prods), "
                   "(SELECT list(demand ORDER BY product) FROM prods), subst(), 3",
                   "STRUCT(listed BOOLEAN[], captured_margin DOUBLE)", ["captured_margin"]),
    "portfolio": ("(SELECT list(ret ORDER BY asset) FROM assets), cov(), 3, 0.5",
                  "STRUCT(weights DOUBLE[], expected_return DOUBLE, volatility DOUBLE, sharpe DOUBLE)",
                  ["expected_return", "volatility", "sharpe"]),
}
# Not solvers: a matrix helper and the build id.
HELPERS = {"anofox_optimize_matrix_from_triples", "anofox_optimize_version"}

# Consistency of a solution `s` (one row, column `s`): an error() names the
# first broken invariant. These also pin the index bases: waves are 1-based; bins and orders start at ob()
# (0 before anofox_optimize v2026.10.26, 1 from then on).
INVARIANTS = {
    "pack": [
        ("bins numbered ob() .. bins_used - 1 + ob()",
         "s.bins_used = list_max(s.assignment) + 1 - ob() AND list_min(s.assignment) = ob()"),
        ("every bin within capacity",
         "(SELECT max(t) FROM (SELECT sum(i.size) t FROM items i GROUP BY s.assignment[i.item])) <= 10"),
    ],
    "knapsack": [
        ("total_weight = Σ selected weights ≤ capacity",
         "abs(s.total_weight - (SELECT sum(weight) FROM ship WHERE s.selected[item])) < 1e-9 AND s.total_weight <= 20"),
        ("total_value = Σ selected values",
         "abs(s.total_value - (SELECT sum(value) FROM ship WHERE s.selected[item])) < 1e-9"),
    ],
    "wave": [
        ("waves are 1-based and contiguous",
         "list_min(s.wave) = 1 AND len(list_distinct(s.wave)) = list_max(s.wave)"),
        ("every wave within capacity",
         "(SELECT max(t) FROM (SELECT sum(i.size) t FROM items i GROUP BY s.wave[i.item])) <= 10"),
    ],
    "sequence": [
        ("order is a permutation of 0..6",
         "list_sort(s.\"order\")::BIGINT[] = range(ob(), 7 + ob())"),
        ("total_setup = Σ consecutive setups (open path)",
         "abs(s.total_setup - (SELECT sum(setup_seq()[((s.\"order\"[k]::BIGINT - ob()) * 7 + s.\"order\"[k + 1]::BIGINT - ob() + 1)]) "
         "FROM range(1, 7) r(k))) < 1e-6"),
    ],
    "schedule": [
        ("order is a permutation of the 6 jobs (from ob())",
         "list_sort(s.\"order\")::BIGINT[] = range(ob(), 6 + ob())"),
        ("makespan = Σ setups + processing along the order",
         "abs(s.makespan - (SELECT max(e) FROM gantt)) < 1e-6"),
        ("total_weighted_tardiness recomputed from the Gantt",
         "abs(s.total_weighted_tardiness - (SELECT sum(w * greatest(e - due, 0)) FROM gantt)) < 1e-6"),
    ],
    "assortment": [
        ("at most 3 listed", "len(list_filter(s.listed, x -> x)) BETWEEN 1 AND 3"),
    ],
    "portfolio": [
        ("weights sum to 1", "abs(list_sum(s.weights) - 1) < 1e-6"),
        ("at most 3 holdings, each ≤ 50 %",
         "len(list_filter(s.weights, x -> x > 1e-9)) <= 3 AND list_max(s.weights) <= 0.5 + 1e-9"),
        ("expected_return = Σ weight · return",
         "abs(s.expected_return - list_dot_product(s.weights, (SELECT list(ret ORDER BY asset) FROM assets))) < 1e-6"),
    ],
}
INVARIANTS["assortment_recapture"] = INVARIANTS["assortment"]

# The schedule as rows: start/end of each job along the returned order.
GANTT = """
CREATE TABLE gantt AS
WITH o AS (SELECT k AS pos, s."order"[k]::BIGINT - ob() AS job,
                  CASE WHEN k = 1 THEN 0 ELSE s."order"[k - 1]::BIGINT - ob() + 1 END AS prev
           FROM sol, range(1, 7) r(k)),
c AS (SELECT o.pos, o.job, j.p, j.due, j.w,
             setup_sched()[o.prev * 7 + o.job + 2] AS setup FROM o JOIN jobs j USING (job))
SELECT pos, job, due, w,
       sum(setup + p) OVER (ORDER BY pos) - p AS b,
       sum(setup + p) OVER (ORDER BY pos) AS e
FROM c;
"""


def family_of(name: str) -> tuple[str, str] | None:
    rest = name.removeprefix("anofox_optimize_")
    for fam in FAMILIES:  # assortment_recapture before assortment
        if rest.startswith(fam + "_"):
            return fam, rest[len(fam) + 1:]
    return None


def role_svg(cols: list[tuple[str, str]], src: str, order: str, w: int = 640, h: int = 400) -> str:
    """anofox_render over `src` with one role per column (expr, ROLE)."""
    struct = ", ".join(f"c{i}: {e}" for i, (e, _) in enumerate(cols))
    roles = ", ".join(f"json_array({i}, '{r}')" for i, (_, r) in enumerate(cols))
    return (f"SELECT anofox_render(json_object('rows', to_json(list({{{struct}}} ORDER BY {order})), "
            f"'roles', json_array({roles}), 'width', {w}, 'height', {h})) FROM ({src}) v")


def view_sql(fam: str) -> str:
    """The family's decision view of the solution in table `sol`."""
    if fam == "pack":
        return role_svg([("'container ' || (s.assignment[item] + 1 - ob())", "XAXIS"), ("size", "BARCHART_STACKED"),
                         ("'carton ' || lpad(item::VARCHAR, 2, '0')", "CATEGORY"), ("10", "REFLINE")],
                        "SELECT * FROM sol, items", "s.assignment[item], item")
    if fam == "wave":
        return role_svg([("'wave ' || s.wave[item]", "XAXIS"), ("size", "BARCHART_STACKED"),
                         ("'p' || priority::INTEGER", "CATEGORY"), ("10", "REFLINE")],
                        "SELECT * FROM sol, items", "s.wave[item], item")
    if fam == "knapsack":
        return role_svg([("'shipment ' || lpad(item::VARCHAR, 2, '0')", "XAXIS"), ("value", "BARCHART"),
                         ("CASE WHEN s.selected[item] THEN 'on the carrier' ELSE 'left out' END", "CATEGORY")],
                        "SELECT * FROM sol, ship", "item")
    if fam.startswith("assortment"):
        return role_svg([("'product ' || product", "XAXIS"), ("margin * demand", "BARCHART"),
                         ("CASE WHEN s.listed[product + 1] THEN 'listed' ELSE 'not listed' END", "CATEGORY")],
                        "SELECT * FROM sol, prods", "product")
    if fam == "portfolio":
        return role_svg([("'asset ' || asset", "XAXIS"), ("round(s.weights[asset + 1], 4)", "BARCHART"),
                         ("0.5", "REFLINE")],
                        "SELECT * FROM sol, assets", "asset")
    if fam == "sequence":
        # the route through the returned order, and the stops
        src = ("SELECT 0 AS k, 'LINESTRING(' || string_agg(lon || ' ' || lat, ', ' ORDER BY pos) || ')' AS wkt, "
               "'route' AS series FROM (SELECT k AS pos, s.\"order\"[k]::BIGINT - ob() AS node FROM sol, range(1, 8) r(k)) "
               "JOIN stops USING (node) "
               "UNION ALL SELECT 1 + node, 'POINT(' || lon || ' ' || lat || ')', 'stop' FROM stops")
        return role_svg([("wkt", "MAP"), ("series", "CATEGORY")], src, "k")
    if fam == "schedule":
        # a real Gantt (#176): one resource (the single machine), a bar per
        # job coloured by status, a dot at its due date — was SCATTER +
        # XMIN/XMAX "error bars" (#171).
        return GANTT + ("; SELECT anofox_plot_schedule(g) FROM ("
                        "SELECT 'line' AS resource, 'job ' || job AS task, b AS start, e AS \"end\", "
                        "due, CASE WHEN e > due THEN 'late' ELSE 'on time' END AS status FROM gantt"
                        ") g")
    raise KeyError(fam)


def summary_rows(fam: str, algo: str) -> str:
    """The solution's objectives as summary-schema rows (model_id, metric, value)."""
    fields = FAMILIES[fam][2]
    metrics = ", ".join(f"'{f}'" for f in fields)
    values = ", ".join(f"s.{f}::DOUBLE" for f in fields)
    return (f"SELECT '{algo}' AS model_id, unnest([{metrics}]) AS metric, unnest([{values}]) AS value FROM sol")


def check_sql(name: str, fam: str) -> str:
    """Assert the return type and the invariants; renders an 'ok' SVG."""
    want = FAMILIES[fam][1]
    # The index lists are UBIGINT before anofox_optimize v2026.10.26 and BIGINT from then on.
    conds = [(f"returns {want}", f"replace(typeof(s), 'UBIGINT', 'BIGINT') = replace('{want}', 'UBIGINT', 'BIGINT')")] \
        + INVARIANTS[fam]
    case = " ".join(f"WHEN NOT ({c}) THEN error('{name}: " + m.replace("'", "''") + "')"
                    for m, c in conds)
    pre = GANTT if fam == "schedule" else ""
    return f"{pre}SELECT CASE {case} ELSE '<svg>ok</svg>' END AS v FROM sol"


def cases(producers: list[str]) -> tuple[list[dict], list[str]]:
    out, missing = [], []
    for name in producers:
        fa = family_of(name)
        if fa is None:
            missing.append(name)
            continue
        fam, algo = fa
        sol = f"CREATE TABLE sol AS SELECT {name}({FAMILIES[fam][0]}) AS s;"
        for which, sql in (
            ("check", check_sql(name, fam)),
            ("auto", f"SELECT anofox_plot(t) FROM ({summary_rows(fam, algo)}) t"),
            ("summary", f"SELECT anofox_plot_summary(t) FROM ({summary_rows(fam, algo)}) t"),
            ("view", view_sql(fam)),
        ):
            out.append({"name": name.removeprefix("anofox_optimize_"), "which": which,
                        "family": fam, "sql": sol + sql})
    return out, missing


def family_cases(producers: list[str]) -> list[dict]:
    """Per family: every algorithm's objectives side by side (anofox_plot_summary)."""
    by: dict[str, list[tuple[str, str]]] = {}
    for name in producers:
        fa = family_of(name)
        if fa:
            by.setdefault(fa[0], []).append((name, fa[1]))
    out = []
    for fam, members in sorted(by.items()):
        fields = FAMILIES[fam][2]
        metrics = ", ".join(f"'{f}'" for f in fields)
        values = ", ".join(f"s.{f}::DOUBLE" for f in fields)
        parts = " UNION ALL ".join(
            f"SELECT '{algo}' AS model_id, {n}({FAMILIES[fam][0]}) AS s" for n, algo in members)
        out.append({"name": f"{fam}_compare", "which": "extra", "family": fam,
                    "sql": f"SELECT anofox_plot_summary(t) FROM (SELECT model_id, unnest([{metrics}]) AS metric, "
                           f"unnest([{values}]) AS value FROM ({parts})) t"})
    return out


# Build warnings the harness accepts (none today).
ALLOWED_WARNINGS: list[str] = []


def run(duckdb: str, ext: str, sql: str, mode: str = "-list") -> tuple[bool, str]:
    return run_duckdb(duckdb, f"LOAD anofox_optimize; LOAD '{ext}';\n{SETUP}\n{sql};", mode, timeout=120)


def check(ok: bool, out: str) -> tuple[str, list[str], str]:
    return _check(ok, out, ALLOWED_WARNINGS)


def main() -> int:
    a = parser(__doc__, "optimize-conformance").parse_args()
    ext = ext_path(a.ext)
    if ext is None:
        return 2
    os.makedirs(a.out, exist_ok=True)

    ok, out = run(a.duckdb, ext, "SELECT 'optimize=' || anofox_optimize_version(); "
                  "SELECT 'visualization=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization'; "
                  "SELECT 'duckdb=' || library_version FROM pragma_version()")
    print(out.strip())
    if not ok:
        print("FAIL: could not LOAD anofox_optimize and the visualization extension")
        return 1

    # Every solver (the opt_* aliases share the function; helpers excluded).
    ok, out = run(a.duckdb, ext, "SELECT DISTINCT function_name || '|' || "
                  "coalesce(tags['anofox.contract'], '') FROM duckdb_functions() "
                  "WHERE function_name LIKE 'anofox\\_optimize\\_%' ESCAPE '\\' AND function_type = 'scalar' ORDER BY 1")
    rows = [l.split("|") for l in out.splitlines() if "|" in l]
    producers = sorted(n for n, _ in rows if n not in HELPERS)
    tagged = [n for n, t in rows if t == "1"]
    print(f"{len(producers)} producers ({len(tagged)} tagged anofox.contract = '1')")
    todo, missing = cases(producers)
    runs = todo + family_cases(producers)
    if a.only:
        runs = [c for c in runs if re.search(a.only, c["name"] + "." + c["which"])]

    def one(c):
        ok, out = run(a.duckdb, ext, c["sql"], "-json")
        return c, check(ok, out)

    results = run_all(runs, one, a.jobs, a.out)
    names = svg_names(results)
    for key, status, msg, n, c in results:
        print(f"{status:4s} {key:48s} {msg}")
    for m in missing:
        print(f"MISS {m}: no family recipe in the harness")
    passed = sum(1 for r in results if r[1] == "OK")
    covered = len({c["name"] for c in todo})
    print(f"\n{covered}/{len(producers)} producers covered, {passed}/{len(results)} cases OK")
    keep = lambda n: not n.endswith((".check", ".auto"))
    if not a.no_montage:
        write_montage(a.out, names, keep)
    return 0 if passed == len(results) and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
