#!/usr/bin/env python3
"""Shared parts of the conformance harnesses (scripts/*_conformance.py, #186).

Each harness keeps its discovery, recipes and extra cases; this module is the
rest: one fresh `duckdb -unsigned` process per case (`run_duckdb`), the SVG
cells of a `-json` result (`svgs_of`), the verdict on a plot call (`check`,
`check_raw`, `known_bug`), the thread pool that runs the cases and writes
every SVG (`run_all`, `svg_names`), the PNG contact sheet (`montage`,
`write_montage`) and the common arguments (`parser`, `ext_path`).

Importing it switches off the anofox extensions' usage telemetry for every
process a harness starts (DATAZOO_DISABLE_TELEMETRY=1, forced: an inherited 0
must not re-enable it).
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import html
import json
import os
import re
import shutil
import subprocess
import sys
from typing import Callable

os.environ["DATAZOO_DISABLE_TELEMETRY"] = "1"

#: The error anofox_plot gives a table that follows no contract schema.
NO_SCHEMA = "match no plottable schema"


def run_duckdb(duckdb: str, script: str, mode: str = "-list", timeout: int = 600,
               quiet: tuple[str, ...] = ()) -> tuple[bool, str]:
    """`script` in a fresh `duckdb -unsigned -noheader <mode>` process → (ok, stdout + stderr).
    ok = exit 0 and no stderr; stderr lines starting with a `quiet` prefix (extension banners) are dropped."""
    p = subprocess.run([duckdb, "-unsigned", "-noheader", mode, "-c", script],
                       capture_output=True, text=True, timeout=timeout)
    err = p.stderr or ""
    if quiet:
        err = "\n".join(l for l in err.splitlines() if not l.startswith(quiet))
    return p.returncode == 0 and not err.strip(), (p.stdout or "") + err


def svgs_of(out: str) -> list[str]:
    """Every SVG cell of the `-json` results (grids nest <svg> elements)."""
    dec, i, cells = json.JSONDecoder(), 0, []
    while (j := out.find("[", i)) >= 0:
        try:
            rows, i = dec.raw_decode(out, j)
        except ValueError:
            i = j + 1
            continue
        cells += [v for r in rows if isinstance(r, dict) for v in r.values()
                  if isinstance(v, str) and v.startswith("<svg")]
    return cells


def render_warnings(svgs: list[str]) -> list[str]:
    """The `data-warnings` of every SVG."""
    out: list[str] = []
    for s in svgs:
        w = re.search(r'data-warnings="([^"]*)"', s)
        if w:
            out += json.loads(html.unescape(w.group(1)))
    return out


def check(ok: bool, out: str, allowed: list[str] | tuple[str, ...] = (),
          no_data: bool = False) -> tuple[str, list[str], str]:
    """FAIL on a non-zero exit, stderr or no SVG (with `no_data`, also on the
    'No data' note: the producer returned no rows); WARN on a render warning
    that matches nothing in `allowed`; else OK."""
    svgs = svgs_of(out) if ok else []
    if not svgs:
        return "FAIL", [], out.strip().replace("\n", " ")[:400]
    if no_data and any(">No data<" in s for s in svgs):
        return "FAIL", svgs, "the producer returned no rows (the plot is the 'No data' note)"
    bad = [w for w in render_warnings(svgs) if not any(a in w for a in allowed)]
    if bad:
        return "WARN", svgs, "; ".join(bad)[:400]
    return "OK", svgs, ""


def check_raw(ok: bool, out: str, allowed: list[str] | tuple[str, ...] = ()) -> tuple[str, list[str], str]:
    """A raw producer output: no schema is the expected answer, a render an XPASS."""
    if not ok and NO_SCHEMA in out:
        return "NOSCHEMA", [], ""
    status, svgs, msg = check(ok, out, allowed)
    return ("XPASS" if status == "OK" else status), svgs, msg or "raw output renders: tag it and add a contract case"


def known_bug(status: str, msg: str, reason: str) -> tuple[str, str]:
    """A case listed as a known bug: XFAIL while it fails, XPASS once it passes."""
    return ("XFAIL", reason) if status != "OK" else ("XPASS", "drop it from KNOWN")


def run_all(runs: list[dict], one: Callable, jobs: int, out_dir: str,
            key: Callable[[dict], str] = lambda c: f"{c['name']}.{c['which']}",
            post: Callable[[dict, str, str], tuple[str, str]] | None = None) -> list[tuple]:
    """`one(case) -> (case, (status, svgs, msg))` over `runs` on `jobs` threads; every
    SVG goes to `out_dir/<key>[.<k>].svg`. Returns (key, status, msg, n_svgs, case)
    sorted by key; `post(case, status, msg)` may rewrite the verdict."""
    results = []
    with cf.ThreadPoolExecutor(jobs) as pool:
        for c, (status, svgs, msg) in pool.map(one, runs):
            k = key(c)
            for i, s in enumerate(svgs):
                fn = k if len(svgs) == 1 else f"{k}.{i}"
                with open(os.path.join(out_dir, fn + ".svg"), "w") as f:
                    f.write(s)
            if post:
                status, msg = post(c, status, msg)
            results.append((k, status, msg, len(svgs), c))
    return sorted(results, key=lambda r: r[0])


def svg_names(results: list[tuple]) -> list[str]:
    """The SVG file names (without .svg) run_all wrote, in result order."""
    return [n for r in results for n in ([r[0]] if r[3] == 1 else [f"{r[0]}.{k}" for k in range(r[3])])]


def montage(out_dir: str, names: list[str], cols: int = 6) -> str | None:
    if not shutil.which("rsvg-convert"):
        return None
    try:
        from PIL import Image, ImageDraw
    except ImportError:
        return None
    pngs = []
    png_dir = os.path.join(out_dir, "png")
    os.makedirs(png_dir, exist_ok=True)
    for n in names:
        svg = os.path.join(out_dir, n + ".svg")
        png = os.path.join(png_dir, n + ".png")
        r = subprocess.run(["rsvg-convert", "-b", "white", "-w", "480", svg, "-o", png],
                           capture_output=True)
        if r.returncode == 0:
            pngs.append((n, png))
    if not pngs:
        return None
    tw, th = 480, 330
    rows = (len(pngs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * tw, rows * (th + 18)), "white")
    draw = ImageDraw.Draw(sheet)
    for k, (n, png) in enumerate(pngs):
        im = Image.open(png).convert("RGB")
        im.thumbnail((tw, th))
        x, y = (k % cols) * tw, (k // cols) * (th + 18)
        sheet.paste(im, (x, y + 18))
        draw.text((x + 4, y + 2), n, fill=(180, 30, 30))
    path = os.path.join(out_dir, "montage.png")
    sheet.save(path)
    return path


def write_montage(out_dir: str, names: list[str], keep: Callable[[str], bool], cols: int = 6) -> None:
    names = [n for n in names if keep(n) and os.path.exists(os.path.join(out_dir, n + ".svg"))]
    m = montage(out_dir, names, cols)
    if m:
        print(f"montage: {m}")


def parser(doc: str | None, out: str, extension_directory: str | None = None) -> argparse.ArgumentParser:
    """--duckdb, --ext, --out, [--extension-directory], --jobs, --only, --no-montage."""
    ap = argparse.ArgumentParser(description=doc, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--duckdb", default=os.environ.get("DUCKDB", "duckdb"))
    ap.add_argument("--ext", required=True, help="anofox_visualization extension file to LOAD")
    ap.add_argument("--out", default=out)
    if extension_directory:
        ap.add_argument("--extension-directory", help=extension_directory)
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--only", help="regex: run only matching case names")
    ap.add_argument("--no-montage", action="store_true")
    return ap


def ext_path(ext: str) -> str | None:
    """The absolute extension path, or None (after a message) when LOAD would miss its entry point."""
    p = os.path.abspath(ext)
    if not re.fullmatch(r"anofox_visualization(\..+)?\.duckdb_extension", os.path.basename(p)):
        print(f"--ext {p}: LOAD takes the entry point from the file name, so it must be named "
              "anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). "
              f"Rename it, or: ln -s '{p}' /tmp/anofox_visualization.duckdb_extension", file=sys.stderr)
        return None
    return p
