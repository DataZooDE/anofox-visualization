#!/usr/bin/env python3
"""Skill-SQL validation harness for the anofox-visualization Claude Code plugin.

Extracts every fenced sql block from the plugin's skills
(plugins/anofox-visualization/skills/*/SKILL.md; with --docs also
docs/DOCS.md, whose blocks are still fragments over tables they do not
create, so it is opt-in and not part of CI) and checks each block on its own, in a fresh DuckDB process,
against the locally built C-API extension (the full toolkit build):

    ```sql             executed with the DuckDB CLI: `.bail on`, then
                       `LOAD anofox_visualization;`, then the block. Any SQL
                       error fails it, and so does an SVG that carries render
                       warnings (`data-warnings`: the engine dropped data).
    ```sql dashboard   a ::ROLE dashboard script: written to a file and linted
                       with `dashboard --check --json --strict`. Any
                       diagnostic fails it: errors, warnings (render-warning,
                       empty-panel, unknown-cast) and design/* advisories.
    ```sql skip        not run (templates with <placeholders>, statements
                       that start servers, MotherDuck, …).

Blocks are self-contained: each one creates the data it needs. A block that
contains `LOAD anofox_statistics` / `LOAD anofox_forecast` gets that
extension (installed once per run from https://get.erpl.io / the community
repository, which needs network access and DuckDB v1.5.6).

The harness uses a private extension directory under --workdir, so it never
touches ~/.duckdb: the built anofox_visualization is installed there from
--ext (so blocks can say `LOAD anofox_visualization;` like a user would).
`dashboard --check` runs the `duckdb` on PATH; the harness puts a wrapper
first on PATH that runs the given CLI with `-unsigned` and that extension
directory.

Usage
-----
    SKIP_WASM=1 DUCKDB=duckdb OUT=/path/anofox_visualization.duckdb_extension \\
        duckext/scripts/build-native.sh --release
    cargo build --release --bin dashboard
    python3 scripts/validate_skills_sql.py --duckdb duckdb \\
        --ext /path/anofox_visualization.duckdb_extension --dashboard target/release/dashboard
    python3 scripts/validate_skills_sql.py ... --file plugins/anofox-visualization/skills/anofox-visualization-charts/SKILL.md
"""

import argparse
import concurrent.futures as cf
import glob
import json
import os
os.environ["DATAZOO_DISABLE_TELEMETRY"] = "1"  # no usage telemetry from anofox extensions, whatever was inherited
import re
import shlex
import stat
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SKILL_GLOB = "plugins/anofox-visualization/skills/*/SKILL.md"
DOCS = ["docs/DOCS.md"]

# ```sql[ tags]\n body \n``` — tags: "skip", "dashboard".
BLOCK_RE = re.compile(r"^```sql([^\n]*)\n(.*?)^```[ \t]*$", re.MULTILINE | re.DOTALL)
TAGS = {"", "skip", "dashboard"}

NEEDS = {
    "anofox_statistics": "INSTALL anofox_statistics FROM 'https://get.erpl.io';",
    "anofox_forecast": "INSTALL anofox_forecast FROM community;",
}


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--duckdb", default=os.environ.get("DUCKDB", "duckdb"), help="DuckDB CLI (v1.5.6)")
    p.add_argument("--ext", required=True, help="the built C-API anofox_visualization.duckdb_extension")
    p.add_argument(
        "--dashboard",
        default=None,
        help="the dashboard binary (default: $CARGO_TARGET_DIR or target/, release then debug)",
    )
    p.add_argument("--file", action="append", help="only these Markdown files (repo-relative)")
    p.add_argument("--docs", action="store_true", help="also validate docs/DOCS.md")
    p.add_argument("--workdir", default=None, help="scratch directory (default: a new temp dir)")
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("-v", "--verbose", action="store_true", help="print every block's result")
    return p.parse_args()


