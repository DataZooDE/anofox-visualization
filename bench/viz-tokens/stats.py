"""#1122: the uncertainty of a pass rate — standard library only.

The unit is the task: repeats of one task are correlated (same question, same
data, same reference), so the bootstrap resamples tasks with all their runs,
never single runs. A Wilson interval over all runs is given for reference; it
treats every run as independent and is too narrow when repeats agree.

    by_task = {"ts-yoy": [True, True, False], "dist-box": [True], …}
    rate_ci(by_task)              -> {"pass", "runs", "rate", "boot": [lo, hi], "wilson": [lo, hi], "tasks"}
    diff_ci(a, b)                 -> a − b with a CI, paired by task when they share tasks
    verdict(d)                    -> "better" / "worse" / "within noise"
"""
import math
import random

N_BOOT = 4000
SEED = 1122
Z95 = 1.959963984540054


def wilson(k, n, z=Z95):
    """Wilson score interval for k successes in n independent trials."""
    if n == 0:
        return [float("nan"), float("nan")]
    p = k / n
    den = 1 + z * z / n
    mid = (p + z * z / (2 * n)) / den
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / den
    # exact at the edges (the formula gives 1 − 1e-16 for k = n)
    return [0.0 if k == 0 else max(0.0, mid - half), 1.0 if k == n else min(1.0, mid + half)]


def _quantiles(xs, lo=0.025, hi=0.975):
    xs = sorted(xs)
    n = len(xs)

    def q(p):
        k = (n - 1) * p
        f, c = math.floor(k), math.ceil(k)
        return xs[f] + (xs[c] - xs[f]) * (k - f)
    return [q(lo), q(hi)]


def _counts(by_task):
    """[(passes, runs)] per task, tasks without runs dropped."""
    return [(sum(bool(x) for x in v), len(v)) for _, v in sorted(by_task.items()) if v]


def rate_ci(by_task, n_boot=N_BOOT, seed=SEED):
    """Pass rate over all runs, its task-clustered bootstrap CI (resampling
    tasks, the rate = passes ÷ runs of the resample) and the Wilson CI."""
    c = _counts(by_task)
    k = sum(p for p, _ in c)
    n = sum(r for _, r in c)
    out = {"pass": k, "runs": n, "tasks": len(c), "rate": k / n if n else float("nan"),
           "wilson": wilson(k, n)}
    if not c:
        out["boot"] = [float("nan"), float("nan")]
        return out
    rnd = random.Random(seed)
    m = len(c)
    boots = []
    for _ in range(n_boot):
        bk = bn = 0
        for _ in range(m):
            p, r = c[rnd.randrange(m)]
            bk += p
            bn += r
        boots.append(bk / bn)
    out["boot"] = _quantiles(boots)
    return out


def _task_rates(by_task):
    return {t: sum(bool(x) for x in v) / len(v) for t, v in by_task.items() if v}


def diff_ci(a, b, n_boot=N_BOOT, seed=SEED):
    """a − b in pass rate with a 95 % bootstrap CI over tasks.

    Paired when a and b share tasks: on the shared tasks only, each task's
    pass rate (passes ÷ its repeats) in a minus in b, averaged over tasks; the
    bootstrap resamples the shared tasks. Each task weighs the same whatever
    its repeats, so runs with 1 and 3 repeats compare fairly. With no shared
    task it falls back to unpaired: each side's tasks resampled on their own.
    """
    ra, rb = _task_rates(a), _task_rates(b)
    shared = sorted(set(ra) & set(rb))
    rnd = random.Random(seed)
    if shared:
        d = [ra[t] - rb[t] for t in shared]
        m = len(d)
        boots = [sum(d[rnd.randrange(m)] for _ in range(m)) / m for _ in range(n_boot)]
        return {"diff": sum(d) / m, "ci": _quantiles(boots), "tasks": m, "paired": True,
                "a": sum(ra[t] for t in shared) / m, "b": sum(rb[t] for t in shared) / m}
    if not ra or not rb:
        return {"diff": float("nan"), "ci": [float("nan"), float("nan")], "tasks": 0, "paired": False,
                "a": float("nan"), "b": float("nan")}
    xa, xb = list(ra.values()), list(rb.values())
    ma, mb = sum(xa) / len(xa), sum(xb) / len(xb)
    boots = [sum(rnd.choice(xa) for _ in xa) / len(xa) - sum(rnd.choice(xb) for _ in xb) / len(xb)
             for _ in range(n_boot)]
    return {"diff": ma - mb, "ci": _quantiles(boots), "tasks": min(len(xa), len(xb)), "paired": False,
            "a": ma, "b": mb}


def verdict(d):
    """'better' when the whole CI is above 0, 'worse' when below, else 'within noise'."""
    lo, hi = d["ci"]
    if lo > 0:
        return "better"
    if hi < 0:
        return "worse"
    return "within noise"


def fmt_rate(s):
    """'41/50 82 % (boot 74–90, Wilson 69–90)'."""
    if not s["runs"]:
        return "–"
    b, w = s["boot"], s["wilson"]
    return (f"{s['pass']}/{s['runs']} {100 * s['rate']:.0f} % "
            f"(boot {100 * b[0]:.0f}–{100 * b[1]:.0f}, Wilson {100 * w[0]:.0f}–{100 * w[1]:.0f})")


def fmt_diff(d):
    """'+6.0 pts (−2.1 … +14.0)'."""
    if d["tasks"] == 0:
        return "–"
    lo, hi = d["ci"]
    return f"{100 * d['diff']:+.1f} pts ({100 * lo:+.1f} … {100 * hi:+.1f})"
