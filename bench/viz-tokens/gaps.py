#!/usr/bin/env python3
"""#1044: mine finished runs for gaps in the framework — no API calls.

For each arm it groups every failed chart call by a normalised error
signature and shows, per group, how often it happened, in which tasks, the
SQL (or code / spec) that failed and the one that then worked: what the
model expected against what the engine accepts. Then every judge failure and
low readability score, with the judge's reason. A cluster that repeats
across tasks is an issue candidate.

  python gaps.py results/pilot [results/pilot-lean …] [--arm anofox-lean] > gaps.md
"""
import argparse
import json
import re
from collections import defaultdict
from pathlib import Path


def signature(err):
    """The error with the specifics taken out, so the same problem groups."""
    s = err.split("\n")[0]
    s = re.sub(r"^statement \d+: ", "", s)
    s = re.sub(r"\bLINE \d+:.*", "", s)
    s = re.sub(r'"[^"]*"', '"…"', s)
    s = re.sub(r"'[^']*'", "'…'", s)
    s = re.sub(r"\b\d+(\.\d+)?\b", "N", s)
    # keep the role a cast error names: that is the gap
    s = re.sub(r"Type with name (\w+) does not exist.*", r"Type with name \1 does not exist", s)
    return s.strip()[:200]


def chart_calls(transcript):
    """(input, ok, result text) for every chart tool call, in order."""
    uses, out = {}, []
    for m in transcript:
        if m["role"] == "assistant":
            for b in m["content"]:
                if b.get("type") == "tool_use" and b["name"] in ("render", "run_python"):
                    uses[b["id"]] = b["input"]
        elif m["role"] == "user" and isinstance(m["content"], list):
            for r in m["content"]:
                if r.get("tool_use_id") in uses:
                    out.append((uses[r["tool_use_id"]], not r.get("is_error"), str(r["content"])))
    return out


def artifact(inp):
    if "code" in inp:
        return inp["code"]
    if "spec" in inp:
        return f"-- sql\n{inp['sql']}\n-- spec\n{json.dumps(inp['spec'])[:1500]}"
    return inp.get("sql", "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results", nargs="+")
    ap.add_argument("--arm", action="append", help="only these arms (repeatable)")
    a = ap.parse_args()
    runs, judg = [], {}
    for d in map(Path, a.results):
        for l in (d / "runs.jsonl").read_text().splitlines():
            r = json.loads(l)
            r["_dir"] = d
            runs.append(r)
        jp = d / "judgments.jsonl"
        if jp.exists():
            for j in map(json.loads, jp.read_text().splitlines()):
                judg[(j["run"], j["phase"])] = j
    if a.arm:
        runs = [r for r in runs if r["arm"] in a.arm]

    clusters = defaultdict(list)  # (arm, signature) -> [(task, failed, fixed)]
    for r in runs:
        tp = Path(r["work"]) / "transcript.json"
        if not tp.exists():
            continue
        calls = chart_calls(json.loads(tp.read_text()))
        for i, (inp, ok, text) in enumerate(calls):
            if ok:
                continue
            fixed = next((c[0] for c in calls[i + 1:] if c[1]), None)
            clusters[(r["arm"], signature(text))].append((r["task"], artifact(inp), artifact(fixed) if fixed else None))

    L = ["# Gaps found in the runs (#1044)\n", "Failed chart calls, grouped by error; then the judge's findings.\n"]
    for arm in sorted({k[0] for k in clusters}):
        items = sorted(((s, v) for (ar, s), v in clusters.items() if ar == arm), key=lambda x: -len(x[1]))
        L.append(f"\n## {arm}: {sum(len(v) for _, v in items)} failed chart calls, {len(items)} kinds\n")
        for s, v in items:
            tasks = sorted({t for t, _, _ in v})
            L.append(f"### {len(v)}× in {len(tasks)} task(s): `{s}`\n")
            L.append("Tasks: " + ", ".join(tasks) + "\n")
            t, failed, fixed = v[0]
            L.append(f"Failed ({t}):\n```\n{failed.strip()}\n```")
            L.append("Then worked:\n```\n" + (fixed.strip() if fixed else "(never)") + "\n```\n")

    L.append("\n## Judge: fails and low readability\n")
    L.append("| run | phase | numbers | chart | readable | reason |")
    L.append("|---|---|---|---|---|---|")
    for r in runs:
        for ph in r["phases"]:
            j = judg.get((r["run"], ph["phase"]))
            if j and (not j["pass"] or (j.get("readable") or 5) <= 3):
                reason = (j.get("reason") or "").replace("|", "/").replace("\n", " ")
                L.append(f"| {r['run']} | {ph['phase']} | {j.get('numbers_correct')} | {j.get('chart_answers')} | {j.get('readable')} | {reason} |")
    print("\n".join(L))


if __name__ == "__main__":
    main()