def find_dashboard(arg: str | None) -> str:
    if arg:
        return os.path.abspath(arg)
    roots = [os.environ.get("CARGO_TARGET_DIR"), os.path.join(REPO, "target")]
    for root in filter(None, roots):
        for prof in ("release", "debug"):
            path = os.path.join(root, prof, "dashboard")
            if os.access(path, os.X_OK):
                return path
    sys.exit("ERROR: dashboard binary not found; build it (cargo build --release --bin dashboard) or pass --dashboard")


def in_repo(raw: str) -> str:
    """Resolve a --file argument; refuse paths outside the repository."""
    if ".." in raw.replace("\\", "/").split("/"):
        sys.exit(f"ERROR: --file must not contain '..': {raw!r}")
    path = os.path.realpath(raw if os.path.isabs(raw) else os.path.join(REPO, raw))
    root = os.path.realpath(REPO)
    if not path.startswith(root + os.sep):
        sys.exit(f"ERROR: --file resolves outside the repository: {path!r}")
    return path


def blocks(path: str):
    """(tag, body, line) for every sql fence of a Markdown file."""
    text = open(path, encoding="utf-8").read()
    out = []
    for m in BLOCK_RE.finditer(text):
        tag = m.group(1).strip()
        line = text[: m.start()].count("\n") + 1
        if tag not in TAGS:
            sys.exit(f"ERROR: {os.path.relpath(path, REPO)}:{line}: unknown sql fence tag {tag!r} (use {sorted(TAGS)})")
        out.append((tag, m.group(2), line))
    return out


