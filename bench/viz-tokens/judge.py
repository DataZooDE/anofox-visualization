#!/usr/bin/env python3
"""#1044: grade every answered phase of a results folder, blind to the arm.

The judge sees the question (and, for a follow-up, the request), the ground
truth rows (from the task's truth SQL), the written answer and the chart
image. It never sees the arm, the code or the SQL.

  ANTHROPIC_API_KEY=… python judge.py --data <dir> results/pilot
"""
import argparse
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

import duckdb

HERE = Path(__file__).resolve().parent
API = "https://api.anthropic.com/v1/messages"
JUDGE_MODEL = "claude-sonnet-5"

RUBRIC = """You grade one answer of a data assistant: a chart (the image) and a short written answer to a user's request. The ground truth rows were computed independently from the same data.

Judge:
1. numbers_correct: every number and ranking the written answer states agrees with the ground truth (rounding is fine; a claim the truth can't check is fine if it doesn't contradict it). false if a key number or the main conclusion is wrong or missing.
2. chart_answers: the chart shows what the request asks for (the right measure, grouping and breakdown) and agrees with the ground truth. For a follow-up, the chart must show the requested change.
3. readable: 1-5, how quickly a business reader gets the answer from the chart (title, labels, number format, chart type, clutter). 5 = at a glance, 3 = works with effort, 1 = misleading or unreadable.

Reply with JSON only: {"numbers_correct": true|false, "chart_answers": true|false, "readable": 1-5, "reason": "one or two sentences"}"""


def truth_rows(data, task, phase):
    sql = task["truth_sql"] if phase == "first" else task["followup_truth_sql"]
    c = duckdb.connect(str(Path(data) / f"{task['dataset']}.duckdb"), read_only=True)
    df = c.execute(sql).df()
    return df.head(60).to_csv(index=False)


def call(key, content):
    body = {"model": JUDGE_MODEL, "max_tokens": 1000, "system": RUBRIC,
            "messages": [{"role": "user", "content": content}]}
    req = urllib.request.Request(API, data=json.dumps(body).encode(), method="POST", headers={
        "x-api-key": key, "anthropic-version": "2023-06-01", "content-type": "application/json"})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 529) and attempt < 4:
                time.sleep(10 * (attempt + 1))
                continue
            raise


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--data", required=True)
    a = ap.parse_args()
    key = os.environ.get("ANTHROPIC_API_KEY") or sys.exit("ANTHROPIC_API_KEY is not set")
    out = Path(a.results)
    tasks = {t["id"]: t for t in map(json.loads, (HERE / "tasks.jsonl").read_text().splitlines())}
    done = set()
    jpath = out / "judgments.jsonl"
    if jpath.exists():
        done = {(j["run"], j["phase"]) for j in map(json.loads, jpath.read_text().splitlines())}
    with open(jpath, "a") as f:
        for rec in map(json.loads, (out / "runs.jsonl").read_text().splitlines()):
            t = tasks[rec["task"]]
            for ph in rec["phases"]:
                if (rec["run"], ph["phase"]) in done:
                    continue
                j = {"run": rec["run"], "phase": ph["phase"]}
                if not ph.get("answer") or not ph.get("chart"):
                    j.update(numbers_correct=False, chart_answers=False, readable=None, reason="no answer or no chart", usage={})
                else:
                    req = t["question"] if ph["phase"] == "first" else f"{t['question']}\n\nThen the user asked: {t['followup']}"
                    png = base64.b64encode((out / ph["chart"]).read_bytes()).decode()
                    content = [
                        {"type": "text", "text": f"Request:\n{req}\n\nGround truth rows (CSV):\n{truth_rows(a.data, t, ph['phase'])}\nWritten answer:\n{ph['answer']}\n\nThe chart:"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}},
                    ]
                    m = call(key, content)
                    text = "".join(b.get("text", "") for b in m["content"] if b.get("type") == "text")
                    try:
                        v = json.loads(text[text.index("{"): text.rindex("}") + 1])
                    except ValueError:
                        v = {"numbers_correct": None, "chart_answers": None, "readable": None, "reason": "judge reply not JSON: " + text[:200]}
                    j.update(v, usage=m.get("usage", {}))
                j["pass"] = bool(j.get("numbers_correct") and j.get("chart_answers"))
                f.write(json.dumps(j) + "\n")
                f.flush()
                print(f"{j['run']} {j['phase']}: {'PASS' if j['pass'] else 'fail'} ({j.get('readable')}) {j.get('reason', '')[:100]}", flush=True)


if __name__ == "__main__":
    main()
