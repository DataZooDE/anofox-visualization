-- Tabular showcase — a data-quality report with anofox_tabular.
--
-- STORED RESULTS, NOT LIVE. The check results, profiles, anomaly scores and
-- the diff below were computed natively with the anofox_tabular extension
-- (examples/tabular-showcase/compute.sql, written to web/data/tabular/*.csv
-- by scripts/gen-showcase-data.sh). anofox_tabular has no DuckDB-Wasm build,
-- so the browser demo only reads and plots the stored tables.
--
-- A made-up online shop moves its customer master to a new CRM and asks: can
-- we trust the data we are about to switch on? 600 customers and 4,509 orders
-- (Jun–Sep 2026), synthetic and deterministic, with planted problems: bad
-- emails and VAT IDs, missing phones, duplicate and negative orders, orphan
-- customers, a feed outage and a three-day pricing bug.
--
-- Tabs: 1 Overview · 2 Rules · 3 Over time · 4 Anomalies · 5 Profiles ·
-- 6 Migration diff. Natively, run from web/:  dashboard --check ../examples/tabular-showcase/showcase.sql

-- ─────────────────────────────────────────────────────────────────────────────
-- Data: the stored anofox_tabular results
-- ─────────────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE tq_check_results AS FROM read_csv('data/tabular/tq_check_results.csv');
CREATE OR REPLACE TABLE tq_daily AS FROM read_csv('data/tabular/tq_daily.csv');
CREATE OR REPLACE TABLE tq_profile AS FROM read_csv('data/tabular/tq_profile.csv');
CREATE OR REPLACE TABLE tq_profile_summary AS FROM read_csv('data/tabular/tq_profile_summary.csv');
CREATE OR REPLACE TABLE tq_validity AS FROM read_csv('data/tabular/tq_validity.csv');
CREATE OR REPLACE TABLE tq_anomaly AS FROM read_csv('data/tabular/tq_anomaly.csv');
CREATE OR REPLACE TABLE tq_outlier_tree AS FROM read_csv('data/tabular/tq_outlier_tree.csv');
CREATE OR REPLACE TABLE tq_diff AS FROM read_csv('data/tabular/tq_diff.csv');
CREATE OR REPLACE TABLE tq_provenance AS FROM read_csv('data/tabular/provenance.csv');

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Can we switch on the new CRM? A data-quality report with anofox_tabular (stored results)'::LABEL;

SELECT 12::COL;
SELECT ('**Stored results, computed natively.** Every check, profile and score on this page was computed with the **anofox_tabular** DuckDB extension (version `' || (SELECT extension_version FROM tq_provenance) || '`) on a native DuckDB ' || (SELECT duckdb_version FROM tq_provenance) || ' by `examples/tabular-showcase/compute.sql`; the demo reads the stored tables from `web/data/tabular/`. They are **not** recomputed in your browser: anofox_tabular has no DuckDB-Wasm build. The data is synthetic and deterministic. The calls used: `run_checks` (a suite of checks stored as rows), `rel_count_change` and `metric_anomaly_iqr` (time-aware checks), `profile_table`, `email_is_valid` / `phonenumber_is_valid` / `vat_is_valid`, `isolation_forest_mv`, `outlier_tree` and `diff_joindiff`.')::MARKDOWN, 'About this dashboard'::TITLE;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT '1 Overview'::TAB;

SELECT 12::COL;
SELECT ('**Question:** is the customer and order data good enough to switch the new CRM on? **What to look for:** **' || (SELECT count(*) FILTER (WHERE status = 'fail') FROM tq_check_results) || ' of ' || (SELECT count(*) FROM tq_check_results) || '** check results fail — most of them narrowly (a 93.5 % well-formed email rate against a 98 % target, 2.2 % orphan orders against 1 %), which is what real data looks like. The time-aware checks flag **' || (SELECT count(*) FILTER (WHERE count_status = 'fail' OR revenue_status = 'fail') FROM tq_daily) || ' days**: the feed outage on 12 Aug, an over-reaction a week later (the outage dragged the baseline down), and the three days of a **pricing bug** (3–5 Sep) whose orders the isolation forest and the outlier tree also single out. The migration diff shows **' || (SELECT count(*) FROM tq_diff WHERE diff_type = 'changed') || ' changed** customers — mostly re-typed emails. Verdict: fix the email and VAT data and re-run the pricing-bug days before switching over.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Data quality at a glance'::GROUP;
SELECT (SELECT count(*) FILTER (WHERE status = 'fail') FROM tq_check_results)::METRIC, ('of ' || (SELECT count(*) FROM tq_check_results) || ' check results fail')::LABEL;
SELECT (SELECT count(*) FILTER (WHERE count_status = 'fail' OR revenue_status = 'fail') FROM tq_daily)::METRIC, 'days flagged by time-aware checks'::LABEL;
SELECT (SELECT count(*) FILTER (WHERE is_anomaly) FROM tq_anomaly)::METRIC, 'orders flagged by the isolation forest'::LABEL;
SELECT (100 * (SELECT avg(valid_rate) FROM tq_validity WHERE field = 'email'))::PERCENT, 'valid customer emails'::LABEL;
SELECT (SELECT count(*) FROM tq_diff)::METRIC, 'customer rows differ after the migration'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT r.check_name ::XAXIS,
       round(CASE r.check_type WHEN 'duplicate_count' THEN r.value
                               WHEN 'null_rate' THEN r.value * n.n_rows
                               ELSE (1 - r.value) * n.n_rows END) ::BARCHART, 10 ::DATALABELS, 1 ::FLIP,
       'Rows to fix per failing check — orphan orders and malformed emails top the list'::TITLE
FROM tq_check_results r
JOIN (SELECT 'tq_orders' AS table_name, value AS n_rows FROM tq_check_results WHERE check_type = 'volume' AND identifier = ''
      UNION ALL SELECT 'tq_customers_legacy', max(row_count) FROM tq_profile) n USING (table_name)
WHERE r.status = 'fail'
ORDER BY 2 DESC;

-- ── 2 · Rules (run_checks) ───────────────────────────────────────────────────
SELECT '2 Rules'::TAB;

SELECT 12::COL;
SELECT '**How:** the suite is a table — one row per check (`check_type`, target table and column, JSON params, thresholds, an optional partition column) — and `SELECT * FROM run_checks(''checks'')` runs all of them and returns uniform rows: the measured **value**, the **thresholds**, a **status** and a message. **What to look for:** the rate checks fail by small margins (0.993 vs 1.0 for the order status: a typo `unknwon` in 0.7 % of rows), the integrity checks by absolute counts (**4** duplicate order IDs). The per-channel volume check runs once per channel through the partition column and passes for all three.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 5::COL;
SELECT 420::HEIGHT;
SELECT check_name ::XAXIS, round(100 * (lower_threshold - value), 2) ::BARCHART, 10 ::DATALABELS, 1 ::FLIP,
       'Failing rate checks: points short of the target — all within 5 points'::TITLE
FROM tq_check_results
WHERE check_type IN ('compliance', 'values_in_set', 'match_rate', 'regex_match') AND status = 'fail'
ORDER BY 2 DESC;

SELECT 7::COL;
SELECT 'All check results'::TITLE,
       status AS "Status" ::BADGE, check_name AS "Check" ::TABLE, identifier AS "Partition", value AS "Value" ::PLAIN,
       lower_threshold AS "Min" ::PLAIN, upper_threshold AS "Max" ::PLAIN
FROM tq_check_results ORDER BY status, check_name;

-- ── 3 · Over time (rel_count_change, metric_anomaly_iqr) ─────────────────────
SELECT '3 Over time'::TAB;

SELECT 12::COL;
SELECT '**How:** `rel_count_change` compares a day''s row count with the mean of the previous 7 days (flag beyond ±50 %); `metric_anomaly_iqr` flags a day whose revenue lies outside the IQR fence (k = 1.5) of the previous 28 days. Each judges one reference date, so the suite holds **one check row per day** and `run_checks` evaluates the whole history in one call. **What to look for:** the **12 Aug** feed outage (9 orders instead of ~40) and, one week later, a false alarm on **18 Aug** — the outage pulled the 7-day baseline down, a reminder to keep incidents out of baselines. The revenue check catches the **pricing bug on 3–5 Sep** (amounts ×10) that the count check cannot see: the number of orders was normal.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT day ::XAXIS, CASE WHEN count_status = 'fail' THEN 'flagged: count drift' ELSE 'normal' END ::CATEGORY,
       orders ::BARCHART_STACKED, 'Orders per day — rel_count_change flags the 12 Aug outage and the 18 Aug rebound'::TITLE
FROM tq_daily ORDER BY day;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT day ::XAXIS, CASE WHEN revenue_status = 'fail' THEN 'flagged: revenue anomaly' ELSE 'normal' END ::CATEGORY,
       revenue ::BARCHART_STACKED, '€'::YFORMAT, 'Revenue per day — metric_anomaly_iqr flags the 3–5 Sep pricing bug'::TITLE
FROM tq_daily ORDER BY day;

-- ── 4 · Anomalies (isolation_forest_mv, outlier_tree) ────────────────────────
SELECT '4 Anomalies'::TAB;

SELECT 12::COL;
SELECT ('**How:** `isolation_forest_mv(''orders'', ''amount, quantity'', …, ''scores'')` scores every order by how easily random splits isolate it (contamination 1 %, fixed seed); `outlier_tree` explains outliers in context ("unusually high **when quantity ≤ 2**"). **What to look for:** the ' || (SELECT count(*) FILTER (WHERE is_anomaly) FROM tq_anomaly) || ' flagged orders sit far above the cloud of normal amounts for their quantity, and all of them were placed on **3–5 Sep** — the same pricing bug the revenue check found, now down to the individual orders to re-bill.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 7::COL;
SELECT 420::HEIGHT;
SELECT quantity ::XAXIS, amount ::JITTER, CASE WHEN is_anomaly THEN 'anomaly' ELSE 'normal' END ::CATEGORY, '€'::YFORMAT,
       'Order amount by quantity — the flagged orders are ×10 the usual amount'::TITLE
FROM tq_anomaly ORDER BY is_anomaly, order_id;

SELECT 5::COL;
SELECT 420::HEIGHT;
SELECT strftime(order_date, '%d %b') ::XAXIS, count(*) ::BARCHART, 10 ::DATALABELS,
       'Flagged orders per day — all on 3–5 Sep'::TITLE
FROM tq_anomaly WHERE is_anomaly GROUP BY order_date ORDER BY order_date;

SELECT 12::COL;
SELECT 'outlier_tree: the most extreme amounts, with their explanation'::TITLE,
       order_id AS "Order" ::TABLE, order_date AS "Date", channel AS "Channel", quantity AS "Qty" ::PLAIN,
       outlier_value AS "Amount (€)" ::PLAIN, z_score AS "z" ::PLAIN, explanation AS "Why it is an outlier"
FROM tq_outlier_tree ORDER BY z_score DESC LIMIT 12;

-- ── 5 · Profiles (profile_table, validators) ─────────────────────────────────
SELECT '5 Profiles'::TAB;

SELECT 12::COL;
SELECT '**How:** `profile_table(''customers'')` returns one row per column — type, null and distinct rates, min / max and a detected **pattern** (email, phone, …); `email_is_valid`, `phonenumber_is_valid(phone, country)` and `vat_is_valid` check each value (the VAT check digit included). **What to look for:** `vat_id` is 71 % empty — expected, only business customers have one — but among the IDs present **10–20 %** fail the check digit in DE, FR and IT. Phone validity is lower than the null rate suggests: truncated numbers like `+31 12345` are present but invalid. Every email is distinct, yet about one in fifteen is malformed.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT column_name ::XAXIS, metric ::CATEGORY, rate ::BARCHART, 'percent'::YFORMAT, 1 ::FLIP,
       'Customer columns — null and distinct rate (profile_table)'::TITLE
FROM (SELECT column_name, 'null rate' AS metric, round(100 * null_rate, 1) AS rate FROM tq_profile
      UNION ALL SELECT column_name, 'distinct rate', round(100 * distinct_rate, 1) FROM tq_profile)
ORDER BY column_name DESC, metric;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT field ::XAXIS, country ::YAXIS, round(100 * valid_rate, 1) ::HEATMAP,
       'Valid values by country (%) — VAT IDs are the weak spot outside NL'::TITLE
FROM tq_validity ORDER BY field, country;

SELECT 12::COL;
SELECT 'Column profile of the customer master'::TITLE,
       column_name AS "Column" ::TABLE, column_type AS "Type", null_count AS "Nulls" ::PLAIN,
       round(100 * null_rate, 1) AS "Null %" ::PLAIN, distinct_count AS "Distinct" ::PLAIN, is_unique AS "Unique",
       pattern_summary AS "Pattern", min_val AS "Min", max_val AS "Max"
FROM tq_profile ORDER BY null_rate DESC, column_name;

-- ── 6 · Migration diff (diff_joindiff) ───────────────────────────────────────
SELECT '6 Migration diff'::TAB;

SELECT 12::COL;
SELECT ('**How:** `diff_joindiff(''customers_legacy'', ''customers_new'', [''customer_id''])` joins the two tables on the key and returns every **added**, **removed** and **changed** row (NULL-safe on the key). **What to look for:** ' || (SELECT count(*) FROM tq_diff WHERE diff_type = 'removed') || ' customers did not make it into the new CRM — each needs an explanation before the switch — and ' || (SELECT count(*) FROM tq_diff WHERE diff_type = 'added') || ' are new sign-ups. Of the ' || (SELECT count(*) FROM tq_diff WHERE diff_type = 'changed') || ' changed customers most have a re-typed **email** (`customer47@…` → `c.47@…`), some lost the country code of their **phone**, and four were re-coded to an unknown **segment** `smb` that the new CRM''s rules will reject.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 4::COL;
SELECT 360::HEIGHT;
SELECT diff_type ::XAXIS, count(*) ::BARCHART, 10 ::DATALABELS, 'Rows by diff type'::TITLE
FROM tq_diff GROUP BY 1 ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 360::HEIGHT;
SELECT changed_fields ::XAXIS, count(*) ::BARCHART, 10 ::DATALABELS, 'Changed rows — which field changed'::TITLE
FROM tq_diff WHERE diff_type = 'changed' GROUP BY 1 ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 'Every differing row'::TITLE,
       diff_type AS "Diff" ::BADGE, customer_id AS "Customer" ::PAGED, changed_fields AS "Changed",
       email_before AS "Email (legacy)", email_after AS "Email (new)"
FROM tq_diff ORDER BY diff_type, customer_id;
