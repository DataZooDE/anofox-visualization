-- Is today's supplier feed safe to load? — monitoring an incoming data feed
-- with anofox_tabular.
--
-- STORED RESULTS, NOT LIVE. The check results, column fill rates, schema
-- check, profiles and price-move scores below were computed natively with
-- the anofox_tabular extension (examples/tabular-feed/compute.sql, written to
-- web/data/tabular-feed/*.csv by scripts/gen-tabular-cases.sh feed).
-- anofox_tabular has no DuckDB-Wasm build, so the browser demo only reads,
-- filters and plots the stored tables. The inputs below re-slice stored
-- results; they never re-run the extension.
--
-- A made-up wholesaler, Nordwind, sends a home-and-garden shop its full price
-- and stock list every night (60 deliveries, 17 Aug – 15 Oct 2026, ~1,200
-- SKUs each; synthetic and deterministic). Planted incidents: a cut-off file,
-- duplicated SKUs, USD prices, a late file, prices in cents, a stale re-send
-- and — today — a new export format that breaks four blocking checks.
--
-- Tabs: Overview · Schema & completeness · Volume & prices · Go / no-go.
-- Natively, run from web/:  dashboard --check ../examples/tabular-feed/showcase.sql

SELECT 'anofox_tabular'::BRAND;
SELECT 'Is today''s supplier feed safe to load?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_tabular results ──────────────────────────────────
CREATE OR REPLACE TABLE fm_checks AS FROM read_csv('data/tabular-feed/fm_checks.csv');
CREATE OR REPLACE TABLE fm_fill AS FROM read_csv('data/tabular-feed/fm_fill.csv');
CREATE OR REPLACE TABLE fm_schema AS FROM read_csv('data/tabular-feed/fm_schema.csv');
CREATE OR REPLACE TABLE fm_profile AS FROM read_csv('data/tabular-feed/fm_profile.csv');
CREATE OR REPLACE TABLE fm_price_moves AS FROM read_csv('data/tabular-feed/fm_price_moves.csv');
CREATE OR REPLACE TABLE fm_deliveries AS FROM read_csv('data/tabular-feed/fm_deliveries.csv');
CREATE OR REPLACE TABLE fm_provenance AS FROM read_csv('data/tabular-feed/provenance.csv');

-- Inputs. Every macro tolerates an unset variable (the native checker runs
-- the panels without the browser's controls).
CREATE OR REPLACE MACRO fm_day() AS coalesce(nullif(getvariable('delivery'), '')::DATE, DATE '2026-10-15');
CREATE OR REPLACE MACRO fm_limit() AS coalesce(getvariable('max_missing'), 2) / 100.0;
CREATE OR REPLACE MACRO fm_cut() AS coalesce(getvariable('min_score'), 65) / 100.0;
CREATE OR REPLACE MACRO fm_fmt(kind, v) AS
    CASE WHEN v IS NULL THEN '—'
         WHEN kind IN ('null_rate', 'compliance', 'values_in_set', 'regex_match') THEN printf('%.1f %%', 100 * v)
         WHEN kind = 'rel_count_change' THEN printf('%+.0f %%', 100 * v)
         WHEN kind = 'freshness' THEN printf('%.1f h', v / 3600)
         WHEN kind IN ('agg', 'metric_anomaly_iqr') THEN printf('%.2f €', v)
         ELSE printf('%,d', v::BIGINT) END;
CREATE OR REPLACE MACRO fm_days(d) AS
    d BETWEEN coalesce(getvariable('from_day')::DATE, DATE '2026-08-17') AND coalesce(getvariable('to_day')::DATE, DATE '2026-10-15');

-- The stored statuses, with the completeness checks re-judged against the
-- "Missing values allowed" input (their stored value is the null rate).
CREATE OR REPLACE VIEW fm_eval AS
SELECT *,
       CASE WHEN category = 'Completeness' AND check_type = 'null_rate'
            THEN CASE WHEN value > fm_limit() THEN (CASE severity WHEN 'blocking' THEN 'fail' ELSE 'warn' END) ELSE 'pass' END
            ELSE status END AS verdict
FROM fm_checks;
CREATE OR REPLACE VIEW fm_verdict AS
SELECT delivery_date,
       count(*) FILTER (WHERE verdict = 'fail') AS blocking,
       count(*) FILTER (WHERE verdict = 'warn') AS warnings,
       CASE WHEN count(*) FILTER (WHERE verdict = 'fail') > 0 THEN 'NO-GO'
            WHEN count(*) FILTER (WHERE verdict = 'warn') > 0 THEN 'GO with warnings' ELSE 'GO' END AS decision
FROM fm_eval GROUP BY delivery_date;

-- ── Header: the question, inputs, KPIs ───────────────────────────────────────
SELECT 12::COL;
SELECT ('**Who asks:** the data engineer on duty at 07:00, before Nordwind''s nightly price and stock file is loaded into the shop''s product catalogue. A bad load puts wrong prices and stock on the website within the hour; holding it back means yesterday''s prices for another day.')::MARKDOWN;

SELECT 'Inputs'::GROUP;
SELECT strftime(delivery_date, '%Y-%m-%d') AS delivery ::DROPDOWN, 'Delivery to judge' ::LABEL
FROM fm_deliveries ORDER BY delivery_date DESC;
SELECT 2 AS max_missing ::NUMBER, 'Missing values allowed (%)' ::LABEL;
SELECT 65 AS min_score ::NUMBER, 'Unusual price move from score (%)' ::LABEL;
SELECT min(delivery_date) AS from_day, max(delivery_date) AS to_day ::DATERANGE, 'History' ::LABEL FROM fm_deliveries;
SELECT 1::ENDGROUP;

SELECT 'The delivery at a glance'::GROUP;
SELECT (SELECT decision FROM fm_verdict WHERE delivery_date = fm_day()) ::TEXT_LARGE,
       ('decision for ' || strftime(fm_day(), '%a %d %b')) ::LABEL;
SELECT (SELECT blocking FROM fm_verdict WHERE delivery_date = fm_day()) ::METRIC,
       (SELECT blocking FROM fm_verdict WHERE delivery_date = fm_day() - 1) ::DELTA_LOWER_GOOD,
       'abs' ::DELTA_FORMAT, 'vs the day before' ::DELTA_LABEL, '>0:bad; good' ::THRESHOLDS,
       'blocking checks failing' ::LABEL;
SELECT (SELECT warnings FROM fm_verdict WHERE delivery_date = fm_day()) ::METRIC,
       (SELECT warnings FROM fm_verdict WHERE delivery_date = fm_day() - 1) ::DELTA_LOWER_GOOD,
       'abs' ::DELTA_FORMAT, 'vs the day before' ::DELTA_LABEL, '>0:warn; good' ::THRESHOLDS,
       'warnings' ::LABEL;
SELECT (SELECT row_count FROM fm_deliveries WHERE delivery_date = fm_day()) ::COMPACT,
       (SELECT avg(row_count) FROM fm_deliveries WHERE delivery_date BETWEEN fm_day() - 7 AND fm_day() - 1) ::DELTA,
       'vs the 7-day mean' ::DELTA_LABEL,
       'rows delivered' ::LABEL;
SELECT (SELECT 100 * ean_missing_rate FROM fm_deliveries WHERE delivery_date = fm_day()) ::PERCENT,
       (SELECT 100 * ean_missing_rate FROM fm_deliveries WHERE delivery_date = fm_day() - 1) ::DELTA_LOWER_GOOD,
       'abs' ::DELTA_FORMAT, 'vs the day before' ::DELTA_LABEL,
       ('>' || 100 * fm_limit() || ':bad; good') ::THRESHOLDS,
       'EANs missing' ::LABEL;
SELECT (SELECT strftime(received_at, '%H:%M') FROM fm_deliveries WHERE delivery_date = fm_day()) ::TEXT_LARGE,
       'file received (contract: by 06:00)' ::LABEL;
SELECT 1::ENDGROUP;

-- ── 1 · Overview: 60 nights of deliveries ────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** most nights pass every check. Seven incidents stand out — the cut-off file (29 Aug), duplicated SKUs (8 Sep), USD prices (16–18 Sep), a late file (21 Sep, a warning only), prices in cents (29 Sep – 1 Oct), a stale re-send (8 Oct) — and today, **'
        || strftime(DATE '2026-10-15', '%d %b') || '**, fails four blocking checks at once: the supplier changed its export (a renamed column, blank EANs, seven prices without their decimal point). Pick another night in **Delivery to judge** to see its verdict.')::MARKDOWN;

SELECT 12::COL;
SELECT 320::HEIGHT;
WITH incidents(day, what) AS (VALUES
    (DATE '2026-08-29', 'cut-off file'), (DATE '2026-09-08', 'duplicated SKUs'), (DATE '2026-09-16', 'USD prices'),
    (DATE '2026-09-29', 'prices in cents'), (DATE '2026-10-08', 're-send'), (DATE '2026-10-15', 'new format'))
SELECT d.delivery_date ::XAXIS, k.category ::CATEGORY, count(e.check_name) ::BARCHART_STACKED,
       any_value(i.day) ::XLINE, any_value(i.what) ::XLINE_LABEL,
       'Failing checks per delivery, by kind — today fails five at once'::TITLE
FROM fm_deliveries d CROSS JOIN (SELECT DISTINCT category FROM fm_checks) k
LEFT JOIN fm_eval e ON e.delivery_date = d.delivery_date AND e.category = k.category AND e.verdict <> 'pass'
LEFT JOIN incidents i ON i.day = d.delivery_date
WHERE fm_days(d.delivery_date)
GROUP BY ALL ORDER BY d.delivery_date, k.category;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT decision ::XAXIS, count(*) ::BARCHART, 'Verdicts over the history — one night in six was not safe to load'::TITLE
FROM fm_verdict WHERE fm_days(delivery_date) GROUP BY ALL ORDER BY 2 DESC;

SELECT 6::COL;
SELECT 300::HEIGHT;
WITH f AS (SELECT category, severity, count(*) AS n FROM fm_eval WHERE verdict <> 'pass' AND fm_days(delivery_date) GROUP BY ALL)
SELECT category ::XAXIS, severity ::CATEGORY, n ::BARCHART_STACKED,
       'Failures by check kind — the price checks fire most'::TITLE
FROM f ORDER BY sum(n) OVER (PARTITION BY category) DESC, severity;

SELECT 12::COL;
SELECT 'Blocking > 0'::FLAG;
SELECT 'Every delivery with a finding'::TITLE,
       strftime(v.delivery_date, '%a %d %b') AS "Delivery" ::TABLE, v.decision AS "Status" ::BADGE,
       v.blocking AS "Blocking" ::PLAIN, v.warnings AS "Warnings" ::PLAIN,
       d.row_count AS "Rows" ::PLAIN, strftime(d.received_at, '%H:%M') AS "Received",
       (SELECT string_agg(check_name, ', ' ORDER BY severity, check_name) FROM fm_eval e
        WHERE e.delivery_date = v.delivery_date AND e.verdict <> 'pass') AS "Failing checks",
       '#Go / no-go?v.delivery=' || strftime(v.delivery_date, '%Y-%m-%d') AS "Judge" ::LINK
FROM fm_verdict v JOIN fm_deliveries d USING (delivery_date)
WHERE v.decision <> 'GO' AND fm_days(v.delivery_date)
ORDER BY v.delivery_date DESC;

-- ── 2 · Schema & completeness ────────────────────────────────────────────────
SELECT 'Schema & completeness'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** the staging table stacks the nightly files by column name, so a column missing from a file shows as 100 % empty. Today `stock_qty` is gone, `stock_level` and `promo_flag` are new — the supplier renamed a column and added one — and 8 % of EANs are blank. `schema_check` on today''s file alone says the same.'::MARKDOWN;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT strftime(delivery_date, '%d %b') ::XAXIS, column_name ::YAXIS, round(100 * null_rate, 1) ::HEATMAP, 'percent'::YFORMAT,
       'Empty values per column (%), the last 14 deliveries — today''s file has a different shape'::TITLE
FROM fm_fill
WHERE delivery_date BETWEEN fm_day() - 13 AND fm_day()
ORDER BY CASE column_name WHEN 'stock_level' THEN 9 WHEN 'promo_flag' THEN 10 ELSE 1 END, column_name, delivery_date;

SELECT 5::COL;
SELECT 'schema_check against the contract — today vs. yesterday'::TITLE,
       delivery AS "File" ::TABLE, status AS "Status" ::BADGE, missing_columns AS "Missing", coalesce(new_columns, '—') AS "New since yesterday"
FROM fm_schema ORDER BY delivery;

SELECT 7::COL;
SELECT 'Empty today > 2'::FLAG;
SELECT 'Column profile — today vs. yesterday (profile_table)'::TITLE,
       column_name AS "Column" ::TABLE,
       100 * max(null_rate) FILTER (WHERE delivery = 'yesterday') AS "Empty yesterday" ::PERCENT,
       100 * max(null_rate) FILTER (WHERE delivery = 'today') AS "Empty today" ::PERCENT,
       max(distinct_count) FILTER (WHERE delivery = 'today') AS "Distinct today" ::PLAIN,
       max(column_type) AS "Type"
FROM fm_profile GROUP BY column_name
ORDER BY max(null_rate) FILTER (WHERE delivery = 'today') DESC NULLS FIRST, column_name;

-- ── 3 · Volume & prices ──────────────────────────────────────────────────────
SELECT 'Volume & prices'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** `rel_count_change` compares each file''s row count with the mean of the 7 nights before — the 29 Aug file is cut in half. `metric_anomaly_iqr` checks the mean price against its 28-day spread and catches the cents bug (29 Sep – 1 Oct). The 8 Oct re-send passes both: only the price timestamp gives it away, and the number of price changes drops to zero.'::MARKDOWN;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT d.delivery_date ::XAXIS,
       CASE WHEN e.verdict = 'fail' THEN 'outside ±30 % of the 7-day mean' ELSE 'normal' END ::CATEGORY,
       d.row_count ::BARCHART_STACKED, 1000 ::REFLINE,
       'Rows per delivery — one cut-off file; the floor is 1,000'::TITLE
FROM fm_deliveries d LEFT JOIN fm_eval e ON e.delivery_date = d.delivery_date AND e.check_type = 'rel_count_change'
WHERE fm_days(d.delivery_date) ORDER BY d.delivery_date;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT d.delivery_date ::XAXIS,
       CASE WHEN e.verdict IN ('fail', 'warn') THEN 'outside the IQR band' ELSE 'normal' END ::CATEGORY,
       d.mean_price ::BARCHART_STACKED, '€'::YFORMAT, 'log10'::YSCALE,
       'Mean price per delivery (log scale) — the cents bug, and today''s lost decimal points'::TITLE
FROM fm_deliveries d LEFT JOIN fm_eval e ON e.delivery_date = d.delivery_date AND e.check_type = 'metric_anomaly_iqr'
WHERE fm_days(d.delivery_date) ORDER BY d.delivery_date;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT delivery_date ::XAXIS, price_changes ::BARCHART,
       'Price changes vs. the night before — none on 8 Oct: a re-sent file'::TITLE
FROM fm_deliveries WHERE fm_days(delivery_date) AND delivery_date > DATE '2026-08-17' ORDER BY delivery_date;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT CASE WHEN change_pct <= -90 THEN '÷100' WHEN change_pct < -5 THEN '−12…−5 %' WHEN change_pct < 0 THEN '−5…0 %'
            WHEN change_pct < 5 THEN '0…+5 %' WHEN change_pct < 20 THEN '+5…+15 %' ELSE '×100' END ::XAXIS,
       CASE WHEN anomaly_score >= fm_cut() THEN 'unusual (score ≥ cut-off)' ELSE 'normal' END ::CATEGORY,
       count(*) ::BARCHART_STACKED, 'log10'::YSCALE,
       'Price moves by size (log count) — the forest isolates the ×100 jumps'::TITLE
FROM fm_price_moves WHERE fm_days(delivery_date)
GROUP BY ALL ORDER BY min(change_pct), 2;

SELECT 12::COL;
SELECT 'none'::TOTALS;
SELECT 'Unusual price moves in the selected delivery (isolation_forest score ≥ cut-off)'::TITLE,
       sku AS "SKU" ::TABLE, category AS "Category", price_before AS "Before (€)" ::PLAIN, price_after AS "After (€)" ::PLAIN,
       currency AS "Currency", change_pct AS "Change %" ::PLAIN, round(100 * anomaly_score) AS "Score" ::PLAIN
FROM fm_price_moves WHERE delivery_date = fm_day() AND anomaly_score >= fm_cut()
ORDER BY anomaly_score DESC, sku;

-- ── 4 · Go / no-go: the decision for the selected delivery ───────────────────
SELECT 'Go / no-go'::TAB;

SELECT 12::COL;
SELECT (CASE (SELECT decision FROM fm_verdict WHERE delivery_date = fm_day())
            WHEN 'NO-GO' THEN '### Hold the load. '
            WHEN 'GO with warnings' THEN '### Load, and tell the supplier. '
            ELSE '### Load. ' END
        || 'The ' || strftime(fm_day(), '%d %b') || ' file fails ' || (SELECT blocking FROM fm_verdict WHERE delivery_date = fm_day())
        || ' blocking and ' || (SELECT warnings FROM fm_verdict WHERE delivery_date = fm_day()) || ' warning checks. '
        || CASE WHEN (SELECT decision FROM fm_verdict WHERE delivery_date = fm_day()) = 'NO-GO'
                THEN 'Keep yesterday''s prices online, send the failing checks below to the supplier and ask for a corrected file.'
                WHEN (SELECT decision FROM fm_verdict WHERE delivery_date = fm_day()) = 'GO with warnings'
                THEN 'Nothing blocks the load; the warnings go to the supplier''s weekly review.'
                ELSE 'Every check of the contract passes.' END)::MARKDOWN;

SELECT 12::COL;
SELECT 'Status = fail, Status = warn'::FLAG;
SELECT 'none'::TOTALS;
SELECT 'The data contract, check by check, for the selected delivery'::TITLE,
       category AS "Category" ::TABLE, check_name AS "Check", severity AS "Severity", verdict AS "Status" ::BADGE,
       fm_fmt(check_type, value) AS "Measured",
       fm_fmt(check_type, lower_threshold) AS "Min",
       fm_fmt(check_type, CASE WHEN category = 'Completeness' AND check_type = 'null_rate' THEN fm_limit() ELSE upper_threshold END) AS "Max",
       check_type AS "anofox_tabular check"
FROM fm_eval WHERE delivery_date = fm_day()
ORDER BY CASE verdict WHEN 'fail' THEN 0 WHEN 'warn' THEN 1 ELSE 2 END, severity, category, check_name;
