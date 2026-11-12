"""#1121: the verdict of several judge calls on one answer."""


def majority(votes):
    """The verdict of several judge calls: each criterion by majority, the
    median readability, the reason of a call that agrees with the outcome.
    `unsure` when the calls disagree on pass (#1121: those get a second look
    instead of counting either way)."""
    ok = [v for v in votes if v["numbers_correct"] is not None]
    if not ok:
        return dict(votes[0], unsure=True)
    maj = lambda k: sum(bool(v[k]) for v in ok) * 2 > len(ok)
    nc, ca = maj("numbers_correct"), maj("chart_answers")
    passes = [bool(v["numbers_correct"] and v["chart_answers"]) for v in ok]
    rs = sorted(v["readable"] for v in ok if isinstance(v.get("readable"), (int, float)))
    reason = next((v["reason"] for v in ok if bool(v["numbers_correct"] and v["chart_answers"]) == (nc and ca)), ok[0]["reason"])
    return {"numbers_correct": nc, "chart_answers": ca, "readable": rs[len(rs) // 2] if rs else None,
            "reason": reason, "unsure": len(set(passes)) > 1 or len(ok) < len(votes)}


def self_agreement(judgments):
    """Share of judged answers whose calls all agreed on pass, or None
    without votes: the judge's own noise, reported with every run."""
    js = [j for j in judgments if len(j.get("votes") or []) > 1]
    return (sum(not j.get("unsure") for j in js) / len(js)) if js else None
