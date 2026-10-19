-- "Is today's supplier feed safe to load?" — the native computation behind
-- the stored results.
--
-- Runs natively only: anofox_tabular has no DuckDB-Wasm build yet, so the
-- browser demo reads the tables this script produces from
-- web/data/tabular-feed/ (regenerate: scripts/gen-tabular-cases.sh feed).
-- Run from the repo root:
--   duckdb -c "INSTALL anofox_tabular FROM community"   # once (signed; replaces a get.erpl.io install with FORCE INSTALL)
--   duckdb -c ".read examples/tabular-feed/compute.sql"
LOAD anofox_tabular;
.read examples/tabular-feed/data.sql

-- ── 1. The data contract, as a check suite run per delivery ────────────────
-- identifier_column = delivery_date gives one result row per nightly file.
-- monitor_only marks the warnings: they are reported ('warn') but do not
-- stop the load. The category column is ours, for the dashboard.
CREATE OR REPLACE TABLE fm_contract (
    check_name VARCHAR, check_type VARCHAR, table_name VARCHAR, column_name VARCHAR, params VARCHAR,
    lower_threshold DOUBLE, upper_threshold DOUBLE, monitor_only BOOLEAN, identifier_column VARCHAR, filter_expr VARCHAR,
    category VARCHAR);
INSERT INTO fm_contract VALUES
    ('stock_qty column delivered',  'null_rate',       'fm_feed', 'stock_qty', NULL, NULL, 0.999, false, 'delivery_date', NULL, 'Schema'),
    ('No columns outside the contract', 'compliance',  'fm_feed', NULL, '{"expression": "promo_flag IS NULL AND stock_level IS NULL"}', 1.0, NULL, true, 'delivery_date', NULL, 'Schema'),
    ('At least 1,000 SKUs',         'volume',          'fm_feed', NULL, NULL, 1000, NULL, false, 'delivery_date', NULL, 'Volume'),
    ('SKU is unique',               'duplicate_count', 'fm_feed', 'sku', NULL, NULL, 0, false, 'delivery_date', NULL, 'Volume'),
    ('EAN present',                 'null_rate',       'fm_feed', 'ean', NULL, NULL, 0.02, false, 'delivery_date', NULL, 'Completeness'),
    ('Price present',               'null_rate',       'fm_feed', 'unit_price', NULL, NULL, 0.005, false, 'delivery_date', NULL, 'Completeness'),
    ('Stock present',               'null_rate',       'fm_feed', 'stock_qty', NULL, NULL, 0.02, false, 'delivery_date', NULL, 'Completeness'),
    ('EAN has 13 digits',           'regex_match',     'fm_feed', 'ean', '{"pattern": "^[0-9]{13}$"}', 0.99, NULL, true, 'delivery_date', NULL, 'Validity'),
    ('Currency is EUR',             'values_in_set',   'fm_feed', 'currency', '{"allowed_values": ["EUR"]}', 1.0, NULL, false, 'delivery_date', NULL, 'Validity'),
    ('Price is positive',           'compliance',      'fm_feed', NULL, '{"expression": "unit_price > 0"}', 1.0, NULL, false, 'delivery_date', NULL, 'Validity'),
    ('Category is known',           'values_in_set',   'fm_feed', 'category',
     '{"allowed_values": ["Kitchen","Audio","Lighting","Garden","Tools","Bathroom","Office","Toys"]}', 1.0, NULL, true, 'delivery_date', NULL, 'Validity'),
    ('Mean price 30–50 €',          'agg',             'fm_feed', 'unit_price', '{"agg": "avg"}', 30, 50, false, 'delivery_date', NULL, 'Distribution');

-- The time-aware checks judge one reference date each: one check row per
-- delivery (params.reference_date / reference_time, filter_expr = that file).
CREATE OR REPLACE TABLE fm_days AS SELECT DISTINCT delivery_date AS d FROM fm_feed;
INSERT INTO fm_contract
SELECT 'Prices at most 30 h old @' || d, 'freshness', 'fm_feed', 'price_valid_from',
       '{"reference_time": "' || d || ' 06:00:00"}', NULL, 30 * 3600, false, NULL,
       format('delivery_date = DATE ''{}''', d), 'Freshness'
FROM fm_days
UNION ALL
SELECT 'File arrived by 06:00 @' || d, 'freshness', 'fm_feed', 'received_at',
       '{"reference_time": "' || d || ' 06:00:00"}', 0, NULL, true, NULL,
       format('delivery_date = DATE ''{}''', d), 'Freshness'
FROM fm_days
UNION ALL
SELECT 'Row count within ±30 % of 7-day mean @' || d, 'rel_count_change', 'fm_feed', NULL,
       '{"date_column": "delivery_date", "window_days": 7, "reference_date": "' || d || '"}', -0.3, 0.3, false, NULL, NULL, 'Volume'
FROM fm_days WHERE d >= DATE '2026-08-24'
UNION ALL
SELECT 'Mean price inside its 28-day IQR band @' || d, 'metric_anomaly_iqr', 'fm_feed', NULL,
       '{"date_column": "delivery_date", "metric_column": "unit_price", "window_days": 28, "k": 3, "mode": "both", "reference_date": "' || d || '"}',
       NULL, NULL, true, NULL, NULL, 'Distribution'
