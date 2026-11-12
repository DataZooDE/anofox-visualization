#!/usr/bin/env python3
"""#1121: a second look at the answers the judge was unsure about (its calls
split on pass).

  python unsure.py export <results> <data-dir>   the unsure answers, blind (no
                                                   judge verdicts), to
                                                   <results>/unsure-items.jsonl
                                                   for a grader that recomputes
                                                   from the data
  python unsure.py apply <results> <grades.jsonl> its verdicts replace the
                                                   majority (resolved_by)

items.jsonl lines: item, run, phase, arm, task, request, good_chart_note,
answer, asked, chart_png, artifact, database, reference_sql (one per reading).
grades.jsonl lines: item, numbers_correct, chart_answers, readable, reason.
The grader follows judge.py's RUBRIC; the data folder is the run's --data.
"""
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def tasks():
    return {t["id"]: t for f in ("tasks.jsonl", "coverage.jsonl")
            for t in map(json.loads, (HERE / f).read_text().splitlines())}


def export(results, data):
    res = Path(results)
    out = res / "unsure-items.jsonl"
    judg = {(j["run"], j["phase"]): j for j in map(json.loads, (res / "judgments.jsonl").read_text().splitlines())}
    T = tasks()
    items = []
    for rec in map(json.loads, (res / "runs.jsonl").read_text().splitlines()):
        t = T[rec["task"]]
        for ph in rec["phases"]:
            j = judg.get((rec["run"], ph["phase"]))
            if not j or not j.get("unsure") or j.get("resolved_by"):
                continue
            key = "truth_sql" if ph["phase"] == "first" else "followup_truth_sql"
            req = t["question"] if ph["phase"] == "first" else f"{t['question']}\n\nThen the user asked: {t['followup']}"
            readings = t.get("readings") or [{"reply": None, key: t[key]}]
            items.append({
                "item": len(items), "run": rec["run"], "phase": ph["phase"], "arm": rec["arm"], "task": rec["task"],
                "request": req, "good_chart_note": t.get("expect"), "answer": ph.get("answer") or "",
                "asked": ph.get("asked") or [],
                "chart_png": str(res / ph["chart"]) if ph.get("chart") else None, "artifact": ph.get("artifact"),
                "database": str(Path(data) / f"{t['dataset']}.duckdb"),
                "reference_sql": [{"reply": r.get("reply"), "sql": r[key]} for r in readings],
            })
    Path(out).write_text("".join(json.dumps(i) + "\n" for i in items))
    print(f"{len(items)} unsure answers → {out}")


def apply(results, grades):
    res = Path(results)
    key_of = {i["item"]: (i["run"], i["phase"])
              for i in map(json.loads, (res / "unsure-items.jsonl").read_text().splitlines())}
    grade = {key_of[g["item"]]: g for g in map(json.loads, Path(grades).read_text().splitlines())}
    lines, n = [], 0
    for j in map(json.loads, (res / "judgments.jsonl").read_text().splitlines()):
        g = grade.get((j["run"], j["phase"]))
        if g:
            j["majority"] = {k: j.get(k) for k in ("numbers_correct", "chart_answers", "pass")}
            j.update(numbers_correct=bool(g["numbers_correct"]), chart_answers=bool(g["chart_answers"]),
                     readable=g.get("readable", j.get("readable")), reason=g.get("reason", j.get("reason")),
                     resolved_by="second look")
            j["pass"] = j["numbers_correct"] and j["chart_answers"]
            n += 1
        lines.append(json.dumps(j))
    (res / "judgments.jsonl").write_text("".join(l + "\n" for l in lines))
    print(f"{n} unsure answers resolved")


if __name__ == "__main__":
    if len(sys.argv) != 4 or sys.argv[1] not in ("export", "apply"):
        sys.exit(__doc__)
    if sys.argv[1] == "export":
        export(sys.argv[2], sys.argv[3])
    else:
        apply(sys.argv[2], sys.argv[3])
