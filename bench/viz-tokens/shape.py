"""#1263: a series' shape and who leads per x, as ggplot-rs (≥ 0.60) writes
them in its plot summary — one source for every arm's digest: harness.py
imports it for the Vega-Lite digest, and MPL_DIGEST (the python arm's
sandboxed script, which can't import from here) carries this file's text.
Keep it self-contained.
"""

def shape_suffix(pts, fmt):
    ys = [y for _, y in pts]
    n = len(ys)
    if n < 4:
        return ""
    rng = max(ys) - min(ys)
    if rng <= 0:
        return ""
    up = ys[-1] >= ys[0]
    best, ext, at, mv = 0.0, ys[0], 0, None
    for j, y in enumerate(ys):
        d = ext - y if up else y - ext
        if d > best:
            best, mv = d, (at, j)
        if (up and y > ext) or (not up and y < ext):
            ext, at = y, j
    out = ""
    if mv and best >= 0.1 * rng:
        a, b = mv
        word = "fall" if ys[b] < ys[a] else "rise"
        out += f"; against the trend: largest {word} {fmt(ys[a])} at {pts[a][0]} → {fmt(ys[b])} at {pts[b][0]}"
    peaks = []
    if n <= 5000:
        for i, y in enumerate(ys):
            if not ((i == 0 or ys[i - 1] < y) and (i + 1 == n or ys[i + 1] <= y)) or (i == 0 and ys[1] == y):
                continue
            def side(ks):
                m = y
                for k in ks:
                    if ys[k] > y:
                        return m
                    m = min(m, ys[k])
                return m
            left, right = side(range(i - 1, -1, -1)), side(range(i + 1, n))
            base = right if i == 0 else left if i + 1 == n else max(left, right)
            if y - base >= 0.25 * rng:
                peaks.append(pts[i])
        peaks.sort(key=lambda p: -p[1])
    if len(peaks) > 1:
        out += f"; {len(peaks)} peaks: " + ", ".join(f"{fmt(y)} at {x}" for x, y in peaks[:3]) + (", …" if len(peaks) > 3 else "")
    return out


def leaders_suffix(keyed):
    if len(keyed) < 2 or any(k is None for k, _ in keyed):
        return ""
    maps = [dict(p) for _, p in keyed]
    count, shared = [0] * len(keyed), 0
    for x, _ in keyed[0][1]:
        if not all(x in m for m in maps):
            continue
        shared += 1
        ys = [m[x] for m in maps]
        top = max(ys)
        for i, y in enumerate(ys):
            count[i] += y == top
    if shared < 3:
        return ""
    top = sorted([(k, c) for (k, _), c in zip(keyed, count) if c], key=lambda t: -t[1])
    return f"; highest at each of {shared} shared x: " + ", ".join(f"{k} {c}" for k, c in top)
