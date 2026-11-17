"""#1116: the claim check — every number an answer states must be backed by
what the conversation's tools returned (query results, chart digests, the
model's own SQL, python's stdout) or the question, within the number's own
rounding: directly, or as a difference, ratio, share or % change of two
numbers from the same tool output. Deterministic, no LLM.

    unsupported(answer, evidence)  -> the claims (as written) nothing backs
    word_problems(answer, digest)  -> trend / peak / 'consistently highest'
                                      words the chart's digest contradicts (#1263)
"""
import datetime
import re

# a number as written: 1,234.5 · 12.5 · −3 · 8.3M · €2.8 M · 26.6 % · 1.2k
NUM = re.compile(
    r"(?<![\w.\-/])([-−+]?)(\d{1,3}(?:[,   ]\d{3})+|\d+)(?:\.(\d+))?"
    r"(?:\s?(%|k\b|K\b|M\b|m\b|bn\b|B\b|million\b|billion\b|thousand\b|Mio\b|Mrd\b))?"
    r"(?![\w\-/]|\.\d)"
)
SCALE = {"k": 1e3, "K": 1e3, "thousand": 1e3, "M": 1e6, "m": 1e6, "million": 1e6, "Mio": 1e6,
         "bn": 1e9, "B": 1e9, "billion": 1e9, "Mrd": 1e9}
# a date or a time is not a claim to check here: 2025-01-20, 20.01.2025, 14:30
DATE = re.compile(r"\d{4}-\d{2}(?:-\d{2})?|\d{1,2}\.\d{1,2}\.\d{2,4}|\d{1,2}:\d{2}")
EVIDENCE_NUM = re.compile(r"[-−]?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?")
MAX_PER_SOURCE = 250


class Claim:
    def __init__(self, text, value, tol, percent):
        self.text, self.value, self.tol, self.percent = text, value, tol, percent

    def __repr__(self):
        return f"Claim({self.text!r}, {self.value}, ±{self.tol})"


def claims(answer):
    """The checkable numbers of `answer`: not years, dates, times or small
    whole counts (≤ 12: 'top 3', '5 suppliers', 'month 9')."""
    masked = DATE.sub(lambda m: " " * len(m.group()), answer)
    out = []
    for m in NUM.finditer(masked):
        sign, whole, frac, unit = m.groups()
        digits = re.sub(r"[,   ]", "", whole)
        value = float(digits + ("." + frac if frac else ""))
        if sign in ("-", "−"):
            value = -value
        if not frac and not unit:
            if abs(value) <= 12 or 1900 <= value <= 2100:
                continue
        scale = SCALE.get(unit, 1.0)
        # the rounding the number was written with: 8.7 → ±0.05; 26,000 → ±500
        if frac:
            tol = 0.5 * 10 ** -len(frac)
        else:
            zeros = len(digits) - len(digits.rstrip("0")) if value else 0
            tol = 0.5 * 10 ** min(zeros, len(digits) - 1)
        out.append(Claim(m.group().strip(), value * scale, tol * scale + 1e-9, unit == "%"))
    return out


def numbers(text):
    vals = []
    for m in EVIDENCE_NUM.finditer(text):
        try:
            v = float(m.group().replace("−", "-"))
        except ValueError:
            continue
        if v == v and abs(v) != float("inf"):
            vals.append(v)
    # dedupe, keep order; a big table keeps its first rows
    seen, out = set(), []
    for v in vals:
        if v not in seen:
            seen.add(v)
            out.append(v)
    return out[:MAX_PER_SOURCE]


def groups(text):
    """The numbers that belong together, for derived claims: each row (a line,
    or a digest segment between ';' / '|'), and each column of a CSV."""
    rows = [r for r in re.split(r"[\n;|]", text) if r.strip()]
    out = [numbers(r) for r in rows]
    cells = [r.split(",") for r in text.splitlines() if "," in r]
    if len(cells) > 2 and len({len(c) for c in cells}) == 1:
        for j in range(len(cells[0])):
            out.append(numbers("\n".join(c[j] for c in cells)))
    return [g for g in out if len(g) > 1]


def backed(claim, sources, related=()):
    """Is `claim` a number of a source, or a difference / ratio / share / %
    change of two numbers that belong together (`related`), within its
    rounding?"""
    targets = [claim.value]
    if claim.percent:
        targets.append(claim.value / 100)  # a share written as 33 % of a 0.33
    for t in targets:
        tol = claim.tol if t == claim.value else claim.tol / 100
        for src in sources:
            if any(abs(v - t) <= tol or abs(abs(v) - abs(t)) <= tol for v in src):
                return True
    v = claim.value
    for src in related:
        for i, a in enumerate(src):
            for b in src[i + 1:]:
                for x, y in ((a, b), (b, a)):
                    if abs(abs(x - y) - abs(v)) <= claim.tol:
                        return True
                    if y:
                        r = x / y
                        if claim.percent and (abs(abs(r * 100 - 100) - abs(v)) <= claim.tol
                                              or abs(r * 100 - v) <= claim.tol):
                            return True
                        if not claim.percent and abs(r - v) <= claim.tol:
                            return True
    return False


