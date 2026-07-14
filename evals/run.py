#!/usr/bin/env python3
"""Score dashboard candidates: correctness (dashboard --check) + rubric heuristics.

Usage:  python3 evals/run.py [dir]     (default: evals/cases)
        DASHBOARD=/path/to/dashboard python3 evals/run.py

Drop generated dashboards into a directory and run this to measure whether a
prompt/skill change improved results. Exits non-zero if any case has lint errors.
"""
import json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
DASHBOARD = os.environ.get("DASHBOARD") or os.path.join(HERE, "..", "target", "debug", "dashboard")
CASES = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "cases")

CHART = r"::(BARCHART\w*|LINECHART\w*|AREACHART|STEP|SMOOTH|SCATTER|BUBBLE|PIE|DONUTCHART|GAUGE|RADAR|HISTOGRAM|DENSITY|BOXPLOT|VIOLIN|HEATMAP|CALENDAR|CANDLESTICK|OHLC|QQ|SPARKLINE|MAP)\b"
KPI   = r"::(METRIC|MONEY|PERCENT|COMPACT)\b"
INPUT = r"(::(DROPDOWN|MULTISELECT|DATE|DATERANGE|NUMBER|TEXT)\b|getvariable\()"

def lint(path):
    out = subprocess.run([DASHBOARD, "--check", path, "--json"], capture_output=True, text=True)
    try:
        d = json.loads(out.stdout)
    except json.JSONDecodeError:
        return 99, 0, out.stderr.strip()[:80]
    diags = d.get("diagnostics", [])
    errs = sum(1 for x in diags if x["severity"] == "error")
    warns = len(diags) - errs
    return errs, warns, ""

def score(path):
    sql = open(path).read()
    errs, warns, note = lint(path)
    charts = len(re.findall(CHART, sql))
    titles = len(re.findall(r"::TITLE\b", sql))
    kpis   = len(re.findall(KPI, sql))
    has_input = bool(re.search(INPUT, sql))
    panels = charts + kpis + len(re.findall(r"::(TABLE|PAGED)\b", sql))

    if errs:
        return 0, errs, warns, f"LINT ERRORS ({note})" if note else "LINT ERRORS"
    s, notes = 100, []
    s -= 5 * warns
    if warns: notes.append(f"{warns} warn")
    if kpis == 0: s -= 15; notes.append("no KPIs")
    if not has_input: s -= 10; notes.append("no filter")
    if charts and titles < charts:
        miss = charts - titles; s -= min(25, 8 * miss); notes.append(f"{miss} untitled chart(s)")
    if panels < 3: s -= 15; notes.append("too few panels")
    if panels > 14: s -= 10; notes.append("too many panels")
    return max(0, s), errs, warns, ", ".join(notes) or "ok"

def main():
    files = sorted(f for f in os.listdir(CASES) if f.endswith(".sql"))
    if not files:
        print(f"no .sql cases in {CASES}"); return 2
    total, any_err = 0, False
    print(f"{'case':<26} {'score':>5}  notes")
    print("-" * 60)
    for f in files:
        sc, errs, warns, notes = score(os.path.join(CASES, f))
        any_err = any_err or errs > 0
        total += sc
        print(f"{f:<26} {sc:>5}  {notes}")
    print("-" * 60)
    print(f"{'mean':<26} {total // len(files):>5}  ({len(files)} cases)")
    return 1 if any_err else 0

sys.exit(main())
