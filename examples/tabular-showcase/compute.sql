-- anofox-tabular showcase — the native computation behind the stored results.
--
-- Runs natively only: anofox_tabular has no DuckDB-Wasm build yet, so the
-- browser demo reads the tables this script produces from web/data/tabular/
-- (regenerate: scripts/gen-showcase-data.sh tabular). Run from the repo root:
--   duckdb -unsigned -c "INSTALL anofox_tabular FROM 'https://get.erpl.io'"   # once
--   duckdb -unsigned -c ".read examples/tabular-showcase/compute.sql"
LOAD anofox_tabular;
.read examples/tabular-showcase/data.sql

-- ── 1. A check suite, defined once as rows and run with one call ───────────
CREATE OR REPLACE TABLE tq_checks (
    check_name VARCHAR, check_type VARCHAR, table_name VARCHAR, column_name VARCHAR, params VARCHAR,
    lower_threshold DOUBLE, upper_threshold DOUBLE, monitor_only BOOLEAN, identifier_column VARCHAR, filter_expr VARCHAR);
INSERT INTO tq_checks VALUES
    ('Orders: at least 3,000 rows',          'volume',          'tq_orders', NULL, NULL, 3000, NULL, false, NULL, NULL),
    ('Orders: order_id is unique',           'duplicate_count', 'tq_orders', 'order_id', NULL, NULL, 0, false, NULL, NULL),
    ('Orders: amount is positive',           'compliance',      'tq_orders', NULL, '{"expression": "amount > 0"}', 1.0, NULL, false, NULL, NULL),
    ('Orders: status is a known value',      'values_in_set',   'tq_orders', 'status', '{"allowed_values": ["delivered","shipped","returned"]}', 1.0, NULL, false, NULL, NULL),
    ('Orders: customer exists',              'match_rate',      'tq_orders', 'customer_id', '{"right_table": "tq_customers_legacy", "left_keys": "customer_id", "right_keys": "customer_id"}', 0.99, NULL, false, NULL, NULL),
    ('Orders: mean amount 50–150 €',         'agg',             'tq_orders', 'amount', '{"agg": "avg"}', 50, 150, false, NULL, NULL),
    ('Customers: email well-formed',         'regex_match',     'tq_customers_legacy', 'email', '{"pattern": "^[^@\\s()]+@[^@\\s]+\\.[a-z]{2,}$"}', 0.98, NULL, false, NULL, NULL),
    ('Customers: phone present',             'null_rate',       'tq_customers_legacy', 'phone', NULL, NULL, 0.02, false, NULL, NULL),
    ('Customers: email unique',              'duplicate_count', 'tq_customers_legacy', 'email', NULL, NULL, 0, false, NULL, NULL),
    ('Customers: segment is known',          'values_in_set',   'tq_customers_legacy', 'segment', '{"allowed_values": ["business","consumer"]}', 1.0, NULL, false, NULL, NULL),
    ('Customers: business has a VAT ID',     'compliance',      'tq_customers_legacy', NULL, '{"expression": "segment <> ''business'' OR vat_id IS NOT NULL"}', 1.0, NULL, false, NULL, NULL),
    ('Orders per channel: ≥ 1,000',          'volume',          'tq_orders', NULL, NULL, 1000, NULL, true, 'channel', NULL);

CREATE OR REPLACE TABLE tq_check_results AS
SELECT check_name, check_type, table_name, coalesce(identifier, '') AS identifier, round(value, 4) AS value,
       lower_threshold, upper_threshold, status, message
FROM run_checks('tq_checks');