def aggregates(text):
    """A CSV's column totals, means, medians, minima and maxima — 'revenue
    was 2.79M in 2024' sums a column the model saw row by row."""
    cells = [r.split(",") for r in text.splitlines() if "," in r]
    if len(cells) < 3 or len({len(c) for c in cells}) != 1:
        return []
    out = []
    for j in range(len(cells[0])):
        col = [v for c in cells[1:] for v in numbers(c[j])]
        if len(col) > 1:
            s = sorted(col)
            n = len(s)
            med = s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2
            out += [sum(col), sum(col) / n, med, s[0], s[-1]]
    return out


def unsupported(answer, evidence):
    """The claims of `answer` that no evidence text backs (as written)."""
    sources = [numbers(e) for e in evidence if e] + [aggregates(e) for e in evidence if e]
    related = [g for e in evidence if e for g in groups(e)]
    return [c.text for c in claims(answer) if not backed(c, sources, related)]


def feedback(bad):
    return ("Not accepted yet: these numbers in your answer appear in no query result, chart "
            "summary or the request (nor as a difference, ratio, share or % change of two such "
            f"numbers): {', '.join(bad)}. Check them — query the data if you need to — and call "
            "answer again. A number you computed yourself is fine if it is right; say how.")


# ------------------------------------------------------------------ words
# #1263: the words around the numbers — "rises steadily", "falls back to zero
# for the rest of the year", "WI_1 is consistently the highest" — checked
# against what the chart drew: the digest (ggplot-rs summary ≥ 0.60) names a
# series' largest move against its trend, its peaks, and who is highest at
# each shared x. Every rule fires only on a phrase it knows and a digest that
# contradicts it, so an answer it can't read passes.

DNUM = r"-?[\d.]+(?:e[-+]?\d+)?"
SERIES = re.compile(
    r"(?:(?P<key>[^;|:]+?) — )?(?P<n>\d+) marks?, x (?P<x0>.+?) … (?P<x1>.+?); y first (?P<first>" + DNUM + r"), last (?P<last>" + DNUM + r"), "
    r"min (?P<min>" + DNUM + r") at (?P<minx>[^,;|]+), max (?P<max>" + DNUM + r") at (?P<maxx>[^;|\n]+?)"
    r"(?:, band [^;|]*)?"
    r"(?:; against the trend: largest (?P<mv>fall|rise) (?P<a>" + DNUM + r") at (?P<ax>.+?) → (?P<b>" + DNUM + r") at (?P<bx>[^;|\n]+?))?"
    r"(?:; (?P<np>\d+) peaks: (?P<peaks>[^;|\n]+))?(?=;|\||$|\n)")
LEADERS = re.compile(r"highest at each of (?P<shared>\d+) shared x: (?P<list>[^;|\n]+)")


def parse_digest(digest):
    """The digest's line / area / col series (not points: a scatter has no path)."""
    out, leaders = [], []
    for layer in re.split(r"\s\|\s|\n", digest or ""):
        geom = re.match(r"(?:statement \d+: )?(\w[\w+]*)(?: \[[^\]]*\])?:", layer)
        if not geom or geom.group(1).startswith("point"):
            continue
        for m in SERIES.finditer(layer):
            d = m.groupdict()
            out.append({"x0": d["x0"], "x1": d["x1"], "minx": d["minx"].strip(), "maxx": d["maxx"].strip(),
                        "key": (d["key"] or "").strip() or None, "first": float(d["first"]), "last": float(d["last"]),
                        "min": float(d["min"]), "max": float(d["max"]), "geom": geom.group(1),
                        "move": (d["mv"], float(d["a"]), d["ax"], float(d["b"]), d["bx"]) if d["mv"] else None,
                        "peaks": int(d["np"] or 1)})
    # per layer, and across facet panels ('line across panels: highest …')
    for m in LEADERS.finditer(digest or ""):
        counts = {}
        for part in m.group("list").split(", "):
            k, _, c = part.rpartition(" ")
            if c.isdigit():
                counts[k] = int(c)
        leaders.append((counts, int(m.group("shared"))))
    return out, leaders


