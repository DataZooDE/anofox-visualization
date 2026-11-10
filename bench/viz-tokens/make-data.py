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
# the coverage set's own datasets (coverage.jsonl), from web/data
COVERAGE = {
    "geo_de": """
        CREATE TABLE districts AS FROM read_csv('data/delivery-coverage/districts.csv', types={'ags': 'VARCHAR'});
        CREATE TABLE customers AS FROM read_csv('data/delivery-coverage/customers.csv', types={'ags': 'VARCHAR'});
        CREATE TABLE dcs AS FROM read_csv('data/delivery-coverage/dcs.csv');
        CREATE TABLE deliveries AS FROM read_csv('data/delivery-coverage/deliveries.csv', types={'ags': 'VARCHAR'});""",
    "world": "CREATE TABLE world_population AS FROM read_csv('data/maps/world-population.csv');",
    "states": "CREATE TABLE states AS FROM read_csv('data/drill-revenue-map/states.csv');",
    "replenish": """
        CREATE TABLE ir_sales AS FROM read_csv('data/optimize-replenishment/ir_sales.csv');
        CREATE TABLE ir_forecast AS FROM read_csv('data/optimize-replenishment/ir_forecast.csv');
        CREATE TABLE ir_skus AS FROM read_csv('data/optimize-replenishment/ir_skus.csv');""",
    # one row per lead: the CSV scores every lead with two models (#1044)
    "leads": "CREATE TABLE leads AS SELECT * EXCLUDE (message, model, team_pred) FROM read_csv('data/decide-leads/ld_scored.csv') WHERE model = 'laya-typed-decisions';",
}
for ds in ["sales", "supply_chain", "lead_times", "claims", "m5"]:
    p = out / f"{ds}.duckdb"
    p.unlink(missing_ok=True)
    c = duckdb.connect(str(p))
    c.execute((ROOT / "tests/assist-eval/datasets" / f"{ds}.sql").read_text())
    if ds == "lead_times":  # disruptions and items for the coverage set
        c.execute("CREATE TABLE disruptions AS FROM read_csv('data/supplier-lead-times/disruptions.csv');"
                  "CREATE TABLE items AS FROM read_csv('data/supplier-lead-times/items.csv');")
    c.close()
    print(p)
for ds, sql in COVERAGE.items():
    p = out / f"{ds}.duckdb"
    p.unlink(missing_ok=True)
    c = duckdb.connect(str(p))
    c.execute(sql)
    c.close()
    print(p)
