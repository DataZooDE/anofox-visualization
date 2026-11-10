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
    # #1077: German column names with spaces / umlauts / €, NULL months,
    # category spellings that differ only in case and spaces — data as a
    # business hands it over
    "messy_de": """
        CREATE TABLE umsatz AS
        SELECT DATE '2025-01-01' + INTERVAL (m) MONTH AS "Monat",
               f AS "Filiale",
               CASE (m + i) % 4 WHEN 0 THEN 'Getränke' WHEN 1 THEN 'getränke ' WHEN 2 THEN 'Snacks' ELSE 'Frische' END AS "Warengruppe",
               CASE WHEN (f = 'Münster' AND m IN (3, 4)) OR (f = 'Köln' AND m = 7) THEN NULL
                    ELSE round(1000 + 37 * m + 113 * i + 17 * ((m * 7 + i * 3) % 11), 2) END AS "Umsatz €",
               10 + (m * 3 + i * 5) % 17 AS "Stück"
        FROM range(12) r(m), (VALUES (0, 'Köln'), (1, 'Düsseldorf'), (2, 'Münster'), (3, 'Würzburg')) t(i, f);""",
    # #1077: a long tail — 1,500 products, 60,000 order lines, the top 10 % about half
    "big": """
        CREATE TABLE products AS SELECT i AS product_id, 'P-' || lpad(i::VARCHAR, 4, '0') AS product,
               ['Tools', 'Garden', 'Kitchen', 'Toys', 'Office'][1 + i % 5] AS category FROM range(1, 1501) r(i);
        CREATE TABLE order_lines AS SELECT o AS line_id, 1 + floor(1499 * pow(((o * 7919) % 10007) / 10007.0, 3))::INT AS product_id,
               1 + o % 9 AS qty, DATE '2025-01-01' + INTERVAL (o % 365) DAY AS day FROM range(60000) r(o);""",
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
