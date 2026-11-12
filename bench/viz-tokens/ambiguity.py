"""#1142: how ambiguous a task is, from the results of its readings.

A task may list several readings of its question (lead time to the first or
the last receipt; the best channel by revenue or by units), each with its own
truth SQL. Their results decide the label, per phase:

- clear: one reading;
- harmless: the readings agree on the conclusion (same rows in the same
  order, every number within REL_TOL) — answer directly, saying the reading;
- ambiguous: they don't (another ranking, another winner, numbers further
  apart) — the right move is to ask the user back first.
"""
import math

REL_TOL = 0.10  # numbers further apart than this change what an answer says


def _num(v):
    if isinstance(v, bool) or v is None:
        return None
    try:
        f = float(v)
    except (TypeError, ValueError):
        return None
    return f if math.isfinite(f) else None


def diverge(a, b, rel_tol=REL_TOL):
    """Whether two result sets (lists of row tuples) lead to different answers."""
    if len(a) != len(b):
        return True
    for ra, rb in zip(a, b):
        if len(ra) != len(rb):
            return True
        for x, y in zip(ra, rb):
            nx, ny = _num(x), _num(y)
            if nx is None or ny is None:
                # a key (name, date): another row here is another ranking
                if str(x) != str(y):
                    return True
                continue
            scale = max(abs(nx), abs(ny))
            if scale > 0 and abs(nx - ny) / scale > rel_tol:
                return True
    return False


def label(results, rel_tol=REL_TOL):
    """clear / harmless / ambiguous for the result sets of a task's readings
    (the default reading first)."""
    if len(results) < 2:
        return "clear"
    first = results[0]
    return "ambiguous" if any(diverge(first, r, rel_tol) for r in results[1:]) else "harmless"
