#!/usr/bin/env python3
"""#1263: score the claim check (claims.py) on a finished run, offline — no
model calls. Each judged answer is checked again with the current code and
compared with the judge's `numbers_correct`:

    numbers   unsupported(answer, evidence): evidence = the question(s) and every
              tool output the run logged (cut at 4000 chars; tool inputs aren't
              logged, so a little stricter than the live check)
    words     word_problems(answer, digest): digest = the last chart's, or with
              --dashboard the final chart (the phase's `artifact`) drawn again by
              that build — to try a new digest on old answers

    python3 replay.py <run dir> [--dashboard target/release/dashboard --data <dir of
        <dataset>.duckdb> --boundaries web/data/boundaries] [--list]

A check change is worth a paid run only when it flags fewer right answers and
more wrong ones here.
"""
import argparse
import json
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import claims

HERE = Path(__file__).resolve().parent


def tasks():
    out = {}
    for f in ("tasks.jsonl", "coverage.jsonl"):
        for line in open(HERE / f):
            t = json.loads(line)
            out[t["id"]] = t
    return out


def redraw(jobs, dash, data, bounds):
    """(run, phase) → the chart/drawn digest of the phase's final chart."""
    dash, data = str(Path(dash).resolve()), Path(data).resolve()
    bounds = str(Path(bounds).resolve()) if bounds else None
    def one(job):
        run, phase, ds, sql = job
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "chart.sql").write_text(
                f"ATTACH '{data / (ds + '.duckdb')}' AS d (READ_ONLY);\n"
                f"SET search_path = 'memory.main,d.main';\n{sql}\n")
            extra = ["--boundaries", bounds] if bounds else []
            p = subprocess.run([dash, "--check", "chart.sql", "--json", "--digest", *extra],
                               cwd=d, capture_output=True, text=True, timeout=120)
        try:
            diags = json.loads(p.stdout).get("diagnostics", [])
        except ValueError:
            return (run, phase), ""
        drawn = [x for x in diags if x.get("code") == "chart/drawn"]
        return (run, phase), "what the chart shows\n" + "\n".join(
            f"statement {x.get('stmt', 0) - 2}: {x.get('message')}" for x in drawn) if drawn else ""
    with ThreadPoolExecutor(8) as ex:
        return dict(ex.map(one, jobs))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("run", type=Path)
    ap.add_argument("--dashboard")
    ap.add_argument("--data")
    ap.add_argument("--boundaries")
    ap.add_argument("--list", action="store_true", help="print each flagged answer with the judge's reason")
    a = ap.parse_args()
    T = tasks()
    judg = {}
    for line in open(a.run / "judgments.jsonl"):
        j = json.loads(line)
        judg[(j["run"], j["phase"])] = j
    runs = [json.loads(line) for line in open(a.run / "runs.jsonl")]
    fresh = {}
    if a.dashboard:
        jobs = [(r["run"], ph["phase"], r["dataset"], ph["artifact"])
                for r in runs for ph in r["phases"] if ph.get("artifact")]
        fresh = redraw(jobs, a.dashboard, a.data, a.boundaries)
    count = {(c, k): [0, 0] for c in ("numbers", "words") for k in (True, False)}
    flagged = []
    for r in runs:
        t = T.get(r["task"], {})
        ev = [t.get("question", "")]
        for ph in r["phases"]:
            if ph["phase"] == "followup":
                ev.append(t.get("followup", ""))
            ev += [x["out"] for c in ph.get("calls", []) for x in c.get("tools", []) if x.get("out")]
            j = judg.get((r["run"], ph["phase"]))
            ans = ph.get("answer") or ""
            if not j or j.get("numbers_correct") is None or not ans:
                continue
            k = bool(j["numbers_correct"])
            digest = fresh.get((r["run"], ph["phase"])) if a.dashboard else claims.latest_digest(ev)
            got = {"numbers": claims.unsupported(ans, ev), "words": claims.word_problems(ans, digest or "")}
            for c, bad in got.items():
                count[(c, k)][0] += bool(bad)
                count[(c, k)][1] += 1
                if bad:
                    flagged.append((c, k, r["run"], ph["phase"], bad, j.get("reason", "")))
    print("| check | numbers right: flagged | numbers wrong: flagged |\n|---|---|---|")
    for c in ("numbers", "words"):
        (fr, nr), (fw, nw) = count[(c, True)], count[(c, False)]
        print(f"| {c} | {fr}/{nr} | {fw}/{nw} |")
    if a.list:
        for c, k, run, phase, bad, reason in flagged:
            print(f"\n{c} {'RIGHT' if k else 'WRONG'} {run} {phase}: {bad}\n  judge: {reason[:300]}")


if __name__ == "__main__":
    main()
