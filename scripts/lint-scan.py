#!/usr/bin/env python3
"""#1133: lint every shipped dashboard the way it really runs, and fail on a
lint code that fires where it isn't accepted.

A false-positive scan is only as good as the panels that reach the checks: a
panel whose SQL errors (a data file read from the wrong folder, an extension
that isn't loaded, a served folder's setup that never ran) is never linted.
So each folder is checked from the directory its relative paths assume, a
served folder against a database built from its own init script, and any
`sql-error` fails the scan as an incomplete one.

  python3 scripts/lint-scan.py [--dashboard target/release/dashboard]
                               [--duckdb duckdb] [--unsigned] [--json out.json]

The `duckdb` CLI must be 1.5.6 with anofox_forecast and anofox_statistics
installed (signed, from community):
  duckdb -c "INSTALL anofox_forecast FROM community; INSTALL anofox_statistics FROM community"
--unsigned starts it -unsigned, for locally installed unsigned builds.

Accepted hits live in scripts/lint-scan-accepted.json: per code, the file
globs it may fire on and why. Exit 1 when a hit is not accepted (a new false
positive, or a real finding to fix) or an accepted entry no longer fires.
Not covered: examples/served (its own `serve.sh check` builds the data with
generate.sh) and tests/ (lint fixtures that fire on purpose).
"""
import argparse
import collections
import fnmatch
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# path: a .sql file or a folder (checked as a served folder). cwd: the folder
# its relative data paths resolve from. init: a setup script run read-write
# into a fresh database (from init_cwd) that the check then opens.
TARGETS = [
    {"path": "web/examples", "cwd": "web"},
    {"path": "examples/drill-revenue-map", "cwd": "web"},
    {"path": "website/showcase"},
    {"path": "website/showcase/gallery"},
    {"path": "examples/dashboards"},
    {"path": "dashboards"},
    {"path": "examples/theming"},
    {"path": "examples/regression/regression-dashboard.sql"},
    {"path": "examples/sp500-dashboard/dash", "init": "init.sql", "init_cwd": "examples/sp500-dashboard"},
    {"path": "examples/sp500-dashboard/single", "init": "init.sql", "init_cwd": "examples/sp500-dashboard"},
    {"path": "examples/serve-dashboards/dash", "init": "examples/serve-dashboards/init.sql"},
    {"path": "examples/load-test/dash", "init": "examples/load-test/setup.sql"},
]


def duckdb_wrapper(tmp, duckdb, unsigned):
    """A `duckdb` on PATH for the dashboard CLI: the chosen binary, -unsigned
    when asked."""
    d = os.path.join(tmp, "bin")
    os.makedirs(d, exist_ok=True)
    w = os.path.join(d, "duckdb")
    real = subprocess.run(["sh", "-c", f'command -v "{duckdb}"'], capture_output=True, text=True).stdout.strip()
    if not real:
        sys.exit(f"duckdb not found: {duckdb}")
    with open(w, "w") as f:
        f.write(f'#!/bin/sh\nexec "{real}"{" -unsigned" if unsigned else ""} "$@"\n')
    os.chmod(w, 0o755)
    return d


def check(target, dashboard, env, tmp):
    """The --check --json result of one target, with paths relative to ROOT."""
    cwd = os.path.join(ROOT, target.get("cwd", "."))
    src = os.path.join(ROOT, target["path"])
    args = [dashboard, "--check", os.path.relpath(src, cwd), "--json"]
    if "init" in target:
        db = os.path.join(tmp, target["path"].replace("/", "_") + ".duckdb")
        init_cwd = os.path.join(ROOT, target.get("init_cwd", "."))
        r = subprocess.run(["duckdb", db, "-c", f".read {target['init']}"], cwd=init_cwd, env=env,
                           capture_output=True, text=True)
        if r.returncode != 0:
            return [{"file": target["path"], "code": "scan/init-failed", "severity": "error",
                     "message": (r.stderr or r.stdout).strip()[:300]}]
        args += ["--db", db]
    r = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True)
    try:
        out = json.loads(r.stdout)
    except json.JSONDecodeError:
        return [{"file": target["path"], "code": "scan/no-json", "severity": "error",
                 "message": (r.stderr or r.stdout).strip()[:300]}]
    files = out["files"] if "files" in out else [dict(out, file=os.path.basename(target["path"]))]
    folder = target["path"] if "files" in out else os.path.dirname(target["path"])
    hits = []
    for f in files:
        for d in f.get("diagnostics", []):
            hits.append({"file": f"{folder}/{f['file']}", "code": d.get("code"), "severity": d.get("severity"),
                         "message": d.get("message", "")})
    return hits


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dashboard", default=os.path.join(ROOT, "target/release/dashboard"))
    ap.add_argument("--duckdb", default="duckdb")
    ap.add_argument("--unsigned", action="store_true")
    ap.add_argument("--accepted", default=os.path.join(ROOT, "scripts/lint-scan-accepted.json"))
    ap.add_argument("--json", help="write every hit here")
    a = ap.parse_args()
    dashboard = os.path.abspath(a.dashboard)
    with open(a.accepted) as f:
        accepted = {k: v for k, v in json.load(f).items() if not k.startswith("_")}

    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, PATH=duckdb_wrapper(tmp, a.duckdb, a.unsigned) + os.pathsep + os.environ["PATH"])
        hits = [h for t in TARGETS for h in check(t, dashboard, env, tmp)]
    if a.json:
        with open(a.json, "w") as f:
            json.dump(hits, f, indent=1)

    def ok(h):
        return any(fnmatch.fnmatch(h["file"], g) for g in accepted.get(h["code"], {}).get("files", []))

    by_code = collections.defaultdict(list)
    for h in hits:
        by_code[h["code"]].append(h)
    print(f"{len(hits)} hits over {len({h['file'] for h in hits})} files")
    print(f"{'code':34} {'hits':>5} {'files':>5}  accepted")
    for code, hs in sorted(by_code.items(), key=lambda kv: -len(kv[1])):
        n_ok = sum(ok(h) for h in hs)
        print(f"{code:34} {len(hs):5} {len({h['file'] for h in hs}):5}  {n_ok}/{len(hs)}")

    bad = [h for h in hits if not ok(h)]
    for h in bad:
        print(f"NOT ACCEPTED {h['severity']} {h['code']} {h['file']}: {h['message'][:200]}")
    stale = [(c, g) for c, v in accepted.items() for g in v.get("files", [])
             if not any(h["code"] == c and fnmatch.fnmatch(h["file"], g) for h in hits)]
    for c, g in stale:
        print(f"STALE accepted entry: {c} on {g} no longer fires — remove it")
    sys.exit(1 if bad or stale else 0)


if __name__ == "__main__":
    main()