STEADY = re.compile(
    r"\b(?:(?:rises?|rising|grows?|growing|increas\w*|climbs?|climbing|declin\w*|falls?|falling|decreas\w*|drops?|dropping)"
    r"\s+(?:steadily|consistently|continuously|monotonically|without (?:a )?(?:dip|break|pause))"
    r"|steady\s+(?:growth|rise|increase|climb|decline|decrease|fall|upward|downward)"
    r"|(?:steadily|consistently|continuously)\s+(?:rising|growing|increasing|declining|falling|decreasing|upward|downward))\b",
    re.I)
AFTER_PEAK = re.compile(
    r"\b(?:single|one|only one|lone)\s+(?:sharp\s+)?(?:peak|spike)\b"
    r"|\b(?:falls?|drops?|returns?|goes|comes|settles?|clears?)\s+back\s+(?:to|near|down)[^.;]{0,60}?\b(?:rest of|remainder|thereafter|afterwards|after that)\b"
    r"|\b(?:near|close to|around)\s+zero\s+(?:for|during)\s+the\s+rest\b", re.I)
ALWAYS = re.compile(r"\b(?:consistently|always|every (?:month|week|year|day|quarter)|throughout|in (?:every|each) (?:month|week|year|period))\b", re.I)
TOP = re.compile(r"\b(?:highest|lead(?:s|ing)?|top|most|largest|biggest|ahead)\b", re.I)
BOTTOM = re.compile(r"\b(?:lowest|least|smallest|fewest|last|behind)\b", re.I)


def at(x):
    """An x label as a number (a date as days) or None."""
    x = x.strip()
    try:
        return float(x)
    except ValueError:
        pass
    try:
        return datetime.date.fromisoformat(x[:10]).toordinal()
    except ValueError:
        return None


def span(x0, x1, a, b):
    """The share of the x extent between a and b (None for a discrete x)."""
    v = [at(x) for x in (x0, x1, a, b)]
    if None in v or v[1] == v[0]:
        return None
    return abs(v[3] - v[2]) / abs(v[1] - v[0])


def clauses(text):
    """Sentences, and the parts of one split by a dash."""
    return [c for c in re.split(r"(?<=[.;!?])\s+|\s+—\s+", text) if c.strip()]


def named(clause, series):
    """The series a clause names by their key."""
    return [s for s in series if s["key"] and re.search(r"(?<!\w)" + re.escape(s["key"]) + r"(?!\w)", clause, re.I)]


def word_problems(answer, digest):
    """(the phrase, what the digest says instead) for each contradicted word claim."""
    series, leaders = parse_digest(digest)
    if not series:
        return []
    out = []
    for cl in clauses(answer):
        # 1. a steady rise / fall against a sizeable counter move
        if STEADY.search(cl):
            cands = named(cl, series) or (series if len({s["key"] for s in series}) == 1 else [])
            for s in cands:
                mv = s["move"]
                rng = s["max"] - s["min"]
                if mv and rng > 0 and abs(mv[1] - mv[3]) >= 0.1 * rng and (
                        abs(mv[1] - mv[3]) >= 0.3 * rng or (span(s["x0"], s["x1"], mv[2], mv[4]) or 0) >= 0.2):
                    out.append((STEADY.search(cl).group(), f"{s['key'] or 'the series'}: {mv[0]} {mv[1]:g} at {mv[2]} → {mv[3]:g} at {mv[4]}"))
                    break
        # 2. one peak / back to the baseline for good, but another peak stands out
        if AFTER_PEAK.search(cl):
            cands = named(cl, series) or series
            more = [s for s in cands if s["peaks"] > 1]
            # said of a named series, or of most of the chart's series
            if more and (named(cl, series) or 2 * len(more) >= len(cands)):
                out.append((AFTER_PEAK.search(cl).group(), f"{more[0]['key'] or 'the series'} has {more[0]['peaks']} peaks"))
        # 3. 'X is consistently the highest' against who is highest per x
        if ALWAYS.search(cl) and leaders:
            for counts, shared in leaders:
                ks = [k for k in counts if re.search(r"(?<!\w)" + re.escape(k) + r"(?!\w)", cl, re.I)]
                if len(ks) != 1 or not TOP.search(cl) or BOTTOM.search(cl):
                    continue
                k = ks[0]
                if counts.get(k, 0) < shared:
                    out.append((ALWAYS.search(cl).group(), f"{k} is highest at {counts.get(k, 0)} of {shared} x"))
                    break
    return out


def latest_digest(evidence):
    """The digest of the last chart drawn so far (the one shown with the answer)."""
    for e in reversed(evidence):
        if e and "what the chart shows" in e:
            return e
    return ""


def word_feedback(bad):
    said = "; ".join(f"'{p}' — but {why}" for p, why in bad)
    return ("Not accepted yet: your answer's wording contradicts what the chart drew "
            f"({said}). Reword it to match the chart, or check the data, and call answer again.")