-- The order checks once per week (filter_expr restricts each run to that
-- week's orders), to see when a rule started to fail. Full weeks only.
CREATE OR REPLACE TABLE tq_weeks AS
SELECT DISTINCT date_trunc('week', order_date)::DATE AS wk FROM tq_orders
WHERE date_trunc('week', order_date)::DATE >= DATE '2026-06-01' AND date_trunc('week', order_date)::DATE + 6 <= DATE '2026-09-28';
CREATE OR REPLACE TABLE tq_weekly_checks AS SELECT * FROM tq_checks LIMIT 0;
INSERT INTO tq_weekly_checks
SELECT c.check_name || ' @' || w.wk::VARCHAR, c.check_type, c.table_name, c.column_name, c.params,
       c.lower_threshold, c.upper_threshold, false, NULL,
       format('order_date >= DATE ''{}'' AND order_date < DATE ''{}''', w.wk, w.wk + 7)
FROM tq_checks c, tq_weeks w
WHERE c.check_name IN ('Orders: amount is positive', 'Orders: status is a known value', 'Orders: customer exists',
                       'Orders: order_id is unique', 'Orders: mean amount 50–150 €')
UNION ALL  -- a weekly volume floor: about 280 orders arrive in a normal week
SELECT 'Orders: ≥ 250 per week @' || w.wk::VARCHAR, 'volume', 'tq_orders', NULL, NULL, 250, NULL, false, NULL,
       format('order_date >= DATE ''{}'' AND order_date < DATE ''{}''', w.wk, w.wk + 7)
FROM tq_weeks w;

CREATE OR REPLACE TABLE tq_check_history AS
SELECT split_part(check_name, ' @', 1) AS check_name, split_part(check_name, ' @', 2)::DATE AS week,
       check_type, round(value, 4) AS value, lower_threshold, upper_threshold, status
FROM run_checks('tq_weekly_checks');

-- ── 2. Time-aware checks for every day: count drift and metric anomalies ───
-- rel_count_change / metric_anomaly_iqr judge one reference date; a check
-- row per day (params.reference_date) runs them over the whole history.
CREATE OR REPLACE TABLE tq_daily_checks AS
SELECT * FROM tq_checks LIMIT 0;
INSERT INTO tq_daily_checks
SELECT 'count ' || d::VARCHAR, 'rel_count_change', 'tq_orders', NULL,
       '{"date_column": "order_date", "window_days": 7, "reference_date": "' || d::VARCHAR || '"}', -0.5, 0.5, false, NULL, NULL
FROM (SELECT DISTINCT order_date AS d FROM tq_orders WHERE order_date >= DATE '2026-06-08')
UNION ALL
SELECT 'revenue ' || d::VARCHAR, 'metric_anomaly_iqr', 'tq_daily_revenue', NULL,
       '{"date_column": "order_date", "metric_column": "revenue", "window_days": 28, "k": 1.5, "mode": "both", "reference_date": "' || d::VARCHAR || '"}',
       NULL, NULL, false, NULL, NULL
FROM (SELECT DISTINCT order_date AS d FROM tq_orders WHERE order_date >= DATE '2026-06-29');
CREATE OR REPLACE TABLE tq_daily_revenue AS
SELECT order_date, sum(amount) AS revenue FROM tq_orders GROUP BY 1;

CREATE OR REPLACE TABLE tq_daily AS
WITH r AS (SELECT split_part(check_name, ' ', 1) AS metric, split_part(check_name, ' ', 2)::DATE AS day, status, value
           FROM run_checks('tq_daily_checks'))
SELECT o.order_date AS day, count(*) AS orders, round(sum(o.amount), 2) AS revenue,
       any_value(c.status) AS count_status, any_value(c.value) AS count_rel_change,
       any_value(v.status) AS revenue_status
FROM tq_orders o
LEFT JOIN r c ON c.metric = 'count' AND c.day = o.order_date
LEFT JOIN r v ON v.metric = 'revenue' AND v.day = o.order_date
GROUP BY o.order_date;

-- ── 3. Column profiles of the customer master ──────────────────────────────
CREATE OR REPLACE TABLE tq_profile AS
SELECT column_name, column_type, row_count, null_count, round(null_rate, 4) AS null_rate,
       distinct_count, round(distinct_rate, 4) AS distinct_rate, is_unique, pattern_summary, min_val, max_val
FROM profile_table('tq_customers_legacy');
CREATE OR REPLACE TABLE tq_profile_summary AS SELECT * FROM profile_summary('tq_orders');

-- Field-level validity (email / phone / VAT) per country.
CREATE OR REPLACE TABLE tq_validity AS
SELECT country, 'email' AS field, round(avg(email_is_valid(email)::INTEGER), 4) AS valid_rate, count(*) AS n
FROM tq_customers_legacy GROUP BY ALL
UNION ALL
SELECT country, 'phone', round(avg(coalesce(phonenumber_is_valid(phone, country), false)::INTEGER), 4), count(*)
FROM tq_customers_legacy GROUP BY ALL
UNION ALL
SELECT country, 'VAT ID', round(avg(vat_is_valid(vat_id)::INTEGER), 4), count(*)
FROM tq_customers_legacy WHERE vat_id IS NOT NULL GROUP BY ALL;

-- The same field checks on the new CRM, to compare before and after.
CREATE OR REPLACE TABLE tq_validity_new AS
SELECT country, 'email' AS field, round(avg(email_is_valid(email)::INTEGER), 4) AS valid_rate, count(*) AS n
FROM tq_customers_new GROUP BY ALL
UNION ALL
SELECT country, 'phone', round(avg(coalesce(phonenumber_is_valid(phone, country), false)::INTEGER), 4), count(*)
FROM tq_customers_new GROUP BY ALL
UNION ALL
SELECT country, 'VAT ID', round(avg(vat_is_valid(vat_id)::INTEGER), 4), count(*)
FROM tq_customers_new WHERE vat_id IS NOT NULL GROUP BY ALL;

-- ── 4. Anomalous orders: isolation forest + explainable outlier tree ───────
CREATE OR REPLACE TABLE tq_anomaly AS
SELECT o.order_id, o.order_date, o.quantity, o.amount, round(o.amount / o.quantity, 2) AS unit_price, o.channel,
       round(f.anomaly_score, 4) AS anomaly_score, f.is_anomaly
FROM (SELECT *, row_number() OVER () AS row_id FROM tq_orders) o
JOIN isolation_forest_mv('tq_orders', 'amount, quantity', 200, 256, 0.01, 'scores', 42) f
  USING (row_id);
-- (row_id is the 1-based row position in tq_orders.)
CREATE OR REPLACE TABLE tq_outlier_tree AS
SELECT o.order_id, o.order_date, o.channel, o.quantity, t.column_name, t.outlier_value,
       round(t.z_score, 1) AS z_score, t.explanation
FROM outlier_tree('tq_orders', 'channel,quantity,amount', 'outliers') t
JOIN (SELECT *, row_number() OVER () AS row_id FROM tq_orders) o USING (row_id);

-- ── 5. The migration diff: legacy CRM vs new CRM ───────────────────────────
CREATE OR REPLACE TABLE tq_diff AS
SELECT d.diff_type, d.customer_id, coalesce(l.country, d.country) AS country,
       CASE WHEN d.diff_type <> 'changed' THEN NULL
            ELSE concat_ws(', ',
                CASE WHEN l.email IS DISTINCT FROM d.email THEN 'email' END,
                CASE WHEN l.phone IS DISTINCT FROM d.phone THEN 'phone' END,
                CASE WHEN l.segment IS DISTINCT FROM d.segment THEN 'segment' END,
                CASE WHEN l.name IS DISTINCT FROM d.name THEN 'name' END) END AS changed_fields,
       coalesce(l.email, d.email) AS email_before, CASE WHEN d.diff_type = 'removed' THEN NULL ELSE d.email END AS email_after
FROM diff_joindiff('tq_customers_legacy', 'tq_customers_new', ['customer_id']) d
LEFT JOIN tq_customers_legacy l USING (customer_id);
