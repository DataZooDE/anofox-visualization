#!/usr/bin/env python3
"""#1117: which roles does a chart accept without drawing anything and without
a word? For each base chart and role: render with and without the role (a
plausible value); an identical SVG and no new diagnostic = silent.

  python3 scripts/role-sweep.py target/release/dashboard <out-dir>
"""
import json, os, re, subprocess, sys, hashlib
D = sys.argv[1]  # the dashboard binary
OUT = sys.argv[2]
os.makedirs(OUT, exist_ok=True)
md = subprocess.run([D, "--roles-md"], capture_output=True, text=True).stdout
roles = []
for l in md.splitlines():
    if l.startswith("| `::"):
        c = [x.strip() for x in l.strip().strip("|").split("|")]
        roles.append((c[0].strip("`").lstrip(":"), c[2], c[3]))

DATA = ("(VALUES ('A', DATE '2024-01-01', 10.0, 3.0, 'x'), ('B', DATE '2024-02-01', 30.0, 5.0, 'y'), "
        "('C', DATE '2024-03-01', 20.0, 1.0, 'x'), ('D', DATE '2024-04-01', 40.0, 4.0, 'y')) t(k, d, v, w, g)")
BASES = {
    "bar": "k::XAXIS, v::BARCHART AS \"Value\"",
    "line": "d::XAXIS, v::LINECHART AS \"Value\"",
    "scatter": "w::XAXIS, v::SCATTER AS \"Value\"",
    "histogram": "v::HISTOGRAM AS \"Value\"",
    "kpi": "sum(v)::METRIC AS \"Total\"",
    "table": "k, v::METRIC AS \"Value\", w, 'x'::TABLE",
}
# a plausible value per role (by name), else by category
VAL = {
    "XFORMAT": "'%b %Y'", "YFORMAT": "'€'", "Y2FORMAT": "'percent'", "XSCALE": "'log10'", "YSCALE": "'log10'",
    "XBREAKS": "'1,2,3'", "YBREAKS": "'10,20,30'", "SMOOTH_METHOD": "'lm'", "ALPHA": "0.5", "LEGEND": "'none'",
    "XTITLE": "'X title'", "YTITLE": "'Y title'", "Y2": "'Value'", "COLORS": "'#d64545,#2563eb'",
    "RANGE": "'0,50'", "LABELS": "'low,high'", "HIST_BINS": "'3'", "HIST_NORM": "'share'", "FLIP": "1",
    "DATALABELS": "''", "LABEL_TOP": "2", "XLINE": "v", "REFLINE": "25", "XLINE_LABEL": "'event'",
    "REFLINE_LABEL": "'target'", "THRESHOLDS": "'<20:bad; good'", "TITLE": "'T'", "SUBTITLE": "'S'",
    "LABEL": "'L'", "CATEGORY": "g", "SIZE": "w", "TOOLTIP": "w", "GOAL": "50", "DELTA": "5",
    "SPARKLINE": "[1,2,3]", "LINESTYLE": "'dashed'", "POINTS": "1", "MIDPOINT": "15", "FACET": "g",
    "FACET_FREE": "g", "FACET_NCOL": "2", "ANNOTATE": "'note'", "TEXT_SMALL": "'t'", "YMIN": "v-2",
    "YMAX": "v+2", "BAND_LOWER": "v-3", "BAND_UPPER": "v+3", "XMIN": "w-1", "XMAX": "w+1",
    "RANK": "w", "INFO": "'info'", "NOTE": "'note'", "CAPTION": "'cap'", "SOURCE": "'src'",
}
BY_CAT = {"modifier": "'x'", "annotation": "1", "encoding": "g", "kpi": "1", "table": "'x'"}
SKIP_CAT = {"input", "layout", "chrome", "chart"}  # statements / panels of their own, or the mark itself


def render(sql, name):
    p = os.path.join(OUT, name + ".sql")
    open(p, "w").write(sql + ";\n")
    chk = subprocess.run([D, "--check", p, "--json"], capture_output=True, text=True).stdout
    try:
        diags = {(d["code"], d["severity"]) for d in json.loads(chk)["diagnostics"]}
    except Exception:
        diags = {("check-failed", "error")}
    svg = subprocess.run([D, "--svg", p, os.path.join(OUT, name + ".svg"), "--width", "600"],
                         capture_output=True, text=True)
    s = open(os.path.join(OUT, name + ".svg")).read() if os.path.exists(os.path.join(OUT, name + ".svg")) else ""
    s = re.sub(r'id="[^"]*"', "", s)  # generated ids
    return hashlib.md5(s.encode()).hexdigest(), diags


rows = []
base_cache = {}
for b, sel in BASES.items():
    group = " GROUP BY ALL" if b == "kpi" else ""
    base_sql = f"SELECT {sel} FROM {DATA}{group}"
    base_cache[b] = render(base_sql, f"base_{b}")
    for tok, cat, meaning in roles:
        if cat in SKIP_CAT:
            continue
        v = VAL.get(tok, BY_CAT.get(cat, "'x'"))
        sql = f"SELECT {sel}, {v}::{tok} FROM {DATA}{group}"
        h, d = render(sql, f"{b}_{tok}")
        bh, bd = base_cache[b]
        new = sorted(x for x in d - bd if x[1] in ("error", "warning", "note"))
        rows.append({"base": b, "role": tok, "cat": cat, "same_svg": h == bh, "new_diags": new})
json.dump(rows, open(os.path.join(OUT, "sweep.json"), "w"), indent=1)
silent = [r for r in rows if r["same_svg"] and not r["new_diags"]]
print(len(rows), "combinations;", len(silent), "silent (same SVG, no new diagnostic)")
by_role = {}
for r in silent:
    by_role.setdefault(r["role"], []).append(r["base"])
for k in sorted(by_role, key=lambda k: -len(by_role[k])):
    print(f"  {k:22s} {next(c for t,c,_ in roles if t==k):10s} silent on: {', '.join(by_role[k])}")
