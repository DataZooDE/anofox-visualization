#!/usr/bin/env python3
"""render.py <script.sql> <out_dir> [extension] — run a SQL script whose
queries return (name, svg) with the anofox_visualization extension loaded and
write <out_dir>/<name>.svg. Uses the `duckdb` CLI (env DUCKDB) with -unsigned;
the extension path defaults to env ANOFOX_VIZ_EXT or the installed one."""
import json, os, subprocess, sys

script, out = sys.argv[1], sys.argv[2]
ext = sys.argv[3] if len(sys.argv) > 3 else os.environ.get("ANOFOX_VIZ_EXT", "anofox_visualization")
duckdb = os.environ.get("DUCKDB", "duckdb")
os.makedirs(out, exist_ok=True)
load = f"LOAD '{ext}';" if ext.endswith(".duckdb_extension") else f"LOAD {ext};"
sql = load + "\n" + open(script).read()
res = subprocess.run([duckdb, "-unsigned", "-jsonlines", "-c", sql], capture_output=True, text=True)
if res.returncode != 0:
    sys.exit(res.stderr)
n = 0
for line in res.stdout.splitlines():
    line = line.strip()
    if not line.startswith("{"):
        continue
    row = json.loads(line)
    if "name" in row and "svg" in row and row["svg"]:
        with open(os.path.join(out, row["name"] + ".svg"), "w") as f:
            f.write(row["svg"])
        n += 1
print(f"wrote {n} SVGs to {out}")
