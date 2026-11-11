"""#1116: the claim check — every number an answer states must be backed by
what the conversation's tools returned (query results, chart digests, the
model's own SQL, python's stdout) or the question, within the number's own
rounding: directly, or as a difference, ratio, share or % change of two
numbers from the same tool output. Deterministic, no LLM.

    unsupported(answer, evidence)  -> the claims (as written) nothing backs
"""
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