class Env:
    def __init__(self, a: argparse.Namespace):
        self.duckdb = a.duckdb
        self.dashboard = find_dashboard(a.dashboard)
        self.work = os.path.abspath(a.workdir or tempfile.mkdtemp(prefix="skills-sql-"))
        os.makedirs(self.work, exist_ok=True)
        self.extdir = os.path.join(self.work, "extensions")
        os.makedirs(self.extdir, exist_ok=True)
        self.ext = os.path.abspath(a.ext)
        # `duckdb` wrapper for `dashboard --check` (it runs the duckdb on PATH).
        self.bindir = os.path.join(self.work, "bin")
        os.makedirs(self.bindir, exist_ok=True)
        wrapper = os.path.join(self.bindir, "duckdb")
        cli = shlex.quote(self._resolve(self.duckdb))
        setx = shlex.quote(f"SET extension_directory = '{self.extdir}'")
        with open(wrapper, "w") as f:
            f.write(f'#!/bin/sh\nexec {cli} -unsigned -cmd {setx} "$@"\n')
        os.chmod(wrapper, os.stat(wrapper).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    @staticmethod
    def _resolve(cmd: str) -> str:
        if os.sep in cmd:
            return os.path.abspath(cmd)
        for d in os.environ.get("PATH", "").split(os.pathsep):
            p = os.path.join(d, cmd)
            if os.access(p, os.X_OK):
                return p
        sys.exit(f"ERROR: DuckDB CLI not found: {cmd}")

    def duck(self, sql: str, cwd: str) -> subprocess.CompletedProcess:
        fd, path = tempfile.mkstemp(suffix=".sql", dir=cwd)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(sql)
        try:
            return subprocess.run(
                [self.duckdb, "-unsigned", "-cmd", f"SET extension_directory = '{self.extdir}'", "-f", path],
                capture_output=True, text=True, cwd=cwd, timeout=600,
            )
        finally:
            os.unlink(path)

    def install(self, needs: set[str]) -> None:
        stmts = [f"FORCE INSTALL '{self.ext}';"] + [NEEDS[n] for n in sorted(needs)]
        for s in stmts:
            for attempt in range(3):
                r = self.duck(".bail on\n" + s + "\n", self.work)
                if r.returncode == 0:
                    break
            else:
                sys.exit(f"ERROR: {s} failed:\n{r.stderr}")
        print(f"installed into {self.extdir}: anofox_visualization" + "".join(f", {n}" for n in sorted(needs)))


def run_block(env: Env, idx: int, tag: str, body: str) -> tuple[bool, str]:
    cwd = os.path.join(env.work, f"b{idx:04d}")
    os.makedirs(cwd, exist_ok=True)
    if tag == "dashboard":
        path = os.path.join(cwd, "dashboard.sql")
        with open(path, "w", encoding="utf-8") as f:
            f.write(body)
        r = subprocess.run(
            [env.dashboard, "--check", path, "--json", "--strict"],
            capture_output=True, text=True, cwd=cwd, timeout=900,
            env={**os.environ, "PATH": env.bindir + os.pathsep + os.environ.get("PATH", "")},
        )
        try:
            res = json.loads(r.stdout.strip().splitlines()[-1])
        except (ValueError, IndexError):
            return False, f"dashboard --check gave no JSON (exit {r.returncode}):\n{r.stdout}{r.stderr}"
        bad = res["diagnostics"]  # every diagnostic fails: errors, warnings, design/*
        if res["ok"] and not bad:
            return True, f"{len(res['diagnostics'])} diagnostic(s)"
        return False, "\n".join(f"[stmt {d['stmt']}] {d['code']}: {d['message']}\n    {d['sql'][:300]}" for d in bad) \
            or f"dashboard --check failed:\n{r.stdout}{r.stderr}"
    # list mode prints every SVG in full, so render warnings are visible.
    r = env.duck(".bail on\n.mode list\nLOAD anofox_visualization;\n" + body, cwd)
    if r.returncode != 0 or "Error:" in r.stderr:
        return False, (r.stderr or r.stdout).strip()
    warned = re.findall(r'data-warnings="([^"]*)"', r.stdout)
    if warned:
        return False, "render warning(s): " + "; ".join(w.replace("&quot;", '"') for w in warned)
    return True, ""


def main() -> int:
    a = parse_args()
    ext = os.path.abspath(a.ext)
    if not re.fullmatch(r"anofox_visualization(\..+)?\.duckdb_extension", os.path.basename(ext)):
        print(f"--ext {ext}: LOAD takes the entry point from the file name, so it must be named "
              "anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). "
              f"Rename it, or: ln -s '{ext}' /tmp/anofox_visualization.duckdb_extension", file=sys.stderr)
        return 2
    if a.file:
        files = [in_repo(f) for f in a.file]
    else:
        files = sorted(glob.glob(os.path.join(REPO, SKILL_GLOB)))
        if a.docs:
            files += [os.path.join(REPO, d) for d in DOCS]
    if not files:
        sys.exit("ERROR: no files to validate")
    env = Env(a)

    jobs, skipped = [], 0
    for f in files:
        for tag, body, line in blocks(f):
            if tag == "skip":
                skipped += 1
                continue
            jobs.append((os.path.relpath(f, REPO), line, tag, body))
    env.install({n for *_, body in jobs for n in NEEDS if re.search(rf"\bLOAD\s+{n}\b", body, re.I)})

    results = {}
    with cf.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        futs = {pool.submit(run_block, env, i, tag, body): i for i, (_, _, tag, body) in enumerate(jobs)}
        for fut in cf.as_completed(futs):
            results[futs[fut]] = fut.result()

    counts = {"": 0, "dashboard": 0}
    failed = 0
    for i, (rel, line, tag, _) in enumerate(jobs):
        ok, msg = results[i]
        counts[tag] += 1
        kind = "dashboard" if tag else "sql"
        if not ok:
            failed += 1
            print(f"FAIL {rel}:{line} ({kind})")
            for ln in msg.splitlines():
                print(f"     {ln}")
        elif a.verbose:
            print(f"PASS {rel}:{line} ({kind}) {msg}")
    print(
        f"\n{len(files)} file(s): executed {counts['']} sql block(s), "
        f"dashboard-checked {counts['dashboard']}, skipped {skipped}, failed {failed}"
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