FROM fm_days WHERE d >= DATE '2026-09-14';

CREATE OR REPLACE TABLE fm_checks_run AS SELECT * EXCLUDE (category) FROM fm_contract;
CREATE OR REPLACE TABLE fm_checks AS
SELECT coalesce(r.identifier::DATE, split_part(r.check_name, ' @', 2)::DATE) AS delivery_date,
       split_part(r.check_name, ' @', 1) AS check_name, c.category,
       CASE WHEN c.monitor_only THEN 'warning' ELSE 'blocking' END AS severity,
       r.check_type, round(r.value, 4) AS value, r.lower_threshold, r.upper_threshold, r.status
FROM run_checks('fm_checks_run') r JOIN fm_contract c USING (check_name);

-- ── 2. Per-delivery column fill: what each file actually contained ─────────
CREATE OR REPLACE TABLE fm_fill_checks AS SELECT * FROM fm_checks_run LIMIT 0;
INSERT INTO fm_fill_checks
SELECT 'fill ' || column_name, 'null_rate', 'fm_feed', column_name, NULL, NULL, 1, true, 'delivery_date', NULL
FROM (VALUES ('sku'), ('ean'), ('product_name'), ('category'), ('unit_price'), ('currency'),
             ('stock_qty'), ('price_valid_from'), ('stock_level'), ('promo_flag')) t(column_name);
CREATE OR REPLACE TABLE fm_fill AS
SELECT identifier::DATE AS delivery_date, column_name, round(value, 4) AS null_rate
FROM run_checks('fm_fill_checks');

-- ── 3. Today's file against the contract and against yesterday ─────────────
CREATE OR REPLACE TABLE fm_schema AS
SELECT 'today' AS delivery, s.status, s.missing_columns::VARCHAR AS missing_columns,
       (SELECT string_agg(column_name, ', ' ORDER BY column_index) FROM duckdb_columns()
        WHERE table_name = 'fm_today' AND column_name NOT IN (SELECT column_name FROM duckdb_columns() WHERE table_name = 'fm_yesterday')) AS new_columns,
       s.message
FROM schema_check('fm_today', ['delivery_date', 'received_at', 'sku', 'ean', 'product_name', 'category',
                               'unit_price', 'currency', 'stock_qty', 'price_valid_from']) s
UNION ALL
SELECT 'yesterday', s.status, s.missing_columns::VARCHAR, NULL, s.message
FROM schema_check('fm_yesterday', ['delivery_date', 'received_at', 'sku', 'ean', 'product_name', 'category',
                                   'unit_price', 'currency', 'stock_qty', 'price_valid_from']) s;

CREATE OR REPLACE TABLE fm_profile AS
SELECT 'today' AS delivery, column_name, column_type, round(null_rate, 4) AS null_rate, distinct_count, is_unique, min_val, max_val
FROM profile_table('fm_today')
UNION ALL
SELECT 'yesterday', column_name, column_type, round(null_rate, 4), distinct_count, is_unique, min_val, max_val
FROM profile_table('fm_yesterday');

-- ── 4. Price moves per SKU and delivery, scored by an isolation forest ─────
-- Every day-over-day price change (log ratio) of every SKU; the forest
-- scores how unusual each move is. Only the unusual moves are stored.
CREATE OR REPLACE TABLE fm_moves AS
WITH p AS (
    SELECT delivery_date, sku, category, any_value(unit_price) AS price, any_value(currency) AS currency
    FROM fm_feed GROUP BY ALL
)
SELECT t.delivery_date, t.sku, t.category, y.price AS price_before, t.price AS price_after, t.currency,
       ln(t.price / y.price) AS log_ratio
FROM p t JOIN p y ON y.sku = t.sku AND y.delivery_date = t.delivery_date - 1
WHERE t.price > 0 AND y.price > 0 AND t.price <> y.price
ORDER BY t.delivery_date, t.sku;
CREATE OR REPLACE TABLE fm_price_moves AS
SELECT m.delivery_date, m.sku, m.category, m.price_before, m.price_after, m.currency,
       round(100 * (m.price_after / m.price_before - 1), 1) AS change_pct, round(f.anomaly_score, 4) AS anomaly_score
FROM (SELECT *, row_number() OVER () AS row_id FROM fm_moves) m
JOIN isolation_forest('fm_moves', 'log_ratio', 200, 256, 0.02, 'scores', 7) f USING (row_id);

-- ── 5. One row per delivery for the history charts ─────────────────────────
CREATE OR REPLACE TABLE fm_deliveries AS
SELECT delivery_date, any_value(received_at) AS received_at, count(*) AS row_count,
       count(DISTINCT sku) AS skus, round(avg(unit_price), 2) AS mean_price,
       round(avg((ean IS NULL)::INTEGER), 4) AS ean_missing_rate,
       (SELECT count(*) FROM fm_moves m WHERE m.delivery_date = f.delivery_date) AS price_changes
FROM fm_feed f GROUP BY delivery_date;
