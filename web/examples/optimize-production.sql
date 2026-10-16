-- Production scheduling with changeovers — anofox_optimize on a bottler's week.
--
-- STORED RESULTS, NOT LIVE. The schedules below were computed natively with
-- the anofox_optimize extension (examples/optimize-production/compute.sql,
-- written to web/data/optimize-production/*.csv by
-- scripts/gen-optimize-cases.sh production). Its DuckDB-Wasm build doesn't
-- load yet (see examples/optimize-showcase/README.md), so this dashboard only
-- reads the stored runs: the inputs pick among stored scenarios and rules,
-- nothing is re-solved in the browser.
--
-- The question: in which order should the three filling lines run week 43's
-- 42 orders so the trucks leave on time and the lines spend the least time on
-- cleaning and format changes? Asked by the production planner on Friday,
-- before the plan is released to the shift leaders.
--
-- Tabs: Overview · Run plan · Orders · Changeovers · Scenarios · Decision.
-- Natively, run from web/:  dashboard --check ../examples/optimize-production/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_optimize'::BRAND;
SELECT 'Which run order gets week 43''s orders out on time with the least changeover?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_optimize runs (CSV) ──────────────────────────────
CREATE OR REPLACE TABLE pp_orders AS FROM read_csv('data/optimize-production/pp_orders.csv');
CREATE OR REPLACE TABLE pp_scenarios AS FROM read_csv('data/optimize-production/pp_scenarios.csv');
CREATE OR REPLACE TABLE pp_algos AS FROM read_csv('data/optimize-production/pp_algos.csv');
CREATE OR REPLACE TABLE pp_gantt AS FROM read_csv('data/optimize-production/pp_gantt.csv');
CREATE OR REPLACE TABLE pp_matrix AS FROM read_csv('data/optimize-production/pp_matrix.csv');
CREATE OR REPLACE TABLE pp_provenance AS FROM read_csv('data/optimize-production/provenance.csv');

-- The inputs (each picks a stored run) and helpers.
CREATE OR REPLACE MACRO pp_vol() AS coalesce(getvariable('volume'), 'as booked');
CREATE OR REPLACE MACRO pp_co() AS coalesce(getvariable('changeovers'), 'today''s changeovers');
CREATE OR REPLACE MACRO pp_rule() AS coalesce(getvariable('rule'), 'exact (proven optimum)');
CREATE OR REPLACE MACRO pp_line() AS coalesce(getvariable('line'), 'L2 PET');
CREATE OR REPLACE MACRO pp_today() AS 'due date order (today)';
CREATE OR REPLACE MACRO pp_clock(m) AS strftime(TIMESTAMP '2026-10-19 06:00' + to_minutes(m::BIGINT), '%a %H:%M');
-- The runs of the chosen scenario.
CREATE OR REPLACE MACRO pp_run() AS TABLE
    SELECT * FROM pp_gantt WHERE volume = pp_vol() AND changeovers = pp_co();
CREATE OR REPLACE MACRO pp_kpi() AS TABLE
    SELECT * FROM pp_algos WHERE volume = pp_vol() AND changeovers = pp_co();

-- ── Header: the question, who asks it, inputs, KPIs ──────────────────────────
SELECT 12::COL;
SELECT ('**The production planner** of a soft-drinks bottler releases next week''s plan on Friday: 42 orders on three filling lines (glass, PET, cans), each with a truck slot; a quarter are key-account promotions that must not miss their truck. Today the lines run in due-date order — simple, but every flavour or bottle change costs a rinse, a full clean or a format change. The `opt_schedule_…` functions sequence each line against due dates and changeovers at once.')::MARKDOWN;

SELECT 'Planning inputs'::GROUP;
SELECT rule ::DROPDOWN, 'Sequencing rule (vs. today''s due-date order)'::LABEL
FROM (SELECT DISTINCT rule, ord FROM pp_algos WHERE rule <> pp_today()) ORDER BY ord = 6, ord DESC;
SELECT volume ::DROPDOWN, 'Demand scenario'::LABEL
FROM (SELECT DISTINCT volume, volume_factor FROM pp_scenarios) ORDER BY volume_factor = 1 DESC, volume_factor;
SELECT changeovers ::DROPDOWN, 'Changeover times'::LABEL
FROM (SELECT DISTINCT changeovers, changeover_factor FROM pp_scenarios) ORDER BY changeover_factor DESC;
SELECT line ::BUTTONS, 'Line to drill into'::LABEL
FROM (SELECT line, sum(weighted_late_min) AS wl FROM pp_algos WHERE rule = pp_today() AND volume = 'as booked' AND changeovers = 'today''s changeovers' GROUP BY line)
ORDER BY wl DESC;
SELECT 1::ENDGROUP;

SELECT 'Selected rule vs. today''s due-date order, all three lines'::GROUP;
SELECT (42 - sum(late_orders) FILTER (WHERE rule = pp_rule()))::METRIC,
       (42 - sum(late_orders) FILTER (WHERE rule = pp_today()))::DELTA, 'abs'::DELTA_FORMAT, 'vs today'::DELTA_LABEL,
       '<42:warn; good'::THRESHOLDS,
       'Orders on their truck (of 42)'::LABEL FROM pp_kpi();
-- Promotions must not miss their truck: any key-account order late is red.
SELECT count(*) FILTER (WHERE rule = pp_rule() AND weight > 1 AND late_min = 0)::METRIC,
       count(*) FILTER (WHERE rule = pp_today() AND weight > 1 AND late_min = 0)::DELTA, 'abs'::DELTA_FORMAT, 'vs today'::DELTA_LABEL,
       '<13:bad; good'::THRESHOLDS,
       'Key-account orders on time (of 13)'::LABEL FROM pp_run();
SELECT round(100 * sum(end_min - start_min) FILTER (WHERE rule = pp_rule()) / sum(end_min - start_min + changeover_min) FILTER (WHERE rule = pp_rule()), 1)::PERCENT,
       round(100 * sum(end_min - start_min) FILTER (WHERE rule = pp_today()) / sum(end_min - start_min + changeover_min) FILTER (WHERE rule = pp_today()), 1)::DELTA,
       'abs'::DELTA_FORMAT, 'vs today'::DELTA_LABEL,
       'Line time spent filling (not changing over)'::LABEL FROM pp_run();
SELECT round(sum(changeover_min) FILTER (WHERE rule = pp_today()) / 60 - sum(changeover_min) FILTER (WHERE rule = pp_rule()) / 60, 1)::METRIC,
       'Changeover hours saved vs. today'::LABEL FROM pp_kpi();
SELECT round(sum(7200 - makespan_min) FILTER (WHERE rule = pp_rule()) / 60, 1)::METRIC,
       round(sum(7200 - makespan_min) FILTER (WHERE rule = pp_today()) / 60, 1)::DELTA, 'vs today'::DELTA_LABEL,
       'Line hours left before Sat 06:00'::LABEL FROM pp_kpi();
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT rule ::XAXIS, line ::CATEGORY, round(weighted_late_min / 60, 1) ::BARCHART_STACKED, 1 ::FLIP, ' h'::YFORMAT, 'tween'::TRANSITION,
       'Minutes each order misses its truck, key-account orders counted three times, summed per line.'::INFO,
       'Weighted lateness by rule (h) — the exact schedule is the proven optimum, "fewest changeovers" ignores the trucks'::TITLE
FROM pp_kpi() ORDER BY sum(weighted_late_min) OVER (PARTITION BY rule) DESC, line;

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT round(sum(changeover_min) / 60, 1) ::XAXIS, sum(late_orders) ::SCATTER,
       replace(replace(rule, ' (proven optimum)', ''), 'due date order (today)', 'today') ::CATEGORY,
       ' h'::XFORMAT, 'tween'::TRANSITION,
       'The trade-off: changeover hours (x) vs. orders late (y) — down and left is better'::TITLE
FROM pp_kpi() GROUP BY rule ORDER BY rule;

SELECT 12::COL;
SELECT 'sum'::TOTALS;
SELECT 'Late: today, selected | Changeover hours: co_today, co_selected'::COLGROUP;
SELECT 'selected > 0'::FLAG;
SELECT line AS "Line" ::TABLE,
       '#Run plan?v.line={value}' AS "Run plan" ::LINK,
       sum(late_orders) FILTER (WHERE rule = pp_today()) AS today,
       sum(late_orders) FILTER (WHERE rule = pp_rule()) AS selected,
       round(sum(changeover_min) FILTER (WHERE rule = pp_today()) / 60, 1) AS co_today,
       round(sum(changeover_min) FILTER (WHERE rule = pp_rule()) / 60, 1) AS co_selected,
       round(sum(weighted_late_min) FILTER (WHERE rule = pp_today()) / 60, 1) AS "Weighted late today (h)",
       round(sum(weighted_late_min) FILTER (WHERE rule = pp_rule()) / 60, 1) AS "Weighted late, selected (h)",
       ('Per line: today''s order vs. ' || pp_rule())::TITLE
FROM pp_kpi() GROUP BY line ORDER BY line;

-- ── 2 · Run plan (one line) ──────────────────────────────────────────────────
SELECT 'Run plan'::TAB;

SELECT 12::COL;
SELECT ('**' || pp_line() || '**, ' || pp_vol() || ', ' || pp_co() || '. Each bar is an order from start to finish (hours after Mon 06:00), the short bar before it the changeover; dashed lines mark midnight. Today''s due-date order (top) pays for a changeover at almost every step; **' || pp_rule() || '** (bottom) runs related flavours back to back wherever the truck slots allow.')::MARKDOWN;

SELECT 12::COL;
SELECT 520::HEIGHT;
WITH g AS (SELECT * FROM pp_run() WHERE line = pp_line() AND rule IN (pp_today(), pp_rule()))
SELECT round((s + e) / 120, 2) ::XAXIS, pos ::SCATTER, round(s / 60, 2) ::XMIN, round(e / 60, 2) ::XMAX,
       status ::CATEGORY, plan ::FACET, 1 ::FACET_NCOL, 'reverse'::YSCALE,
       18 + 24 * ((pos - 1) % 5) ::XLINE,
       'Run plan: x = hours after Mon 06:00, y = position in the sequence'::TITLE
FROM (
    SELECT CASE WHEN rule = pp_today() THEN '1 today: due-date order' ELSE '2 ' || rule END AS plan, pos,
           start_min AS s, end_min AS e,
           CASE WHEN late_min > 0 THEN 'order late' ELSE 'order on time' END AS status FROM g
    UNION ALL
    SELECT CASE WHEN rule = pp_today() THEN '1 today: due-date order' ELSE '2 ' || rule END, pos,
           start_min - changeover_min, start_min, 'changeover' FROM g WHERE changeover_min > 0
) ORDER BY plan, status, pos;

SELECT 12::COL;
SELECT 'late_min > 0'::FLAG;
SELECT 'Changeover min: top 3'::HIGHLIGHT;
SELECT 'sum'::TOTALS;
SELECT pos AS "#" ::PLAIN, order_id AS "Order" ::TABLE, product AS "Product", customer AS "Customer",
       CASE WHEN weight > 1 THEN 'key account' ELSE 'standard' END AS "Priority" ::BADGE,
       changeover_type AS "Changeover before", changeover_min AS "Changeover min",
       pp_clock(start_min) AS "Start" ::PLAIN, pp_clock(end_min) AS "Finish" ::PLAIN, pp_clock(due_min) AS "Truck" ::PLAIN,
       late_min,
       (pp_line() || ': the sequence of ' || pp_rule())::TITLE
FROM pp_run() WHERE line = pp_line() AND rule = pp_rule() ORDER BY pos;

-- ── 3 · Orders (all lines) ───────────────────────────────────────────────────
SELECT 'Orders'::TAB;

SELECT 12::COL;
SELECT '**All 42 orders**, today vs. the selected rule, subtotalled by line. A ⚠ marks an order that misses its truck under the selected rule; key-account orders weigh three times in the objective, so the optimiser lets a standard order slip before a promotion.'::MARKDOWN;

SELECT 12::COL;
SELECT 'line'::SUBTOTAL;
SELECT 'late_selected > 0'::FLAG;
SELECT 'late_today: sum, late_selected: sum, Volume (k bottles): sum'::TOTALS;
SELECT 'Truck slot: due | Minutes late: late_today, late_selected'::COLGROUP;
SELECT t.order_id AS "Order" ::TABLE, t.line, t.product AS "Product", t.customer AS "Customer",
       CASE WHEN t.weight > 1 THEN 'key account' ELSE 'standard' END AS "Priority" ::BADGE,
       t.volume_kbtl AS "Volume (k bottles)",
       pp_clock(t.due_min) AS due ::PLAIN,
       t.late_min AS late_today ::COLORSCALE, s.late_min AS late_selected ::COLORSCALE,
       CASE WHEN s.late_min = 0 AND t.late_min > 0 THEN 'ok: rescued'
            WHEN s.late_min = 0 THEN 'ok: on time'
            WHEN s.late_min < t.late_min THEN 'late, but earlier'
            ELSE 'late' END AS "Selected rule" ::BADGE,
       ('Order by order: today vs. ' || pp_rule())::TITLE
FROM pp_run() t JOIN pp_run() s ON s.order_id = t.order_id AND s.rule = pp_rule()
WHERE t.rule = pp_today()
ORDER BY t.line, t.due_min, t.order_id;

-- ── 4 · Changeovers ──────────────────────────────────────────────────────────
SELECT 'Changeovers'::TAB;

SELECT 12::COL;
SELECT ('**Why order matters on ' || pp_line() || ':** a lighter flavour after a heavier one needs a full clean (80 min), a new bottle format 45 min more, sugar-free its own rinse; the next lot of the same product only 10 min. The selected rule saves time mainly by avoiding full cleans.')::MARKDOWN;

SELECT 6::COL;
SELECT 420::HEIGHT;
SELECT kind ::XAXIS, plan ::CATEGORY, h ::BARCHART, 1 ::FLIP, ' h'::YFORMAT,
       'Changeover hours by kind, all lines — full cleans are what today''s order pays for'::TITLE
FROM (SELECT changeover_type AS kind, CASE WHEN rule = pp_today() THEN 'today (due-date order)' ELSE 'selected rule' END AS plan,
             round(sum(changeover_min) / 60, 1) AS h
      FROM pp_run() WHERE rule IN (pp_today(), pp_rule()) GROUP BY 1, 2) a
JOIN (SELECT changeover_type AS kind, sum(changeover_min) AS tot FROM pp_run() WHERE rule = pp_today() GROUP BY 1) b USING (kind)
ORDER BY tot, plan;

SELECT 6::COL;
SELECT 420::HEIGHT;
SELECT to_product ::XAXIS, from_product ::YAXIS, minutes ::HEATMAP,
       (pp_line() || ': changeover minutes from row to column (light → heavy flavours)')::TITLE
FROM pp_matrix WHERE line = pp_line() ORDER BY from_ord, to_ord;

-- ── 5 · Scenarios ────────────────────────────────────────────────────────────
SELECT 'Scenarios'::TAB;

SELECT 12::COL;
SELECT '**What if demand moves?** Every rule was solved for four demand scenarios and two changeover settings (today''s times, and the SMED / fast-cleaning project that cuts them by 40 %). The heatmap reads the stored runs at the changeover setting chosen above; the lines compare today''s order with the selected rule for both settings.'::MARKDOWN;

SELECT 7::COL;
SELECT 380::HEIGHT;
SELECT volume ::XAXIS, rule ::YAXIS, sum(late_orders) ::HEATMAP, ''::DATALABELS,
       ('Orders late by rule and demand (' || pp_co() || ') — darker = more trucks missed')::TITLE
FROM pp_algos a JOIN pp_scenarios s USING (volume, changeovers)
WHERE changeovers = pp_co() GROUP BY ALL ORDER BY any_value(a.ord), any_value(s.volume_factor);

SELECT 5::COL;
SELECT 380::HEIGHT;
SELECT round(100 * s.volume_factor) ::XAXIS,
       CASE WHEN a.rule = pp_today() THEN 'today' ELSE 'selected' END || CASE WHEN s.changeover_factor < 1 THEN ' + SMED' ELSE '' END ::CATEGORY,
       round(sum(a.weighted_late_min) / 60, 1) ::LINECHART, ' h'::YFORMAT, 100 ::XLINE,
       'Weighted lateness (h) vs. demand (% of booked) — SMED buys the heatwave'::TITLE
FROM pp_algos a JOIN pp_scenarios s USING (volume, changeovers)
WHERE a.rule IN (pp_today(), pp_rule()) GROUP BY ALL ORDER BY 2, 1;

-- ── 6 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

-- The rule with the least weighted lateness at the chosen scenario (ties: fewer changeovers).
CREATE OR REPLACE MACRO pp_best() AS (
    SELECT rule FROM pp_kpi() GROUP BY rule ORDER BY sum(weighted_late_min), sum(changeover_min), rule LIMIT 1);

SELECT 12::COL;
WITH k AS (
    SELECT sum(late_orders) FILTER (WHERE rule = pp_today()) AS lt, sum(late_orders) FILTER (WHERE rule = pp_best()) AS lb,
           sum(changeover_min) FILTER (WHERE rule = pp_today()) / 60 AS ct, sum(changeover_min) FILTER (WHERE rule = pp_best()) / 60 AS cb,
           sum(late_orders) FILTER (WHERE rule = 'fewest changeovers') AS lf
    FROM pp_kpi()
), h AS (
    SELECT sum(late_orders) FILTER (WHERE changeovers = 'today''s changeovers') AS heat,
           sum(late_orders) FILTER (WHERE changeovers <> 'today''s changeovers') AS heat_smed
    FROM pp_algos WHERE rule = 'exact (proven optimum)' AND volume = '+10 % (heatwave)'
)
SELECT ('### Recommendation' || chr(10) || chr(10) ||
        '**Release the ' || pp_best() || ' plan** (' || pp_vol() || ', ' || pp_co() || '): **' || lb || '** of 42 orders miss their truck instead of **' || lt ||
        '** with today''s due-date order, and the lines spend **' || round(cb, 1) || ' h** on changeovers instead of **' || round(ct, 1) || ' h** (' || round(100 * (1 - cb / ct)) || ' % less). ' ||
        'Don''t chase changeovers alone: "fewest changeovers" cleans least but leaves **' || lf || '** orders late.' || chr(10) || chr(10) ||
        '**If the heatwave comes (+10 %),** even the proven-optimal sequence leaves **' || heat || '** orders late at today''s changeover times; with the SMED project''s faster changeovers it would be **' || heat_smed ||
        '** — the case for funding it. Below: the late orders of the released plan, and whom to call.')::MARKDOWN
FROM k, h;

SELECT 12::COL;
SELECT 'late_min > 60'::FLAG;
SELECT 'sum'::TOTALS;
SELECT order_id AS "Order" ::TABLE, line AS "Line", customer AS "Customer to call",
       CASE WHEN weight > 1 THEN 'key account' ELSE 'standard' END AS "Priority" ::BADGE,
       pp_clock(due_min) AS "Truck" ::PLAIN, pp_clock(end_min) AS "Ready" ::PLAIN, late_min,
       ('Late orders in the released plan (' || pp_best() || ', ' || pp_vol() || ')')::TITLE
FROM pp_run() WHERE rule = pp_best() AND late_min > 0
ORDER BY late_min DESC;
