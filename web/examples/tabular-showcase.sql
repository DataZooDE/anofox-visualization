-- @title Can we switch on the new CRM?
-- @group Data quality
-- @tags  tabular, data quality, migration
--
-- Tabular showcase — a data-quality report with anofox_tabular.
--
-- STORED RESULTS, NOT LIVE. The check results, weekly rule runs, profiles,
-- validity rates, anomaly scores and the migration diff below were computed
-- natively with the anofox_tabular extension (examples/tabular-showcase/
-- compute.sql, written to web/data/tabular/*.csv by
-- scripts/gen-showcase-data.sh). anofox_tabular has no DuckDB-Wasm build, so
-- the browser demo only reads, filters and plots the stored tables. The
-- filters below re-slice stored results; they never re-run the extension.
--
-- A made-up online shop moves its customer master to a new CRM and asks: can
-- we trust the data we are about to switch on? 600 customers and 4,509 orders
-- (Jun–Sep 2026), synthetic and deterministic, with planted problems: bad
-- emails and VAT IDs, missing phones, duplicate and negative orders, orphan
-- customers, a feed outage and a three-day pricing bug.
--
-- Tabs: Overview · Rules · Over time · Anomalies · Profiles · Migration.
-- Natively, run from web/:  dashboard --check ../examples/tabular-showcase/showcase.sql

-- ── Brand ────────────────────────────────────────────────────────────────────
SELECT 'anofox_tabular'::BRAND;
SELECT 'Can we switch on the new CRM?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_tabular results ──────────────────────────────────
CREATE OR REPLACE TABLE tq_check_results AS FROM read_csv('data/tabular/tq_check_results.csv');
CREATE OR REPLACE TABLE tq_check_history AS FROM read_csv('data/tabular/tq_check_history.csv');
CREATE OR REPLACE TABLE tq_daily AS FROM read_csv('data/tabular/tq_daily.csv');
CREATE OR REPLACE TABLE tq_profile AS FROM read_csv('data/tabular/tq_profile.csv');
CREATE OR REPLACE TABLE tq_validity AS FROM read_csv('data/tabular/tq_validity.csv');
CREATE OR REPLACE TABLE tq_validity_new AS FROM read_csv('data/tabular/tq_validity_new.csv');
CREATE OR REPLACE TABLE tq_anomaly AS FROM read_csv('data/tabular/tq_anomaly.csv');
CREATE OR REPLACE TABLE tq_outlier_tree AS FROM read_csv('data/tabular/tq_outlier_tree.csv');
CREATE OR REPLACE TABLE tq_diff AS FROM read_csv('data/tabular/tq_diff.csv');
CREATE OR REPLACE TABLE tq_provenance AS FROM read_csv('data/tabular/provenance.csv');

-- Rows behind each check result: how many rows a failing check asks you to fix.
CREATE OR REPLACE TABLE tq_fix AS
SELECT r.*,
       round(CASE r.check_type WHEN 'duplicate_count' THEN r.value
                               WHEN 'null_rate' THEN r.value * n.n_rows
                               WHEN 'volume' THEN 0
                               WHEN 'agg' THEN 0
                               ELSE (1 - r.value) * n.n_rows END) AS rows_to_fix,
       CASE WHEN r.table_name = 'tq_orders' THEN 'Orders' ELSE 'Customers' END AS scope_name
FROM tq_check_results r
JOIN (SELECT 'tq_orders' AS table_name, value AS n_rows FROM tq_check_results WHERE check_type = 'volume' AND coalesce(identifier, '') = ''
      UNION ALL SELECT 'tq_customers_legacy', max(row_count) FROM tq_profile) n USING (table_name);

-- Filters. Every macro tolerates an unset variable (the native checker runs
-- the panels without the browser's controls).
CREATE OR REPLACE MACRO tq_scope(t) AS
    CASE coalesce(getvariable('scope'), 'All tables') WHEN 'Orders' THEN t = 'tq_orders'
                                                      WHEN 'Customers' THEN t = 'tq_customers_legacy' ELSE true END;
CREATE OR REPLACE MACRO tq_cty(c) AS
    coalesce(len(getvariable('country')) = 0 OR list_contains(getvariable('country'), c), true);
CREATE OR REPLACE MACRO tq_days(d) AS
    d BETWEEN coalesce(getvariable('from_day')::DATE, DATE '2026-06-01') AND coalesce(getvariable('to_day')::DATE, DATE '2026-09-28');
CREATE OR REPLACE MACRO tq_short(n) AS
    CASE n WHEN 'Orders: amount is positive' THEN 'positive amount' WHEN 'Orders: status is a known value' THEN 'status known'
           WHEN 'Orders: order_id is unique' THEN 'unique order_id' WHEN 'Orders: mean amount 50–150 €' THEN 'mean amount 50–150 €'
           ELSE replace(n, 'Orders: ', '') END;
CREATE OR REPLACE MACRO tq_thr() AS coalesce(getvariable('min_score'), 70) / 100.0;

-- ── Header: filters, KPIs ────────────────────────────────────────────────────

SELECT 'Filters'::GROUP;
SELECT scope ::BUTTONS, 'Checks on' ::LABEL FROM (VALUES (1, 'All tables'), (2, 'Orders'), (3, 'Customers')) t(k, scope) ORDER BY k;
SELECT DISTINCT country ::MULTISELECT, 'Countries' ::LABEL FROM tq_validity ORDER BY country;
SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE, 'Order dates' ::LABEL FROM tq_daily;
SELECT 70 AS min_score ::NUMBER, 'Outlier cut-off (score %)' ::LABEL;
SELECT 1::ENDGROUP;

SELECT 'Where the data stands'::GROUP;
SELECT count(*) FILTER (WHERE status = 'fail')::METRIC, ('of ' || count(*) || ' check results fail')::LABEL
FROM tq_check_results WHERE tq_scope(table_name);
SELECT sum(rows_to_fix) FILTER (WHERE status = 'fail')::COMPACT, 'rows to fix'::LABEL
FROM tq_fix WHERE tq_scope(table_name);
SELECT count(*) FILTER (WHERE count_status = 'fail' OR revenue_status = 'fail')::METRIC, 'days flagged in the date range'::LABEL
FROM tq_daily WHERE tq_days(day);
SELECT count(*) FILTER (WHERE anomaly_score >= tq_thr())::METRIC, 'orders above the anomaly threshold'::LABEL
FROM tq_anomaly WHERE tq_days(order_date);
SELECT (SELECT 100 * sum(valid_rate * n) / sum(n) FROM tq_validity_new WHERE field = 'email' AND tq_cty(country))::PERCENT,
       (SELECT 100 * sum(valid_rate * n) / sum(n) FROM tq_validity WHERE field = 'email' AND tq_cty(country))::DELTA,
       'valid emails in the new CRM, vs. legacy'::LABEL;
SELECT (SELECT 100 * sum(valid_rate * n) / sum(n) FROM tq_validity_new WHERE field = 'VAT ID' AND tq_cty(country))::PERCENT,
       (SELECT 100 * sum(valid_rate * n) / sum(n) FROM tq_validity WHERE field = 'VAT ID' AND tq_cty(country))::DELTA,
       'valid VAT IDs in the new CRM, vs. legacy'::LABEL;
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 4::COL;
SELECT 320::HEIGHT;
SELECT round(100.0 * count(*) FILTER (WHERE status = 'pass') / count(*)) ::GAUGE, '0,100'::RANGE,
       '<80:bad; <95:warn; good'::THRESHOLDS, 'Check results passing (%) — not yet ready to switch'::TITLE
FROM tq_check_results WHERE tq_scope(table_name);

SELECT 8::COL;
SELECT 320::HEIGHT;
SELECT check_name ::XAXIS, rows_to_fix ::BARCHART, 1 ::FLIP,
       'Rows to fix per failing check — orphan orders and malformed emails lead'::TITLE
FROM tq_fix WHERE status = 'fail' AND rows_to_fix > 0 AND tq_scope(table_name)
ORDER BY rows_to_fix;

SELECT 12::COL;
SELECT 'Failing checks — weekly trend for the order checks'::TITLE,
       f.status AS "Status" ::BADGE, f.check_name AS "Check" ::TABLE, f.scope_name AS "Table",
       round(f.value, 3) AS "Measured" ::PLAIN,
       coalesce(f.lower_threshold, f.upper_threshold) AS "Threshold" ::PLAIN,
       f.rows_to_fix AS "Rows to fix" ::PLAIN,
       h.trend AS "Weekly value" ::SPARKLINE,
       h.weeks_failing AS "Weeks failing" ::COLORSCALE
FROM tq_fix f
LEFT JOIN (SELECT check_name, list(value ORDER BY week) AS trend, count(*) FILTER (WHERE status = 'fail') AS weeks_failing
           FROM tq_check_history WHERE tq_days(week) GROUP BY 1) h USING (check_name)
WHERE f.status = 'fail' AND tq_scope(f.table_name)
ORDER BY f.rows_to_fix DESC;

-- ── 2 · Rules: the order checks week by week ─────────────────────────────────
SELECT 'Rules'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** the same rules run once per week (`run_checks` with a per-week `filter_expr`). "Customer exists" fails almost every week — a steady leak, not an incident — while the duplicate, negative-amount and mean-amount failures each hit single weeks.'::MARKDOWN;

SELECT 12::COL;
SELECT 320::HEIGHT;
SELECT week ::XAXIS, tq_short(check_name) ::CATEGORY, 1 ::BARCHART_STACKED,
       'Failing order checks per week — the outage week (10 Aug) and the pricing-bug week (31 Aug) stack up'::TITLE
FROM tq_check_history WHERE status = 'fail' AND tq_days(week) ORDER BY week, check_name;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT week ::XAXIS, tq_short(check_name) ::CATEGORY, 100 * value ::LINECHART, 99 ::REFLINE, 'percent'::YFORMAT,
       'Rate checks per week — "customer exists" stays under the 99 % line (dashed)'::TITLE
FROM tq_check_history
WHERE check_type IN ('match_rate', 'compliance', 'values_in_set') AND tq_days(week)
ORDER BY tq_short(check_name), week;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT week ::XAXIS, CASE WHEN status = 'fail' THEN 'under the floor' ELSE 'normal week' END ::CATEGORY, value ::BARCHART_STACKED, 250 ::REFLINE,
       'Orders per week — only the outage week misses the floor of 250'::TITLE
FROM tq_check_history WHERE check_type = 'volume' AND tq_days(week) ORDER BY week;

SELECT 12::COL;
SELECT 'All check results'::TITLE,
       status AS "Status" ::BADGE, check_name AS "Check" ::TABLE, identifier AS "Partition", round(value, 4) AS "Value" ::PLAIN,
       lower_threshold AS "Min" ::PLAIN, upper_threshold AS "Max" ::PLAIN, check_type AS "Type"
FROM tq_check_results WHERE tq_scope(table_name) ORDER BY status, check_name;

-- ── 3 · Over time: daily drift and anomaly checks ────────────────────────────
SELECT 'Over time'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** `rel_count_change` flags the **12 Aug** feed outage and a false alarm on **18 Aug** (the outage dragged the 7-day baseline down); `metric_anomaly_iqr` catches the **3–5 Sep** pricing bug that the count check cannot see. Narrow the date range in the filters to zoom in.'::MARKDOWN;

SELECT 8::COL;
SELECT 320::HEIGHT;
SELECT day ::XAXIS, CASE WHEN count_status = 'fail' THEN 'volume drift' ELSE 'normal' END ::CATEGORY,
       orders ::BARCHART_STACKED, 'Orders per day — volume drift flagged on 12 and 18 Aug; the strip shows which check fired'::TITLE
FROM tq_daily WHERE tq_days(day) ORDER BY day;
SELECT day ::XAXIS, check_kind ::YAXIS, '#d64545' ::CATEGORY, 1 ::HEATMAP, check_kind || ' on ' || strftime(day, '%d %b') ::LABEL,
       'Checks' ::TRACK
FROM (SELECT day, 'volume drift' AS check_kind FROM tq_daily WHERE count_status = 'fail' AND tq_days(day)
      UNION ALL SELECT day, 'revenue anomaly' FROM tq_daily WHERE revenue_status = 'fail' AND tq_days(day))
ORDER BY day;

SELECT 4::COL;
SELECT 'Flagged days'::TITLE,
       strftime(d.day, '%a %d %b') AS "Day" ::TABLE,
       CASE WHEN count_status = 'fail' AND revenue_status = 'fail' THEN 'volume + revenue'
            WHEN count_status = 'fail' THEN 'volume drift' ELSE 'revenue anomaly' END AS "Flag" ::BADGE,
       orders AS "Orders" ::PLAIN, round(100 * count_rel_change) AS "vs. 7-day mean %" ::TREND
FROM tq_daily d WHERE (count_status = 'fail' OR revenue_status = 'fail') AND tq_days(d.day) ORDER BY d.day;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT day ::XAXIS, CASE WHEN revenue_status = 'fail' THEN 'revenue anomaly' ELSE 'normal' END ::CATEGORY,
       revenue ::BARCHART_STACKED, '€'::YFORMAT, 'Revenue per day — the 3–5 Sep pricing bug stands out by a factor of ten'::TITLE
FROM tq_daily WHERE tq_days(day) ORDER BY day;

-- ── 4 · Anomalies: isolation forest + outlier tree ───────────────────────────
SELECT 'Anomalies'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** `isolation_forest_mv` scored every order on amount and quantity; the **Outlier cut-off** filter sets the score threshold (in %). At the default 70 all ' || (SELECT count(*) FILTER (WHERE anomaly_score >= 0.7) FROM tq_anomaly) || ' flagged orders fall on the pricing-bug days; lower it and ordinary large orders start to creep in. Click a channel bar to filter the order list.')::MARKDOWN;

SELECT 7::COL;
SELECT 380::HEIGHT;
SELECT quantity ::XAXIS, amount ::JITTER,
       CASE WHEN anomaly_score >= tq_thr() THEN 'outlier' ELSE 'normal' END ::CATEGORY, '€'::YFORMAT,
       'Order amount by quantity — flagged orders sit far above their quantity''s cloud'::TITLE
FROM tq_anomaly WHERE tq_days(order_date) ORDER BY anomaly_score >= tq_thr() DESC, order_id;

SELECT 5::COL;
SELECT 380::HEIGHT;
SELECT round(floor(anomaly_score * 50) / 50, 2) ::XAXIS,
       CASE WHEN anomaly_score >= tq_thr() THEN 'outlier' ELSE 'normal' END ::CATEGORY,
       count(*) ::BARCHART_STACKED, 'Anomaly scores — a clear gap before the flagged tail'::TITLE
FROM tq_anomaly WHERE tq_days(order_date) GROUP BY ALL ORDER BY 1, 2;

SELECT 4::COL;
SELECT 360::HEIGHT;
SELECT 'channel'::SELECT_AS;
SELECT channel ::XAXIS, count(*) ::BARCHART, 'Flagged orders by channel — click to filter'::TITLE
FROM tq_anomaly WHERE anomaly_score >= tq_thr() AND tq_days(order_date) GROUP BY ALL ORDER BY 2 DESC;

SELECT 8::COL;
SELECT 'Flagged orders, with the outlier tree''s explanation'::TITLE,
       a.order_id AS "Order" ::PLAIN, strftime(a.order_date, '%d %b') AS "Date" ::TABLE, a.channel AS "Channel", a.quantity AS "Qty" ::PLAIN,
       a.amount AS "Amount (€)" ::PLAIN, round(100 * a.anomaly_score) AS "Score" ::PLAIN,
       coalesce(regexp_replace(t.explanation, ' \(mean.*\)', ''), '—') AS "Outlier tree"
FROM tq_anomaly a LEFT JOIN tq_outlier_tree t USING (order_id)
WHERE a.anomaly_score >= tq_thr() AND tq_days(a.order_date) AND coalesce(getvariable('channel'), '') IN ('', a.channel)
ORDER BY a.anomaly_score DESC;

-- ── 5 · Profiles: columns and field validity ─────────────────────────────────
SELECT 'Profiles'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** `vat_id` is 71 % empty — expected, only business customers have one — but 10–20 % of the IDs present fail the check digit outside NL. The migration copies the problems over: validity in the new CRM is the same as in the legacy one.'::MARKDOWN;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT field ::XAXIS, country ::YAXIS, round(100 * valid_rate, 1) ::HEATMAP, 'percent'::YFORMAT, ''::DATALABELS,
       'Valid values by country (%), legacy CRM — VAT IDs are the weak spot'::TITLE
FROM tq_validity WHERE tq_cty(country) ORDER BY field, country;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT field ::XAXIS, crm ::CATEGORY, 100 * sum(valid_rate * n) / sum(n) ::BARCHART, 'percent'::YFORMAT, 100 ::REFLINE,
       'Legacy vs. new CRM — the migration fixes nothing'::TITLE
FROM (SELECT *, 'Legacy CRM' AS crm FROM tq_validity UNION ALL SELECT *, 'New CRM' FROM tq_validity_new)
WHERE tq_cty(country) GROUP BY ALL ORDER BY field, crm;

SELECT 12::COL;
SELECT 'Column profile of the legacy customer master (profile_table)'::TITLE,
       column_name AS "Column" ::TABLE, column_type AS "Type", pattern_summary AS "Pattern",
       100 * null_rate AS "Null" ::PERCENT, null_count AS "Nulls" ::COLORSCALE,
       100 * distinct_rate AS "Distinct" ::PERCENT, is_unique AS "Unique", min_val AS "Min" ::PLAIN, max_val AS "Max" ::PLAIN
FROM tq_profile ORDER BY null_rate DESC, column_name;

-- ── 6 · Migration: diff_joindiff ─────────────────────────────────────────────
SELECT 'Migration'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** ' || (SELECT count(*) FROM tq_diff WHERE diff_type = 'removed' AND tq_cty(country)) || ' customers did not make it into the new CRM — each needs an explanation before the switch. Most changed rows are re-typed emails; four were re-coded to a segment `smb` the new CRM will reject. Click a diff type to filter the list.')::MARKDOWN;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT 'diff'::SELECT_AS;
SELECT diff_type ::CATEGORY, count(*) ::DONUTCHART, 'Differing rows by type'::TITLE
FROM tq_diff WHERE tq_cty(country) GROUP BY ALL ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT changed_fields ::XAXIS, count(*) ::BARCHART, 1 ::FLIP, 'Changed rows — mostly the email'::TITLE
FROM tq_diff WHERE diff_type = 'changed' AND tq_cty(country) GROUP BY ALL ORDER BY 2;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT country ::XAXIS, diff_type ::CATEGORY, count(*) ::BARCHART_STACKED, 'By country — all new sign-ups are German'::TITLE
FROM tq_diff WHERE tq_cty(country) GROUP BY ALL ORDER BY 1, 2;

SELECT 12::COL;
SELECT 'Every differing row'::TITLE,
       diff_type AS "Diff" ::BADGE, customer_id AS "Customer" ::PLAIN, country AS "Country" ::TABLE, changed_fields AS "Changed",
       email_before AS "Email (legacy)", email_after AS "Email (new)"
FROM tq_diff WHERE tq_cty(country) AND coalesce(getvariable('diff'), '') IN ('', diff_type)
ORDER BY diff_type, customer_id;
