#!/usr/bin/env python3
"""#1044: the report of a results folder (runs.jsonl + judgments.jsonl) —
report.md and summary.json in the same folder.

  python report.py results/pilot [results/pilot-lean …]

Several folders (the same tasks, run apart, e.g. a later arm) are read as one;
the report goes to the first.
"""
import json
import math
import random
import sys
from collections import defaultdict
from pathlib import Path
from statistics import median

from scipy.stats import binomtest, wilcoxon

sys.path.insert(0, str(Path(__file__).resolve().parent))
from stats import diff_ci, fmt_diff, rate_ci, verdict  # noqa: E402
import votes  # noqa: E402

PHASES = ("first", "followup")


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    k = (len(xs) - 1) * q
    lo, hi = math.floor(k), math.ceil(k)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def tokens(ph):
    t = ph["tokens"]
    inp = t["input_tokens"] + t["cache_read_input_tokens"] + t["cache_creation_input_tokens"]
    return {"input": inp, "output": t["output_tokens"], "total": inp + t["output_tokens"], "cost": ph["cost"],
            "cache_read": t["cache_read_input_tokens"]}


def tool_stats(ph):
    tools = [x for c in ph["calls"] for x in c["tools"]]
    chart = [x for x in tools if x["kind"] in ("chart", "repair")]
    return {"calls": len(ph["calls"]), "explore": sum(x["kind"] == "explore" for x in tools),
            "chart_calls": len(chart), "chart_failed": sum(not x["ok"] for x in chart),
            "first_try": bool(chart) and chart[0]["ok"]}


def split_output(ph):
    """Each model call's output tokens, given to the kind of tool it called
    (shared equally when it called several): where the writing went."""
    out = defaultdict(float)
    for c in ph["calls"]:
        n = c["usage"].get("output_tokens", 0)
        kinds = [x["kind"] for x in c["tools"]] or ["none"]
        for k in kinds:
            out[k] += n / len(kinds)
    return out


def geo_ci(ratios, n=4000, seed=1044):
    logs = [math.log(r) for r in ratios if r > 0]
    if not logs:
        return float("nan"), float("nan"), float("nan")
    rnd = random.Random(seed)
    boots = sorted(sum(rnd.choice(logs) for _ in logs) / len(logs) for _ in range(n))
    return math.exp(sum(logs) / len(logs)), math.exp(boots[int(0.025 * n)]), math.exp(boots[int(0.975 * n)])


def by_task(R):
    """{task: [pass, …]} over its repeats."""
    out = defaultdict(list)
    for x in R:
        out[x["task"]].append(bool(x["pass"]))
    return out


