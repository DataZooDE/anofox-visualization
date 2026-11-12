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

import auth
from pathlib import Path

import duckdb

HERE = Path(__file__).resolve().parent
API = "https://api.anthropic.com/v1/messages"
JUDGE_MODEL = "claude-sonnet-5"

RUBRIC = """You grade one answer of a data assistant: a chart (the image) and a short written answer to a user's request. The ground truth rows were computed independently from the same data.

Judge:
1. numbers_correct: every number, ranking and claim about the data that the written answer states agrees with the ground truth (rounding is fine; a claim the truth can't check is fine if it doesn't contradict it). false if any stated number, ranking or claim is wrong (a side remark too, e.g. the wrong peak month or date range), or a key number or the main conclusion is missing.
   A stated assumption: if the request is genuinely ambiguous (it leaves a definition open that changes the numbers, e.g. what counts as stock, which period, gross or net) and the answer names a reasonable definition the request allows, check its numbers against that definition, not the ground truth's: right under it is true. This never excuses a definition the request rules out, an unstated one, or numbers wrong under the stated one.
2. chart_answers: the chart shows what the request asks for (the right measure, grouping and breakdown) and agrees with the ground truth. For a follow-up, the chart must show the requested change. When a good chart is described, a chart that serves the request clearly worse (a bar chart where a map was asked for, no uncertainty band where one was asked for) is false.
   A chart whose axis or labels misstate the values (a share or ratio labelled 100x off, a wrong unit) does not answer the request. Interactive parts (a filter, a dropdown, a toggle) don't show in a static image: judge them from the chart's source, which you get next to the image.
   With no ground-truth rows, the right answer says there is no such data; then no chart is needed (chart_answers true when the answer says so). Otherwise no chart drawn means chart_answers false.
3. readable: 1-5, how quickly a business reader gets the answer from the chart (title, labels, number format, chart type, clutter). 5 = at a glance, 3 = works with effort, 1 = misleading or unreadable.

When the ground truth comes in several readings (the request can be meant more than one way): if the user was asked and replied, grade against the reading that reply picks. Otherwise grade against the reading the answer says it used, and if it says none, against reading A. Whether the assistant should have asked is measured separately; don't grade it here.

Reply with JSON only: {"numbers_correct": true|false, "chart_answers": true|false, "readable": 1-5, "reason": "one or two sentences"}"""


def rows_csv(data, task, sql):
    c = duckdb.connect(str(Path(data) / f"{task['dataset']}.duckdb"), read_only=True)
    df = c.execute(sql).df()
    # all of it up to 300 rows (#1044 pilot: a 60-row cut hid the answer's
    # rows from the judge, which then marked right answers wrong)
    more = f"\n({len(df)} rows, first 300 shown)" if len(df) > 300 else ""
    return df.head(300).to_csv(index=False) + more


def truth_rows(data, task, phase):
    key = "truth_sql" if phase == "first" else "followup_truth_sql"
    rs = task.get("readings")
    if not rs:
        return "Ground truth rows (CSV):\n" + rows_csv(data, task, task[key])
    # #1142: every reading, each with the reply a user gives to pick it
    return "Ground truth, one table per reading:\n" + "".join(
        f"\nReading {chr(65 + i)} (the user's reply that picks it: \"{r['reply']}\"):\n{rows_csv(data, task, r[key])}"
        for i, r in enumerate(rs))


def asked_text(phases, upto):
    """The questions the assistant asked the user, and the replies, up to this phase."""
    lines = []
    for ph in phases:
        for q in ph.get("asked") or []:
            lines.append(f"- The assistant asked: {q}")
        if ph["phase"] == upto:
            break
    return ("The assistant asked the user before answering (each question got the reading A reply above, "
            "or a go-ahead when there is one reading):\n" + "\n".join(lines) + "\n\n") if lines else ""


