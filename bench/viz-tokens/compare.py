"""Compare a finished, judged run with an earlier one, and keep a history (#1074).

    python compare.py <run> [--baseline <run>] [--history history.csv] [--set smoke]

Writes <run>/compare.md: pass rate per arm and phase, per family, per task,
then the regressions — a task that passed at least 2 of its repeats in the
baseline and at most 1 now, a phase whose pass rate fell by more than
`--drop` points, or a phase worse beyond noise (#1122: the 95 % CI of the
difference, paired by task, below 0; see stats.py). Appends one row per arm to the history CSV. Exit status 2
on a regression (so a scheduled run can open an issue), 0 otherwise.

Pass rate comes first: correctness is what the bench is for (#1044); tokens
and cost are reported after it.
"""
import argparse
import csv
import json
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from stats import diff_ci, fmt_diff, fmt_rate, rate_ci, verdict  # noqa: E402

PHASES = ("first", "followup")


def families():
    fam = {}
    for f in ("tasks.jsonl", "coverage.jsonl"):
        for t in map(json.loads, (HERE / f).read_text().splitlines()):
            fam[t["id"]] = t.get("family", "comparison")
    return fam


def load(run):
    """{(task, arm, phase): [pass, …]} from a run's judgments, plus its summary."""
    run = Path(run)
    out = defaultdict(list)
    path = run / "judgments.jsonl"
    if path.exists():
        for j in map(json.loads, path.read_text().splitlines()):
            task, arm, _rep = j["run"].split("__")
            out[(task, arm, j["phase"])].append(bool(j.get("pass")))
    summary = json.loads((run / "summary.json").read_text()) if (run / "summary.json").exists() else {}
    return out, summary


def rate(cells):
    n = sum(len(v) for v in cells)
    return (sum(sum(v) for v in cells), n)


def pct(p, n):
    return f"{p}/{n} ({100 * p / n:.0f} %)" if n else "–"