def main():
    dirs = [Path(d) for d in sys.argv[1:]]
    out = dirs[0]
    meta = json.loads((out / "meta.json").read_text())
    runs, judg = [], {}
    for d in dirs:
        m = json.loads((d / "meta.json").read_text())
        if d != out:  # a later arm: its arms, references and prefix counts join
            meta["arms"] += [a for a in m["arms"] if a not in meta["arms"]]
            for k in ("ref_chars", "ref_sha", "prefix_tokens"):
                meta.setdefault(k, {}).update(m.get(k, {}))
        runs += [json.loads(l) for l in (d / "runs.jsonl").read_text().splitlines() if l.strip()]
        jp = d / "judgments.jsonl"
        if jp.exists():
            for j in map(json.loads, jp.read_text().splitlines()):
                judg[(j["run"], j["phase"])] = j
    arms = meta["arms"]
    prefix = meta.get("prefix_tokens", {})

    # one row per run and phase
    rows = []
    for r in runs:
        for ph in r["phases"]:
            j = judg.get((r["run"], ph["phase"]))
            rows.append({"run": r["run"], "task": r["task"], "arm": r["arm"], "rep": r["rep"], "phase": ph["phase"],
                         "answered": bool(ph.get("answer")), "chart": bool(ph.get("chart")),
                         "pass": None if j is None else j["pass"], "readable": None if j is None else j.get("readable"),
                         **tokens(ph), **tool_stats(ph), "split": split_output(ph),
                         "artifact_chars": len(ph.get("artifact") or "")})
    judged = bool(judg)
    L = [f"# Talk to data: anofox vs Python vs Vega-Lite (#1044)\n",
         f"Model `{meta['model']}`, effort `{meta['effort']}`, {meta['reps']} repeats, {len(meta['tasks'])} tasks, started {meta['started']}."
         + (" **Dry run (scripted turns): the numbers mean nothing.**" if meta.get("dry") else ""),
         f"Reference per arm (system prompt, cached): " + ", ".join(f"{a} {prefix.get(a, '?')} tokens ({meta['ref_chars'][a]:,} chars)" for a in arms) + ".",
         "" if judged else "\n**Not judged yet** (judge.py): pass rates are missing.\n"]

    summary = {"meta": meta, "arms": {}, "paired": {}}
    for phase in PHASES:
        L.append(f"\n## {'First answer' if phase == 'first' else 'Follow-up edit'}\n")
        if judged:
            L.append("Pass rate with a 95 % CI (#1122): **boot** = bootstrap over tasks, a task's repeats kept together "
                     "(the one to read); **Wilson** = every run counted as independent (for reference; too narrow when "
                     "repeats agree). Claims of progress need `--reps 3`.\n")
        L.append("| | runs | pass | pass rate (95 % CI) | chart first try | median tokens (p90) | median output | median cost | tokens per pass | cost per pass | median calls | chart repairs | readable |")
        L.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        for arm in arms:
            R = [x for x in rows if x["arm"] == arm and x["phase"] == phase]
            if not R:
                continue
            passes = [x for x in R if x["pass"]]
            npass = len(passes)
            tot = sum(x["total"] for x in R)
            cst = sum(x["cost"] for x in R)
            rd = [x["readable"] for x in R if isinstance(x["readable"], (int, float))]
            s = {"runs": len(R), "pass": npass if judged else None,
                 "first_try": sum(x["first_try"] for x in R),
                 "median_total": median(x["total"] for x in R), "p90_total": pct([x["total"] for x in R], 0.9),
                 "median_output": median(x["output"] for x in R), "median_cost": median(x["cost"] for x in R),
                 "tokens_per_pass": tot / npass if npass else None, "cost_per_pass": cst / npass if npass else None,
                 "median_calls": median(x["calls"] for x in R), "repairs": sum(x["chart_failed"] for x in R),
                 "readable": sum(rd) / len(rd) if rd else None}
            ci = "–"
            if judged:
                rc = rate_ci(by_task(R))
                s.update(pass_rate=rc["rate"], pass_ci_boot=rc["boot"], pass_ci_wilson=rc["wilson"], tasks=rc["tasks"])
                ci = (f"{100 * rc['rate']:.0f} % (boot {100 * rc['boot'][0]:.0f}–{100 * rc['boot'][1]:.0f}, "
                      f"Wilson {100 * rc['wilson'][0]:.0f}–{100 * rc['wilson'][1]:.0f})")
            summary["arms"].setdefault(arm, {})[phase] = s
            f = lambda v, d=0: "–" if v is None else f"{v:,.{d}f}"
            money = lambda v: "–" if v is None else f"${v:.3f}"
            L.append(f"| {arm} | {s['runs']} | {f(s['pass']) if judged else '–'} | {ci} | {s['first_try']} | {f(s['median_total'])} ({f(s['p90_total'])}) "
                     f"| {f(s['median_output'])} | ${s['median_cost']:.3f} | {f(s['tokens_per_pass'])} | {money(s['cost_per_pass'])} "
                     f"| {f(s['median_calls'], 1)} | {s['repairs']} | {f(s['readable'], 1)} |")

        # paired, per task: the median over repeats, each anofox variant against python and vegalite
        for ours in [a for a in ("anofox", "anofox-lean") if a in arms]:
            L.append(f"\n**Paired by task ({ours} ÷ other; < 1 means {ours} needs less).** Geometric mean of the per-task ratios of the medians over repeats, 95 % bootstrap CI over tasks, Wilcoxon signed-rank on the log ratios. Success: McNemar (exact) over the same task and repeat.\n")
            L.append("| vs | metric | ratio | 95 % CI | Wilcoxon p | tasks |")
            L.append("|---|---|---|---|---|---|")
            for other in [a for a in arms if not a.startswith("anofox")]:
                cmp = summary["paired"].setdefault(phase, {}).setdefault(f"{ours} vs {other}", {})
                for metric in ("total", "output", "cost"):
                    ratios = []
                    for t in sorted({x["task"] for x in rows}):
                        a = [x[metric] for x in rows if x["task"] == t and x["arm"] == ours and x["phase"] == phase]
                        b = [x[metric] for x in rows if x["task"] == t and x["arm"] == other and x["phase"] == phase]
                        if a and b and median(b) > 0:
                            ratios.append(median(a) / median(b))
                    g, lo, hi = geo_ci(ratios)
                    p = wilcoxon([math.log(r) for r in ratios]).pvalue if len(ratios) >= 5 and any(r != 1 for r in ratios) else float("nan")
                    cmp[metric] = {"ratio": g, "ci": [lo, hi], "p": p, "n": len(ratios)}
                    L.append(f"| {other} | {'cost ($)' if metric == 'cost' else metric + ' tokens'} | {g:.2f} | {lo:.2f}–{hi:.2f} | {p:.3g} | {len(ratios)} |")
                if judged:
                    key = lambda x: (x["task"], x["rep"])
                    A = {key(x): x["pass"] for x in rows if x["arm"] == ours and x["phase"] == phase}
                    B = {key(x): x["pass"] for x in rows if x["arm"] == other and x["phase"] == phase}
                    both = [k for k in A if k in B]
                    b = sum(bool(A[k]) and not B[k] for k in both)
                    c = sum(bool(B[k]) and not A[k] for k in both)
                    p = binomtest(b, b + c).pvalue if b + c else float("nan")
                    cmp["mcnemar"] = {"ours_only": b, "other_only": c, "p": p}
                    L.append(f"| {other} | pass (McNemar) | {ours} only {b}, {other} only {c} | | {p:.3g} | {len(both)} pairs |")
                    # #1122: the pass-rate difference, paired by task (each task's rate over its repeats)
                    d = diff_ci(by_task([x for x in rows if x["arm"] == ours and x["phase"] == phase]),
                                by_task([x for x in rows if x["arm"] == other and x["phase"] == phase]))
                    cmp["pass_diff"] = {**d, "verdict": verdict(d)}
                    L.append(f"| {other} | pass rate, {ours} − {other} (bootstrap over tasks) | {fmt_diff(d)} | | {verdict(d)} | {d['tasks']} tasks |")

        # where the output tokens went
        L.append("\n**Where the output went** (mean output tokens per run, by the tool each model call used; thinking included) and the reference read per run (cached prefix × calls):\n")
        L.append("| | explore | chart | repair | answer | other | reference read | artifact (chars) |")
        L.append("|---|---|---|---|---|---|---|---|")
        for arm in arms:
            R = [x for x in rows if x["arm"] == arm and x["phase"] == phase]
            if not R:
                continue
            m = lambda k: sum(x["split"].get(k, 0) for x in R) / len(R)
            ref = sum(x["calls"] for x in R) / len(R) * prefix[arm] if prefix.get(arm) else None
            L.append(f"| {arm} | {m('explore'):,.0f} | {m('chart'):,.0f} | {m('repair'):,.0f} | {m('answer'):,.0f} | {m('none'):,.0f} "
                     f"| {'–' if ref is None else f'{ref:,.0f}'} | {sum(x['artifact_chars'] for x in R) / len(R):,.0f} |")

    fam, amb = {}, {}
    here = Path(__file__).resolve().parent
    for f in ("tasks.jsonl", "coverage.jsonl"):
        for t in map(json.loads, (here / f).read_text().splitlines()):
            if "family" in t:
                fam[t["id"]] = t["family"]
            amb[t["id"]] = t.get("ambiguity", {})
    if judged and any(x["task"] in fam for x in rows):
        L.append("\n## Per family (pass / runs, first answer + follow-up)\n")
        L.append("| family | " + " | ".join(arms) + " |")
        L.append("|---|" + "---|" * len(arms))
        for fm in sorted(set(fam.values())):
            cells = []
            for arm in arms:
                R = [x for x in rows if fam.get(x["task"]) == fm and x["arm"] == arm]
                cells.append(f"{sum(bool(x['pass']) for x in R)}/{len(R)}" if R else "–")
            L.append(f"| {fm} | " + " | ".join(cells) + " |")

    L.append("\n## Per task (median total tokens over repeats; pass count)\n")
    L.append("| task | phase | " + " | ".join(arms) + " |")
    L.append("|---|---|" + "---|" * len(arms))
    for t in sorted({x["task"] for x in rows}):
        for phase in PHASES:
            cells = []
            for arm in arms:
                R = [x for x in rows if x["task"] == t and x["arm"] == arm and x["phase"] == phase]
                if not R:
                    cells.append("–")
                    continue
                ps = sum(bool(x["pass"]) for x in R)
                cells.append(f"{median(x['total'] for x in R):,.0f}" + (f" ({ps}/{len(R)})" if judged else ""))
            L.append(f"| {t} | {phase} | " + " | ".join(cells) + " |")

    # #1121: the judge's own noise — how often its calls on one answer agree,
    # and the answers it was unsure about (a second look settles those)
    voted = [j for j in judg.values() if len(j.get("votes") or []) > 1]
    if voted:
        sa = votes.self_agreement(voted)
        n_votes = max(len(j["votes"]) for j in voted)
        L.append("\n## Judge agreement (#1121)\n")
        L.append(f"{n_votes} judge calls per answer; the majority counts. Self-agreement (all calls agree on pass): "
                 f"**{100 * sa:.0f} %** of {len(voted)} answers. An **unsure** answer (the calls split) is counted by its "
                 "majority above. `unsure.py` exports those answers for a second look, and its `apply` writes the result back.\n")
        L.append("| arm | unsure | pass, sure answers only | resolved by a second look |")
        L.append("|---|---|---|---|")
        for arm in arms:
            J = [judg[(x["run"], x["phase"])] for x in rows if x["arm"] == arm and (x["run"], x["phase"]) in judg]
            uns = [j for j in J if j.get("unsure")]
            sure = [j for j in J if not j.get("unsure")]
            res = sum(1 for j in J if j.get("resolved_by"))
            L.append(f"| {arm} | {len(uns)}/{len(J)} | "
                     f"{sum(bool(j['pass']) for j in sure)}/{len(sure)} | {res} |")
        summary["judge_self_agreement"] = sa

    # #1116: the claim check against the judge — how many answers it would
    # flag (shadow) or sent back (on), among the judge's right and wrong ones
    cc = [(r, ph) for r in runs for ph in r["phases"] if ph.get("claim_check")]
    if cc and judged:
        L.append("\n## Claim check (#1116)\n")
        L.append(f"Mode `{meta.get('claim_check', '?')}`. An answer counts as flagged when its first answer call had numbers nothing backs.\n")
        L.append("| arm | numbers right: flagged | numbers wrong: flagged |")
        L.append("|---|---|---|")
        for arm in arms:
            c = {True: [0, 0], False: [0, 0]}
            for r, ph in cc:
                j = judg.get((r["run"], ph["phase"]))
                if r["arm"] != arm or not j or j.get("numbers_correct") is None:
                    continue
                k = bool(j["numbers_correct"])
                c[k][0] += bool(ph["claim_check"][0]["unsupported"])
                c[k][1] += 1
            L.append(f"| {arm} | {c[True][0]}/{c[True][1]} | {c[False][0]}/{c[False][1]} |")
        summary["claim_check"] = meta.get("claim_check")

    # #1142: asking back. A phase is where the question is decided: the first
    # phase, or the follow-up when only the follow-up can be read two ways
    # (after a first answer the reading is settled).
    if any("asked" in ph for r in runs for ph in r["phases"]):
        L.append("\n## Asking back (#1142)\n")
        L.append("Per task phase, from how far its readings' results diverge (ambiguity.py): **ambiguous** = the "
                 "readings lead to different answers, so the right move is to ask first; **clear / harmless** = "
                 "answer directly. Detection = asked on ambiguous; false asks = asked on clear or harmless.\n")
        L.append("| arm | detection | false asks | pass on ambiguous (after asking or not) |")
        L.append("|---|---|---|---|")
        kpi = {}
        for arm in arms:
            det, fa, ok = [0, 0], [0, 0], [0, 0]
            for r in runs:
                if r["arm"] != arm:
                    continue
                a = amb.get(r["task"], {})
                decides = "first" if a.get("first") == "ambiguous" or a.get("followup") != "ambiguous" else "followup"
                for ph in r["phases"]:
                    asked = bool(ph.get("asked"))
                    if a.get(ph["phase"]) == "ambiguous" and ph["phase"] == decides:
                        det[0] += asked
                        det[1] += 1
                        j = judg.get((r["run"], ph["phase"]))
                        if j:
                            ok[0] += bool(j.get("pass"))
                            ok[1] += 1
                    elif a.get(ph["phase"], "clear") != "ambiguous":
                        fa[0] += asked
                        fa[1] += 1
            share = lambda c: f"{c[0]}/{c[1]} ({100 * c[0] / c[1]:.0f} %)" if c[1] else "–"
            L.append(f"| {arm} | {share(det)} | {share(fa)} | {share(ok)} |")
            kpi[arm] = {"detection": det, "false_asks": fa, "pass_ambiguous": ok}
        summary["asking_back"] = kpi

    (out / "report.md").write_text("\n".join(L) + "\n")
    (out / "summary.json").write_text(json.dumps(summary, indent=1, default=str))
    print(out / "report.md")


if __name__ == "__main__":
    main()