def call(key, content):
    # #1044: 1000 tokens cut some replies off before the JSON (about 1 in 12)
    # 11-11: 4000 still cut ~1 in 150 off mid-JSON — more room, and the
    # caller asks again when the reply doesn't parse
    body = {"model": JUDGE_MODEL, "max_tokens": 8000, "system": RUBRIC,
            "messages": [{"role": "user", "content": content}]}
    for attempt in range(5):
        # the auth header per attempt: a federated token may be re-exchanged (#1075)
        req = urllib.request.Request(API, data=json.dumps(body).encode(), method="POST", headers={
            **auth.headers(), "anthropic-version": "2023-06-01", "content-type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            if e.code == 401 and attempt < 4:
                auth.invalidate()
                continue
            if e.code in (429, 500, 502, 503, 529) and attempt < 4:
                time.sleep(10 * (attempt + 1))
                continue
            raise


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--data", required=True)
    a = ap.parse_args()
    key = None
    if not auth.available():
        sys.exit("no Claude API credential: set ANTHROPIC_API_KEY, or the ANTHROPIC_FEDERATION_* variables (auth.py)")
    out = Path(a.results)
    tasks = {t["id"]: t for f in ("tasks.jsonl", "coverage.jsonl")
             for t in map(json.loads, (HERE / f).read_text().splitlines())}
    done = set()
    jpath = out / "judgments.jsonl"
    if jpath.exists():
        # a reply that wasn't JSON is judged again (kept lines stay as they are)
        keep = [l for l in jpath.read_text().splitlines()
                if not json.loads(l).get("reason", "").startswith("judge reply not JSON")]
        jpath.write_text("".join(l + "\n" for l in keep))
        done = {(j["run"], j["phase"]) for j in map(json.loads, keep)}
    with open(jpath, "a") as f:
        for rec in map(json.loads, (out / "runs.jsonl").read_text().splitlines()):
            t = tasks[rec["task"]]
            for ph in rec["phases"]:
                if (rec["run"], ph["phase"]) in done:
                    continue
                j = {"run": rec["run"], "phase": ph["phase"]}
                if not ph.get("answer"):
                    j.update(numbers_correct=False, chart_answers=False, readable=None, reason="no answer", usage={})
                else:
                    req = t["question"] if ph["phase"] == "first" else f"{t['question']}\n\nThen the user asked: {t['followup']}"
                    expect = (f"\nA good chart for the first request: {t['expect']} (another kind of chart that answers "
                              "the request as well is fine; one that answers it clearly worse is not).\n") if t.get("expect") else ""
                    # #1121: the chart's source shows what the image can't (a
                    # filter, an input); capped so a big spec doesn't crowd out the rest
                    src = (ph.get("artifact") or "")[:8000]
                    src = f"The chart's source (what the assistant wrote):\n{src}\n\n" if src else ""
                    head = (f"Request:\n{req}\n{expect}\n{truth_rows(a.data, t, ph['phase'])}\n"
                            f"{asked_text(rec['phases'], ph['phase'])}Written answer:\n{ph['answer']}\n\n{src}")
                    # 11-11: no chart is judged too — right for a question the
                    # data can't answer, chart_answers false for any other
                    if ph.get("chart"):
                        png = base64.b64encode((out / ph["chart"]).read_bytes()).decode()
                        content = [{"type": "text", "text": head + "The chart:"},
                                   {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}}]
                    else:
                        content = [{"type": "text", "text": head + "No chart was drawn."}]
                    for attempt in range(3):
                        m = call(key, content)
                        text = "".join(b.get("text", "") for b in m["content"] if b.get("type") == "text")
                        try:
                            v = json.loads(text[text.index("{"): text.rindex("}") + 1])
                            break
                        except ValueError:
                            v = {"numbers_correct": None, "chart_answers": None, "readable": None, "reason": "judge reply not JSON: " + text[:200]}
                    j.update(v, usage=m.get("usage", {}))
                j["pass"] = bool(j.get("numbers_correct") and j.get("chart_answers"))
                f.write(json.dumps(j) + "\n")
                f.flush()
                print(f"{j['run']} {j['phase']}: {'PASS' if j['pass'] else 'fail'} ({j.get('readable')}) {j.get('reason', '')[:100]}", flush=True)


if __name__ == "__main__":
    main()