def git_sha(path):
    try:
        return subprocess.run(["git", "-C", str(path), "rev-parse", "--short", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ""


def compare(run, base, drop):
    """(markdown lines, regressions)."""
    fam = families()
    now, summary = load(run)
    was, _ = load(base) if base else ({}, {})
    arms = sorted({a for (_, a, _) in now})
    L = [f"# Compare: {Path(run).name}" + (f" vs {Path(base).name}" if base else " (no baseline)"), ""]
    regressions = []

    L += ["## Pass rate", "",
          "95 % CIs (#1122): **boot** = bootstrap over tasks, a task's repeats kept together (the one to read); "
          "**Wilson** = every run counted as independent (for reference; too narrow when repeats agree). "
          "**Δ** = now − baseline on the tasks both ran, each task's pass rate over its repeats, "
          "averaged over tasks (each task weighs the same), with a bootstrap CI over those tasks. "
          "**better / worse** only when the whole CI is above / below 0, else **within noise**. "
          "Claims of progress need `--reps 3`.", "",
          "| arm | phase | now | baseline (same tasks) | Δ now − baseline (95 % CI) | verdict |",
          "|---|---|---|---|---|---|"]
    verdicts = []
    for arm in arms:
        for ph in PHASES:
            cur = {t: v for (t, a, q), v in now.items() if a == arm and q == ph}
            # the baseline on the same tasks only, so a smoke run compares like with like
            old = {t: v for (t, a, q), v in was.items() if a == arm and q == ph and t in cur}
            rc, rb = rate_ci(cur), rate_ci(old)
            if old:
                d = diff_ci(cur, old)
                v = verdict(d)
                verdicts.append(f"{arm} {ph}: **{v}** ({fmt_diff(d)}, {d['tasks']} tasks)")
                L.append(f"| {arm} | {ph} | {fmt_rate(rc)} | {fmt_rate(rb)} | {fmt_diff(d)} | {v} |")
                if v == "worse":
                    regressions.append(f"{arm} {ph}: worse beyond noise, {fmt_diff(d)}")
            else:
                L.append(f"| {arm} | {ph} | {fmt_rate(rc)} | – | – | – |")
            n, bn = rc["runs"], rb["runs"]
            if n and bn and 100 * (rb["rate"] - rc["rate"]) > drop:
                regressions.append(f"{arm} {ph}: {pct(rb['pass'], bn)} → {pct(rc['pass'], n)}")
    if base:
        L += ["", "**Verdict:** " + ("; ".join(verdicts) if verdicts else "no task in common with the baseline")]

    L += ["", "## Per family (first + follow-up)", "", "| family | arm | now | baseline |", "|---|---|---|---|"]
    for f in sorted({fam.get(t, "?") for (t, _, _) in now}):
        for arm in arms:
            keys = [k for k in now if fam.get(k[0], "?") == f and k[1] == arm]
            p, n = rate([now[k] for k in keys])
            bp, bn = rate([was[k] for k in keys if k in was])
            L.append(f"| {f} | {arm} | {pct(p, n)} | {pct(bp, bn)} |")

    L += ["", "## Per task", "", "| task | arm | phase | now | baseline |", "|---|---|---|---|---|"]
    for k in sorted(now):
        t, arm, ph = k
        p, n = rate([now[k]])
        bp, bn = rate([was[k]]) if k in was else (0, 0)
        mark = ""
        if bn and bp >= 2 and p <= 1:
            mark = " ⚠"
            regressions.append(f"{t} {arm} {ph}: {bp}/{bn} → {p}/{n}")
        elif bn and p > bp:
            mark = " ↑"
        L.append(f"| {t} | {arm} | {ph} | {p}/{n}{mark} | {f'{bp}/{bn}' if bn else '–'} |")

    L += ["", "## Regressions", ""]
    L += [f"- {r}" for r in regressions] or ["none"]
    costs = summary.get("arms", {})
    if costs:
        L += ["", "## Cost (after correctness)", "", "| arm | phase | median tokens | cost per pass |", "|---|---|---|---|"]
        for arm, phases in costs.items():
            for ph, s in phases.items():
                L.append(f"| {arm} | {ph} | {s.get('median_total', 0):,.0f} | ${s.get('cost_per_pass') or 0:.3f} |")
    return L, regressions


def append_history(path, run, set_name, engine):
    now, summary = load(run)
    meta = summary.get("meta", {})
    new = not Path(path).exists()
    with open(path, "a", newline="") as f:
        w = csv.writer(f)
        if new:
            w.writerow(["started", "run", "set", "model", "arm", "engine", "bench",
                        "first_pass", "first_runs", "followup_pass", "followup_runs", "median_cost_first"])
        for arm in sorted({a for (_, a, _) in now}):
            row = [meta.get("started", ""), Path(run).name, set_name, meta.get("model", ""), arm, engine, git_sha(HERE)]
            for ph in PHASES:
                p, n = rate([v for (t, a, q), v in now.items() if a == arm and q == ph])
                row += [p, n]
            row.append(summary.get("arms", {}).get(arm, {}).get("first", {}).get("median_cost", ""))
            w.writerow(row)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("run")
    ap.add_argument("--baseline", help="an earlier judged run (default: none)")
    ap.add_argument("--history", help="append to this CSV")
    ap.add_argument("--set", default="", help="the question set's name, for the history")
    ap.add_argument("--engine", default="", help="the engine commit, for the history")
    ap.add_argument("--drop", type=float, default=10.0, help="a phase regresses past this many points (default 10)")
    a = ap.parse_args()
    lines, regressions = compare(a.run, a.baseline, a.drop)
    text = "\n".join(lines) + "\n"
    (Path(a.run) / "compare.md").write_text(text)
    print(text)
    if a.history:
        append_history(a.history, a.run, a.set, a.engine)
    raise SystemExit(2 if regressions else 0)


if __name__ == "__main__":
    main()
