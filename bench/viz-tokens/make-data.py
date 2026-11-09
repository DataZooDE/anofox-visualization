#!/usr/bin/env python3
"""#1044: one read-only DuckDB file per dataset (tests/assist-eval/datasets),
the same file for every arm.  python make-data.py <out-dir>"""
import os
import sys
from pathlib import Path

import duckdb

ROOT = Path(__file__).resolve().parents[2]
out = Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
os.chdir(ROOT / "web")  # the datasets read data/… relative to web/
for ds in ["sales", "supply_chain", "lead_times", "claims", "m5"]:
    p = out / f"{ds}.duckdb"
    p.unlink(missing_ok=True)
    c = duckdb.connect(str(p))
    c.execute((ROOT / "tests/assist-eval/datasets" / f"{ds}.sql").read_text())
    c.close()
    print(p)
