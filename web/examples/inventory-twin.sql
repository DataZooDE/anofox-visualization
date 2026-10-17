-- Inventory digital twin: stock, risk, monitoring, calibration and replay of
-- one twin run of inventory-rs (DataZooDE/inventory-rs, private).
--
-- STORED RESULTS, NOT LIVE. inventory-rs is a Rust library; its demo run
-- (end_to_end_demo: ingest -> build-twin -> calibrate -> a 120-period event
-- stream -> replay-history, then a projection) writes long export tables that
-- this dashboard reads from web/data/inventory-twin/*.csv
-- (scripts/gen-inventory-cases.sh). Until that export exists in inventory-rs
-- (DataZooDE/inventory-rs#22, #23, #26, #27, #28) the tables are SYNTHETIC:
-- examples/inventory-twin/synthetic.py writes them in exactly the specified
-- schemas, seed 42, 12 made-up SKUs. Provenance (source, inventory-rs
-- version, git sha, schema and export version, seed) is in
-- web/data/inventory-twin/provenance.csv. Once the real export lands, the
-- script writes the same files and this SQL stays unchanged.
--
-- The question: which items will run short in the next four weeks, and can
-- the plan behind that answer be trusted? Asked by the replenishment planner
-- of a wholesaler who runs a twin per item: it steps each item's stock with
-- every sale and delivery, projects the next four weeks, watches its own
-- forecast for drift and recalibrates itself.
--
-- Tabs: Overview · Stock & pipeline · Risk · Monitoring · Calibration · Replay · Pipeline.
-- Natively, run from web/:  dashboard --check ../examples/inventory-twin/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'inventory-rs digital twin'::BRAND;
SELECT 'Where will stock run short, and can we trust the plan?'::DASHBOARD_TITLE;

-- ── Data: the export tables (CSV) ────────────────────────────────────────────
CREATE OR REPLACE TABLE it_trajectory AS FROM read_csv('data/inventory-twin/trajectory.csv');
CREATE OR REPLACE TABLE it_projection AS FROM read_csv('data/inventory-twin/projection.csv');
CREATE OR REPLACE TABLE it_alarms AS FROM read_csv('data/inventory-twin/alarms.csv');
CREATE OR REPLACE TABLE it_changes AS FROM read_csv('data/inventory-twin/parameter_changes.csv');
CREATE OR REPLACE TABLE it_proposals AS FROM read_csv('data/inventory-twin/proposals.csv');
CREATE OR REPLACE TABLE it_status AS FROM read_csv('data/inventory-twin/detector_status.csv');
CREATE OR REPLACE TABLE it_trace AS FROM read_csv('data/inventory-twin/detector_trace.csv');
CREATE OR REPLACE TABLE it_replay AS FROM read_csv('data/inventory-twin/replay_deviation.csv');
CREATE OR REPLACE TABLE it_manifest AS FROM read_csv('data/inventory-twin/run_manifest.csv');
CREATE OR REPLACE TABLE it_edges AS FROM read_csv('data/inventory-twin/run_edges.csv');

-- One row per item: its class (cohort) and demand shape, from the alarms'
-- and the trace's columns (every twin has a trace).
CREATE OR REPLACE TABLE it_items AS
SELECT sku, any_value(cohort) AS cohort, any_value(shape) AS shape FROM it_trace GROUP BY sku;

-- The calendar: periods are days from the demo's anchor, 2024-01-01.
CREATE OR REPLACE MACRO it_ds(p) AS (DATE '2024-01-01' + p::INTEGER);
CREATE OR REPLACE MACRO it_today() AS (SELECT max(ds) FROM it_trajectory);

-- The inputs.
CREATE OR REPLACE MACRO it_sku() AS coalesce(getvariable('sku'), 'SKU-004');
CREATE OR REPLACE MACRO it_level_txt() AS coalesce(getvariable('plan_level'), '90 %');
CREATE OR REPLACE MACRO it_level() AS replace(it_level_txt(), ' %', '')::INTEGER / 100;
-- The lower edge of the plan band is the (1 - level) / 2 quantile: at the
-- 90 % level, the 5 % quantile. When it reaches zero, the chance of running
-- out that day is at least that tail.
CREATE OR REPLACE MACRO it_tail() AS (1 - it_level()) / 2;
CREATE OR REPLACE MACRO it_detector(n) AS
    CASE n WHEN 'cusum' THEN 'CUSUM' WHEN 'page_hinkley' THEN 'Page-Hinkley'
           WHEN 'occurrence_cusum' THEN 'Occurrence CUSUM' WHEN 'psi' THEN 'PSI'
           WHEN 'ks' THEN 'KS' WHEN 'mae' THEN 'MAE' ELSE n END;
CREATE OR REPLACE MACRO it_param(p) AS
    CASE p WHEN 'supplier.lead_time_mean' THEN 'Lead time (days)'
           WHEN 'supplier.lead_time_std_dev' THEN 'Lead-time spread (days)'
           WHEN 'demand_calibration.sigma_multiplier' THEN 'Demand spread (× model)'
           WHEN 'demand_calibration.bias' THEN 'Demand bias' ELSE p END;

-- Per item over the run, and its risk over the projection at the plan level.
CREATE OR REPLACE MACRO it_runs() AS TABLE
SELECT sku, sum(demand) AS demand, sum(fulfilled) AS served, sum(lost) AS lost,
       count(*) FILTER (WHERE on_hand_end = 0) AS days_out,
       arg_max(on_hand_end, period) AS on_hand_now, arg_max(on_order_end, period) AS on_order_now,
       count(*) FILTER (WHERE order_qty > 0) AS orders
FROM it_trajectory GROUP BY sku;
CREATE OR REPLACE MACRO it_risk() AS TABLE
SELECT model_id AS sku,
       min(ds) FILTER (WHERE yhat_lower = 0) AS short_by,
       max(cumulative_stockout_probability) AS p_out_4w,
       sum(expected_unmet) AS units_short,
       1 - product(1 - order_probability) FILTER (WHERE period <= (SELECT max(period) FROM it_trajectory) + 7) AS p_order_7d
FROM it_projection WHERE level = it_level() GROUP BY model_id;

-- Proposals waiting for a decision: the latest proposal per item and
-- parameter, when it was escalated or proposed and not approved since.
CREATE OR REPLACE MACRO it_waiting() AS TABLE
SELECT c.*, p.expected_rop_delta FROM it_changes c JOIN it_proposals p ON p.id = c.proposal_id
WHERE c.status IN ('escalated', 'proposed')
  AND c.proposal_id NOT IN (SELECT proposal_id FROM it_changes WHERE status = 'approved')
  AND c.period = (SELECT max(period) FROM it_changes x WHERE x.sku = c.sku AND x.parameter = c.parameter AND x.origin = 'calibration');

-- Criticality, written down once. Per item at the plan level:
--   Critical  a chance of 50 % or more of at least one day out of stock in
--             the next 4 weeks, and it can run out within 7 days (the plan
--             band's lower edge reaches zero) or it is a class A item (a top
--             seller: the export has no prices, the class stands for value);
--   Watch     a chance of 20 % or more (the Risk tab's thresholds);
--   OK        below 20 %.
-- Most critical first: status, then the day it can run out, then the chance.
-- Trust is separate: 'alarm' when a detector is latched in alarm now,
-- 'review' when a stock count did not match the twin or a recalibration needs
-- the planner (escalated, or any waiting for a class A / B item; class C
-- proposals always wait by design and stay routine), else 'ok'.
CREATE OR REPLACE MACRO it_crit() AS TABLE
WITH base AS (
    SELECT r.sku, i.cohort, i.shape, r.p_out_4w, r.short_by, r.units_short,
           (r.short_by - it_today())::INTEGER AS days_left,
           n.on_hand_now, n.on_order_now,
           n.on_hand_now / nullif((SELECT avg(demand) FROM it_trajectory t WHERE t.sku = r.sku
                                   AND t.period > (SELECT max(period) FROM it_trajectory) - 28), 0) AS cover_days,
           (SELECT string_agg(DISTINCT it_detector(test), ', ') FROM it_status s WHERE s.sku = r.sku AND s.state = 'alarm') AS alarm_now,
           -- Class C items never change on their own: their proposals wait for
           -- the planner by design (routine). Escalated ones, and any waiting
           -- for class A / B, need a decision.
           (SELECT count(*) FROM it_waiting() w WHERE w.sku = r.sku AND (w.status = 'escalated' OR i.cohort IN ('A', 'B'))) AS waiting,
           (SELECT count(*) FROM it_waiting() w WHERE w.sku = r.sku) AS waiting_all,
           (SELECT count(*) FROM it_replay x WHERE x.sku = r.sku) AS counts,
           (SELECT count(*) FILTER (WHERE NOT within_tolerance) FROM it_replay x WHERE x.sku = r.sku) AS counts_missed
    FROM it_risk() r JOIN it_items i USING (sku) JOIN it_runs() n USING (sku)),
graded AS (
    SELECT *,
           CASE WHEN p_out_4w >= 0.5 AND (days_left <= 7 OR cohort = 'A') THEN 'Critical'
                WHEN p_out_4w >= 0.2 THEN 'Watch' ELSE 'OK' END AS status,
           CASE WHEN alarm_now IS NOT NULL THEN 'alarm'
                WHEN waiting > 0 OR counts_missed > 0 THEN 'review' ELSE 'ok' END AS trust,
           concat_ws(' · ', CASE WHEN alarm_now IS NOT NULL THEN alarm_now || ' alarm' END,
                     CASE WHEN waiting > 0 THEN waiting || ' waiting' END,
                     CASE WHEN waiting_all > waiting THEN (waiting_all - waiting) || ' routine (class C)' END,
                     CASE WHEN counts_missed > 0 THEN counts_missed || '/' || counts || ' counts off' END) AS trust_why
    FROM base)
SELECT *, CASE status WHEN 'Critical' THEN 1 WHEN 'Watch' THEN 2 ELSE 3 END AS grade,
       row_number() OVER (ORDER BY CASE status WHEN 'Critical' THEN 1 WHEN 'Watch' THEN 2 ELSE 3 END,
                                   short_by NULLS LAST, p_out_4w DESC, sku) AS rnk
FROM graded;

-- The selected item's incident, step by step (the story on the stock chart):
-- the first warning and alarm, the longest stockout, the first escalated
-- calibration and the first approval. Any item gets its own story.
CREATE OR REPLACE MACRO it_story() AS TABLE
WITH out_runs AS (
    SELECT period, period - row_number() OVER (ORDER BY period) AS grp
    FROM it_trajectory WHERE sku = it_sku() AND on_hand_end = 0),
spell AS (
    SELECT min(period) AS p0, max(period) AS p1, count(*) AS days FROM out_runs GROUP BY grp
    ORDER BY days DESC, p0 LIMIT 1),
steps AS (
    FROM (SELECT period, 1 AS k, 'Early warning: ' || it_detector(name) || ' on the forecast error passes its warning line — demand runs above the forecast' AS caption
          FROM it_alarms WHERE sku = it_sku() AND tier = 'warning' AND stream = 'forecast_error' ORDER BY period LIMIT 1)
    UNION ALL
    FROM (SELECT period, 2, 'Alarm: ' || it_detector(name) || ' crosses its alarm line; the alarm triggers a calibration run'
          FROM it_alarms WHERE sku = it_sku() AND tier = 'alarm' ORDER BY period LIMIT 1)
    UNION ALL
    SELECT p0, 3, 'Out of stock for ' || days || ' days; ' || (SELECT sum(lost) FROM it_trajectory WHERE sku = it_sku() AND period BETWEEN p0 AND p1) || ' units of demand lost'
    FROM spell
    UNION ALL
    FROM (SELECT period, 4, 'The twin proposes ' || lower(it_param(parameter)) || ' ' || before_num || ' → ' || after_num || ': too large to apply on its own, sent to the planner'
          FROM it_changes WHERE sku = it_sku() AND status = 'escalated'
            AND period >= coalesce((SELECT min(period) FROM it_alarms WHERE sku = it_sku() AND tier = 'alarm'), 0)
          ORDER BY period LIMIT 1)
    UNION ALL
    FROM (SELECT period, 5, 'The planner approves it; the reorder point rises and the next order goes out earlier'
          FROM it_changes WHERE sku = it_sku() AND status = 'approved' ORDER BY period LIMIT 1))
SELECT it_ds(period) AS ds, string_agg(caption, ' · ' ORDER BY k) AS caption FROM steps GROUP BY period;

-- ── Header: the question, inputs, the fleet at a glance ─────────────────────
SELECT 12::COL;
SELECT ('**The replenishment planner** of a wholesaler runs a digital twin for each of ' || (SELECT count(*) FROM it_items) ||
        ' items: it replays every sale and delivery, projects the stock four weeks ahead, watches its own forecast for drift and recalibrates itself. ' ||
        'It is the evening of **' || strftime(it_today(), '%-d %b %Y') || '**. Which items will run short before the next deliveries, and is the twin behind that answer still right? ' ||
        '**Answer:** ' || coalesce((SELECT CASE count(*) WHEN 1 THEN '1 item is' ELSE count(*) || ' items are' END || ' critical — ' || string_agg('**' || sku || '**', ', ' ORDER BY rnk) FROM it_crit() WHERE status = 'Critical'), 'no item is critical') ||
        ' — likely to run out in the next four weeks, and soon or a top seller (tab *Overview*); ' ||
        (SELECT CASE count(DISTINCT sku) WHEN 1 THEN '1 twin is' ELSE count(DISTINCT sku) || ' twins are' END FROM it_status WHERE state = 'alarm') || ' in alarm now (*Monitoring*); ' ||
        (SELECT count(*) FROM it_waiting()) ||
        ' recalibrations wait for a decision (*Calibration*); the twins match ' ||
        (SELECT round(100 * avg(within_tolerance::INTEGER))::INTEGER FROM it_replay) || ' % of the stock counts (*Replay*).')::MARKDOWN;

SELECT 'Inputs'::GROUP;
SELECT i.sku AS sku ::DROPDOWN, 'Item'::LABEL,
       'class ' || i.cohort || ', ' || i.shape || ' demand' ::HINT
FROM it_items i ORDER BY i.sku = 'SKU-004' DESC, i.sku;
SELECT plan_level ::BUTTONS, 'Plan for'::LABEL,
       hint ::HINT
FROM (VALUES (1, '90 %', 'between the 5 % and 95 % quantiles of the projected stock'),
             (2, '80 %', 'between the 10 % and 90 % quantiles'),
             (3, '50 %', 'between the quartiles')) t(k, plan_level, hint) ORDER BY k;
SELECT 1::ENDGROUP;

SELECT 'All items, last 120 days and next 4 weeks'::GROUP;
SELECT (SELECT 100 * sum(served) / sum(demand) FROM it_runs())::PERCENT, 'Fill rate, last 120 days'::LABEL,
       '<95:bad; <98:warn; good'::THRESHOLDS,
       'Share of demand served from stock at once, over all items. Lost sales: what was not on the shelf is lost, not backordered.'::INFO;
SELECT (SELECT count(*) FROM it_risk() WHERE p_out_4w >= 0.5)::METRIC, 'Items likely to run out, next 4 weeks'::LABEL,
       '>=3:bad; >=1:warn; good'::THRESHOLDS,
       'Items whose chance of at least one day out of stock in the next 28 days is 50 % or more, from 400 simulated futures per twin.'::INFO;
SELECT (SELECT count(DISTINCT sku) FROM it_status WHERE state = 'alarm')::METRIC, 'Twins in alarm now'::LABEL,
       '>=2:bad; >=1:warn; good'::THRESHOLDS;
SELECT (SELECT count(*) FROM it_waiting())::METRIC, 'Recalibrations waiting for a decision'::LABEL,
       'The latest proposal per item and parameter that was escalated (too large to apply on its own) or only proposed (class C items), and not approved since.'::INFO;
SELECT (SELECT 100 * avg(within_tolerance::INTEGER) FROM it_replay)::PERCENT, 'Stock counts the twins match'::LABEL,
       '<95:bad; <99:warn; good'::THRESHOLDS,
       'Counted stock vs the twin''s on-hand on the same day; a match is within max(1 unit, 2 % of the count).'::INFO;
SELECT 1::ENDGROUP;

-- ── 0 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 12::COL;
SELECT ('**Which items need attention first?** ' ||
        coalesce((SELECT CASE count(*) WHEN 1 THEN '1 item is' ELSE count(*) || ' items are' END || ' **Critical**: ' ||
                         string_agg('**' || sku || '** (' || round(100 * p_out_4w)::INTEGER || ' %, can run out ' ||
                                    CASE days_left WHEN 1 THEN 'tomorrow' ELSE 'by ' || strftime(short_by, '%a %-d %b') END ||
                                    CASE trust WHEN 'alarm' THEN ', its twin in alarm' ELSE '' END || ')', ', ' ORDER BY rnk)
                  FROM it_crit() WHERE status = 'Critical'), 'No item is **Critical**') || '; ' ||
        (SELECT count(*) FROM it_crit() WHERE status = 'Watch') || ' on *Watch*. ' ||
        '**Critical** means a chance of 50 % or more of running out in the next four weeks, and the item can run out within a week (planning for the ' || it_level_txt() ||
        ' band) or it is a class A top seller. **Watch**: a chance of 20 % or more. **OK**: below. ' ||
        '*Plan trust* (watch list): *alarm* = a drift detector is in alarm now, *review* = a recalibration needs a decision or a stock count missed. ' ||
        'Click an item to show it on the other tabs (now: **' || it_sku() || '**).')::MARKDOWN;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT x AS "Days until it can run out" ::XAXIS, 100 * p_out_4w AS "Chance out in 4 weeks (%)" ::SCATTER,
       status ::CATEGORY, 'Critical:bad; Watch:warn; OK:good' ::COLORS, 4.5 ::SIZE,
       sku || CASE WHEN sku = it_sku() THEN ' ◂' ELSE '' END ::LABEL, 12 ::LABEL_TOP, 100 - rnk ::RANK,
       '>=50:bad; >=20:warn; good' ::THRESHOLDS,
       CASE rnk WHEN 1 THEN 0 WHEN 2 THEN 28.5 END ::XBAND_FROM,
       CASE rnk WHEN 1 THEN 7 WHEN 2 THEN 31.5 END ::XBAND_TO,
       CASE rnk WHEN 2 THEN '> 4 weeks' END ::XBAND_LABEL,
       '?v.sku=' || sku ::LINK,
       'percent' ::YFORMAT,
       ('Risk map: how likely, how soon, how much (' || it_level_txt() || ' plan)')::TITLE,
       'One point per item. Right: the days until the plan band''s lower edge first reaches zero, i.e. the item can run out then (shaded on the left: within a week); items that cannot within four weeks sit in the band at the right edge. Up: the chance of at least one day out of stock in the next 28 days, from 400 simulated futures; red line: 50 %, amber: 20 %. Colour: the item''s status (Critical, Watch, OK). Click a point to select the item (◂ marks the selected one).'::INFO
FROM (SELECT *, coalesce(days_left, 29) + 0.4 * (row_number() OVER (PARTITION BY coalesce(days_left, 29) ORDER BY rnk) - 1) AS x
      FROM it_crit())
ORDER BY rnk;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT p.ds ::XAXIS, c.rnk || '. ' || p.model_id || ' · ' || c.status ::YAXIS, 100 * p.cumulative_stockout_probability ::HEATMAP,
       'percent' ::YFORMAT,
       'When: the chance each item has run out by that day (%)'::TITLE,
       'The share of the 400 simulated futures in which the item has had at least one day out of stock by that day, over the next 28 days. Rows: most critical first.'::INFO
FROM it_projection p JOIN it_crit() c ON c.sku = p.model_id
WHERE p.level = it_level()
ORDER BY c.rnk, p.ds;

SELECT 12::COL;
SELECT 'Chance out in 4 weeks %: >=50:bad; >=20:warn; good | Days of cover: <7:bad; <14:warn; good'::THRESHOLDS;
SELECT c.sku AS "Item" ::TABLE, c.status AS "Status" ::BADGE,
       round(100 * c.p_out_4w) AS "Chance out in 4 weeks %",
       coalesce(strftime(c.short_by, '%a %-d %b') || ' (' || c.days_left || ' d)', 'not in 4 weeks') AS "Can run out by",
       '#Stock & pipeline?v.sku={value}' AS "Open" ::LINK,
       round(c.units_short) AS "Expected units short",
       c.trust AS "Plan trust" ::BADGE,
       coalesce(nullif(c.trust_why, ''), '—') AS "Trust: why",
       round(c.cover_days) AS "Days of cover",
       c.on_hand_now || ' + ' || c.on_order_now AS "On hand + on order",
       (SELECT list(yhat ORDER BY ds) FROM it_projection p WHERE p.model_id = c.sku AND p.level = it_level()) AS "Projected stock (median)" ::SPARKLINE,
       c.cohort || ' · ' || c.shape AS "Class · demand",
       ('Watch list: every item, most critical first (' || it_level_txt() || ' plan)')::TITLE,
       'Days of cover: units on hand / average daily demand of the last 28 days. Can run out by: the first day the plan band''s lower edge reaches zero. Expected units short: demand the simulated futures could not serve, on average, over the next 28 days. Plan trust: alarm = a drift detector is latched in alarm now; review = a recalibration needs the planner (n waiting: escalated, or any for a class A / B item; class C proposals always wait and count as routine), or stock counts did not match the twin (missed / counted).'::INFO
FROM it_crit() c ORDER BY c.rnk;

-- ── 1 · Stock & pipeline ────────────────────────────────────────────────────
SELECT 'Stock & pipeline'::TAB;

SELECT 12::COL;
SELECT ('**How did ' || r.sku || ' do?** Class ' || i.cohort || ', ' || i.shape || ' demand. Over the last 120 days the twin served **' ||
        round(100 * r.served / r.demand, 1) || ' %** of demand from stock; it was out of stock on **' || r.days_out || '** days and **' ||
        r.lost || '** units of demand were lost. Today **' || r.on_hand_now || '** units are on hand and **' || r.on_order_now || '** on order. ' ||
        CASE WHEN EXISTS (SELECT 1 FROM it_story()) THEN 'Step through the chart (‹ / Play / ›) to follow what happened.' ELSE '' END)::MARKDOWN
FROM it_runs() r JOIN it_items i USING (sku) WHERE r.sku = it_sku();

SELECT it_sku() || ': this item'::GROUP;
SELECT 100 * served / demand ::PERCENT, 'Fill rate'::LABEL, '<95:bad; <98:warn; good'::THRESHOLDS,
       'Units served from stock at once / units demanded, last 120 days.'::INFO
FROM it_runs() WHERE sku = it_sku();
SELECT days_out ::METRIC, 'Days out of stock'::LABEL, '>=5:bad; >=1:warn; good'::THRESHOLDS FROM it_runs() WHERE sku = it_sku();
SELECT lost ::METRIC, 'Units lost'::LABEL FROM it_runs() WHERE sku = it_sku();
SELECT on_hand_now ::METRIC, 'On hand now'::LABEL,
       (SELECT list(on_hand_end ORDER BY period) FROM it_trajectory WHERE sku = it_sku() AND period > (SELECT max(period) FROM it_trajectory) - 28) ::SPARKLINE
FROM it_runs() WHERE sku = it_sku();
SELECT on_order_now ::METRIC, 'On order now'::LABEL FROM it_runs() WHERE sku = it_sku();
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT t.ds ::XAXIS, s.series ::CATEGORY, s.units ::STEP,
       CASE WHEN s.series = 'On hand' AND t.on_hand_end = 0 THEN t.ds END ::MARKAREA,
       CASE WHEN s.series = 'On hand' THEN st.caption END ::STORY,
       (it_sku() || ': stock on hand and on order, end of day (grey: out of stock)')::TITLE,
       'On hand: units on the shelf after the day''s sales. On order: ordered, not yet delivered. The strips below share the time axis: the detectors'' alarms, the orders and deliveries, and the twin''s parameter changes.'::INFO
FROM it_trajectory t
CROSS JOIN LATERAL (VALUES ('On hand', t.on_hand_end), ('On order', t.on_order_end)) s(series, units)
LEFT JOIN it_story() st ON st.ds = t.ds
WHERE t.sku = it_sku()
ORDER BY s.series, t.ds;
SELECT ds ::XAXIS, it_detector(name) ::YAXIS,
       CASE tier WHEN 'alarm' THEN '#d64545' ELSE '#e8a23b' END ::CATEGORY,
       1 ::HEATMAP,
       tier || ': ' || it_detector(name) || coalesce(' on ' || replace(stream, '_', ' '), '') || ' = ' || value || ' (line ' || threshold || ')' ::LABEL,
       'Alarms' ::TRACK
FROM it_alarms WHERE sku = it_sku() ORDER BY ds;
SELECT ds ::XAXIS, lane ::YAXIS, lane ::CATEGORY, what ::LABEL, 'Orders' ::TRACK
FROM (SELECT ds, 'ordered' AS lane, 'ordered ' || order_qty || ', due ' || strftime(it_ds(order_arrival_period), '%-d %b') AS what
      FROM it_trajectory WHERE sku = it_sku() AND order_qty > 0
      UNION ALL
      SELECT ds, 'delivered', 'delivered ' || receipts FROM it_trajectory WHERE sku = it_sku() AND receipts > 0)
ORDER BY ds;
SELECT ds ::XAXIS, 'Twin changes' ::TRACK,
       it_param(parameter) || ' ' || before_num || ' → ' || after_num || ' (' || status || ')' ::LABEL
FROM it_changes WHERE sku = it_sku() AND status IN ('applied', 'approved') ORDER BY ds;

SELECT 12::COL;
SELECT 260::HEIGHT;
SELECT ds ::XAXIS, kind ::CATEGORY, units ::BARCHART_STACKED,
       (it_sku() || ': demand per day — served from stock, and lost')::TITLE
FROM (SELECT ds, 'served' AS kind, fulfilled AS units FROM it_trajectory WHERE sku = it_sku()
      UNION ALL SELECT ds, 'unserved (lost)', lost FROM it_trajectory WHERE sku = it_sku())
ORDER BY kind, ds;

-- ── 2 · Risk ─────────────────────────────────────────────────────────────────
SELECT 'Risk'::TAB;

SELECT 12::COL;
SELECT ('**Where will stock run short in the next four weeks?** The twin simulates 400 futures per item from today''s stock and open orders, with its current parameters. ' ||
        'Most likely to run out (a chance of 50 % or more): ' ||
        coalesce((SELECT string_agg('**' || sku || '** (' || round(100 * p_out_4w)::INTEGER || ' %)', ', ' ORDER BY p_out_4w DESC, sku) FROM it_risk() WHERE p_out_4w >= 0.5), 'none') ||
        '. Planning for the **' || it_level_txt() || '** band, the table says by when each item can run out (the band''s lower edge reaches zero). For **' || it_sku() || '** the chance of at least one day out of stock is **' ||
        (SELECT round(100 * p_out_4w)::INTEGER FROM it_risk() WHERE sku = it_sku()) || ' %**. Click an item in the table to see its fan.')::MARKDOWN;

SELECT 7::COL;
SELECT 360::HEIGHT;
WITH fan AS (
    SELECT ds, any_value(yhat) AS yhat,
           max(yhat_lower) FILTER (WHERE level = 0.9) AS lo90, max(yhat_upper) FILTER (WHERE level = 0.9) AS hi90,
           max(yhat_lower) FILTER (WHERE level = 0.8) AS lo80, max(yhat_upper) FILTER (WHERE level = 0.8) AS hi80,
           max(yhat_lower) FILTER (WHERE level = 0.5) AS lo50, max(yhat_upper) FILTER (WHERE level = 0.5) AS hi50
    FROM it_projection WHERE model_id = it_sku() GROUP BY ds)
SELECT ds ::XAXIS, 'On hand' ::CATEGORY, on_hand_end ::LINECHART,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::DOUBLE ::BAND_LOWER, NULL::DOUBLE ::BAND_UPPER,
       NULL::VARCHAR ::LINESTYLE,
       it_today() ::XLINE, 'Today' ::XLINE_LABEL,
       (it_sku() || ': on hand, the last 6 weeks and the next 4 (bands: 90 / 80 / 50 %)')::TITLE,
       'The median of the simulated stock, and the bands that hold 90 %, 80 % and 50 % of the simulated futures (darker = narrower). A band''s lower edge at zero means that share of futures, or more, run out that day.'::INFO
FROM it_trajectory WHERE sku = it_sku() AND ds > it_today() - 42
UNION ALL
SELECT ds, 'Projected (median)', yhat, lo90, hi90, lo80, hi80, lo50, hi50, 'flow', NULL, NULL, NULL, NULL FROM fan
ORDER BY 1;

SELECT 5::COL;
SELECT 360::HEIGHT;
SELECT ds ::XAXIS, kind ::CATEGORY, 100 * p ::LINECHART, 'percent'::YFORMAT,
       100 * it_tail() ::REFLINE, ('accepted at ' || it_level_txt())::REFLINE_LABEL,
       (it_sku() || ': chance of running out (%)')::TITLE,
       'Per day: the share of simulated futures with no stock left at the end of that day. By then: the share that ran out on any day up to it. The line: the tail outside the plan band below its lower edge.'::INFO
FROM (SELECT ds, 'per day' AS kind, stockout_probability AS p FROM it_projection WHERE model_id = it_sku() AND level = it_level()
      UNION ALL
      SELECT ds, 'by then', cumulative_stockout_probability FROM it_projection WHERE model_id = it_sku() AND level = it_level())
ORDER BY kind, ds;

SELECT 12::COL;
SELECT 'Chance out in 4 weeks %: >=50:bad; >=20:warn; good'::THRESHOLDS;
SELECT r.sku AS "Item" ::TABLE, i.cohort AS "Class", i.shape AS "Demand",
       n.on_hand_now AS "On hand", n.on_order_now AS "On order",
       coalesce(strftime(r.short_by, '%a %-d %b'), '—') AS "Can run out by",
       round(100 * r.p_out_4w) AS "Chance out in 4 weeks %",
       round(r.units_short) AS "Expected units short",
       round(100 * r.p_order_7d) AS "Orders within 7 days %",
       '#Risk?v.sku={value}' AS "Fan" ::LINK,
       ('Every item at the ' || it_level_txt() || ' plan level, riskiest first')::TITLE
FROM it_risk() r JOIN it_items i USING (sku) JOIN it_runs() n USING (sku)
ORDER BY r.p_out_4w DESC, r.sku;

-- ── 3 · Monitoring ───────────────────────────────────────────────────────────
SELECT 'Monitoring'::TAB;

SELECT 12::COL;
SELECT ('**Does the twin still describe reality?** Every twin runs drift detectors on its own forecast error and on the demand it sees. Over the run they raised **' ||
        (SELECT count(*) FILTER (WHERE tier = 'alarm') FROM it_alarms) || '** alarms and **' || (SELECT count(*) FILTER (WHERE tier = 'warning') FROM it_alarms) ||
        '** warnings, most on ' || (SELECT string_agg(DISTINCT it_detector(name), ' and ') FROM (SELECT name FROM it_alarms GROUP BY name ORDER BY count(*) DESC, name LIMIT 2)) || '. ' ||
        CASE WHEN (SELECT count(*) FROM it_alarms WHERE sku = it_sku()) = 0 THEN '**' || it_sku() || '** raised none.'
             ELSE '**' || it_sku() || '** raised ' || (SELECT count(*) FROM it_alarms WHERE sku = it_sku()) || ', the first on ' ||
                  (SELECT strftime(min(ds), '%-d %b') FROM it_alarms WHERE sku = it_sku()) || '.' END)::MARKDOWN;

SELECT 5::COL;
SELECT 300::HEIGHT;
SELECT a.cohort || ' · ' || a.shape ::XAXIS, it_detector(a.name) ::YAXIS, count(*) ::HEATMAP, 12 ::DATALABELS,
       'Alarms and warnings by detector and item group'::TITLE,
       'Columns: the items'' value class (A = top sellers) and demand shape; rows: the detector that fired. CUSUM and Page-Hinkley watch the forecast error, PSI and KS compare the demand distribution with the baseline, MAE is the forecast''s running error, Occurrence CUSUM the share of days with any demand.'::INFO
FROM it_alarms a GROUP BY ALL ORDER BY 2, 1;

SELECT 7::COL;
SELECT 300::HEIGHT;
SELECT date_trunc('week', ds)::DATE ::XAXIS, tier ::CATEGORY, count(*) ::BARCHART_STACKED,
       'Alarms and warnings per week, all items'::TITLE
FROM it_alarms GROUP BY ALL ORDER BY 2 DESC, 1;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT it_ds(t.period) ::XAXIS, t.statistic ::LINECHART,
       CASE t.period WHEN m.p0 THEN t.warning_threshold WHEN m.p0 + 1 THEN t.alarm_threshold END ::REFLINE,
       CASE t.period WHEN m.p0 THEN 'warning' WHEN m.p0 + 1 THEN 'alarm' END ::REFLINE_LABEL,
       CASE WHEN t.state = 'alarm' THEN it_ds(t.period) END ::MARKAREA,
       (it_sku() || ': CUSUM on the forecast error (shaded: in alarm)')::TITLE,
       'Cumulative sum of the standardized forecast error beyond a slack of half a standard deviation, both directions. It climbs when the forecast is off in one direction for many days and falls back when it is right again. Empty at the start: the detector first collects a baseline.'::INFO
FROM it_trace t, (SELECT min(period) AS p0 FROM it_trace WHERE sku = it_sku() AND test = 'cusum' AND statistic IS NOT NULL) m
WHERE t.sku = it_sku() AND t.test = 'cusum' AND t.statistic IS NOT NULL ORDER BY t.period;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT it_ds(t.period) ::XAXIS, t.statistic ::LINECHART,
       CASE t.period WHEN m.p0 THEN t.warning_threshold WHEN m.p0 + 1 THEN t.alarm_threshold END ::REFLINE,
       CASE t.period WHEN m.p0 THEN 'warning' WHEN m.p0 + 1 THEN 'alarm' END ::REFLINE_LABEL,
       CASE WHEN t.state = 'alarm' THEN it_ds(t.period) END ::MARKAREA,
       (it_sku() || ': Page-Hinkley on the forecast error (shaded: in alarm)')::TITLE,
       'How far the running sum of the forecast error has risen above its lowest point: a lasting shift in the mean shows as a steady climb.'::INFO
FROM it_trace t, (SELECT min(period) AS p0 FROM it_trace WHERE sku = it_sku() AND test = 'page_hinkley' AND statistic IS NOT NULL) m
WHERE t.sku = it_sku() AND t.test = 'page_hinkley' AND t.statistic IS NOT NULL ORDER BY t.period;

SELECT 12::COL;
SELECT it_detector(test) AS "Detector" ::TABLE, coalesce(replace(stream, '_', ' '), 'forecast (running)') AS "Watches",
       statistic AS "Now", warning_threshold AS "Warning at", alarm_threshold AS "Alarm at",
       replace(level, '_', ' ') AS "Today" ::BADGE, replace(state, '_', ' ') AS "Latched state" ::BADGE,
       n_baseline AS "Baseline days", n_current AS "Recent days",
       (it_sku() || ': every detector at the end of the run')::TITLE,
       'Today: where the statistic stands against its lines this day. Latched state: the alarm or warning stays raised until the statistic falls well below the warning line again, so a flapping detector does not alarm twice.'::INFO
FROM it_status WHERE sku = it_sku() ORDER BY test;

-- ── 4 · Calibration ──────────────────────────────────────────────────────────
SELECT 'Calibration'::TAB;

SELECT 12::COL;
SELECT ('**What did the twin change about itself, and what waits for a decision?** Every 30 days, and the day after an alarm, the twin re-estimates its lead times and its demand model from what it saw. ' ||
        'Over the run it proposed **' || (SELECT count(*) FROM it_proposals) || '** changes: **' || (SELECT count(*) FROM it_proposals WHERE status = 'applied') ||
        '** applied on its own (small enough for guarded auto), **' || (SELECT count(*) FROM it_proposals WHERE status = 'escalated') ||
        '** escalated to the planner as too large, **' || (SELECT count(*) FROM it_proposals WHERE status = 'proposed') ||
        '** proposed for the class C items, which never change on their own. For **' || it_sku() || '**: ' ||
        (SELECT count(*) FROM it_changes WHERE sku = it_sku() AND status IN ('applied', 'approved')) || ' changes in effect, ' ||
        (SELECT count(*) FROM it_waiting() WHERE sku = it_sku()) ||
        ' waiting.')::MARKDOWN;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT t.ds ::XAXIS,
       coalesce((SELECT sum(p.expected_rop_delta) FROM it_changes c JOIN it_proposals p ON p.id = c.proposal_id
                 WHERE c.sku = t.sku AND c.status IN ('applied', 'approved') AND c.period < t.period), 0) ::STEP,
       0 ::REFLINE, 'as built' ::REFLINE_LABEL,
       (it_sku() || ': how calibration moved the reorder point (units, against the twin as built)')::TITLE,
       'The sum of the expected reorder-point changes of every change in effect: lead times and the demand model set how much stock has to cover the time until a delivery arrives. Each step is a change applied by the twin or approved by the planner.'::INFO
FROM it_trajectory t WHERE t.sku = it_sku() ORDER BY t.ds;

SELECT 12::COL;
SELECT 'status = escalated'::FLAG;
SELECT strftime(it_ds(period), '%-d %b') AS "Day" ::TABLE, it_param(parameter) AS "Parameter",
       old_num AS "Before", new_num AS "After", expected_rop_delta AS "Reorder point change (units)" ::COLORSCALE_DIVERGING,
       sample_size AS "Evidence (n)", confidence AS "Confidence",
       lineage_independent AS "Independent", lineage_twin_influenced AS "Twin-influenced",
       status AS "Status" ::BADGE, reason AS "Why",
       'Evidence: independent, twin-influenced'::COLGROUP,
       (it_sku() || ': every proposal, before and after')::TITLE,
       'Independent evidence was observed without the twin''s own decisions in play (e.g. deliveries of orders placed before the twin went live); twin-influenced evidence was shaped by them (orders the twin placed, days its own stockouts cut demand short). A proposal resting mostly on twin-influenced evidence can feed back on itself.'::INFO
FROM it_proposals WHERE sku = it_sku() ORDER BY period, parameter;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT it_param(parameter) ::XAXIS, kind ::CATEGORY, n ::BARCHART_STACKED, 1 ::FLIP,
       'Evidence behind all proposals: independent vs twin-influenced'::TITLE,
       'Observations behind each proposal, summed over all items and runs.'::INFO
FROM (SELECT parameter, 'independent' AS kind, sum(lineage_independent) AS n FROM it_proposals GROUP BY ALL
      UNION ALL SELECT parameter, 'twin-influenced', sum(lineage_twin_influenced) FROM it_proposals GROUP BY ALL)
ORDER BY 1, 2;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT anofox_plot_events(e),
       (it_sku() || ': alarms and parameter changes on one timeline')::TITLE
FROM (SELECT ds, 'alarm' AS kind, it_detector(name) || coalesce(' on ' || replace(stream, '_', ' '), '') AS label,
             tier AS severity, value, threshold, sku AS model_id, 'alarms' AS series
      FROM it_alarms WHERE sku = it_sku()
      UNION ALL
      SELECT ds, status, it_param(parameter) || ' ' || before_num || ' → ' || after_num,
             CASE WHEN status = 'escalated' THEN 'warning' ELSE 'info' END, after_num, NULL, sku, 'calibration'
      FROM it_changes WHERE sku = it_sku()) e;

SELECT 12::COL;
SELECT sku AS "Item" ::TABLE, strftime(ds, '%-d %b') AS "Day", it_param(parameter) AS "Parameter",
       before_num AS "Before", after_num AS "After", expected_rop_delta AS "Reorder point change (units)" ::COLORSCALE_DIVERGING,
       status AS "Status" ::BADGE, reason AS "Why",
       'Waiting for the planner: escalated (too large to apply on their own) or proposed (class C), largest effect first'::TITLE
FROM it_waiting()
ORDER BY abs(expected_rop_delta) DESC NULLS LAST, sku;

-- ── 5 · Replay ───────────────────────────────────────────────────────────────
SELECT 'Replay'::TAB;

SELECT 12::COL;
SELECT ('**Can we trust the twin''s stock?** The twin''s end-of-day stock was compared with every physical count: **' ||
        (SELECT count(*) FILTER (WHERE within_tolerance) FROM it_replay) || '** of **' || (SELECT count(*) FROM it_replay) ||
        '** counts match within the tolerance, the mean gap is **' || (SELECT round(avg(abs_deviation), 2) FROM it_replay) || '** units. ' ||
        coalesce((SELECT 'The misses: ' || string_agg(sku || ' (' || n || ')', ', ' ORDER BY n DESC, sku) || ' — a count to check on the shelf.'
                  FROM (SELECT sku, count(*) AS n FROM it_replay WHERE NOT within_tolerance GROUP BY sku)), 'No count misses.'))::MARKDOWN;

SELECT 12::COL;
SELECT 320::HEIGHT;
SELECT anofox_plot_prediction(p, y_label := 'units on hand', x_label := ''),
       (it_sku() || ': the twin''s stock (line) vs the physical counts (points)')::TITLE
FROM (SELECT t.ds, r.recorded_on_hand AS y, t.on_hand_end AS yhat, 'twin' AS model_id
      FROM it_trajectory t LEFT JOIN it_replay r ON r.sku = t.sku AND r.period = t.period
      WHERE t.sku = it_sku()) p;

SELECT 12::COL;
SELECT 'Counts matched %: <90:bad; <100:warn; good'::THRESHOLDS;
SELECT sku AS "Item" ::TABLE, count(*) AS "Counts", count(*) FILTER (WHERE within_tolerance) AS "Matched",
       round(100 * avg(within_tolerance::INTEGER), 1) AS "Counts matched %",
       round(avg(abs_deviation), 2) AS "Mean gap (units)", max(abs_deviation) AS "Largest gap (units)",
       strftime(arg_max(ds, abs_deviation), '%-d %b') AS "Largest gap on",
       list(deviation ORDER BY period) AS "Twin − count" ::SPARKLINE,
       'Twin vs physical counts, per item'::TITLE,
       'A count matches when |twin − count| ≤ max(1 unit, 2 % of the count).'::INFO
FROM it_replay GROUP BY sku ORDER BY "Counts matched %", sku;

-- ── 6 · Pipeline ─────────────────────────────────────────────────────────────
SELECT 'Pipeline'::TAB;

SELECT 12::COL;
SELECT ('**What did this run do?** From ' || (SELECT records_in FROM it_manifest WHERE stage = 'ingest') || ' records (' ||
        (SELECT records_rejected FROM it_manifest WHERE stage = 'ingest') || ' rejected) to ' || (SELECT twins FROM it_manifest WHERE stage = 'build_twin') ||
        ' twins, ' || (SELECT sum(alarms) FROM it_manifest) || ' alarms, ' || (SELECT sum(proposals) FROM it_manifest) || ' calibration proposals (' ||
        (SELECT sum(proposals_applied) FROM it_manifest) || ' in effect) and a four-week projection. The loop from the stream back to calibration is the twin correcting itself.')::MARKDOWN;

-- The run as a diagram, straight from the manifest and its edges in
-- inventory-rs's own names (stage, seq, from_stage, to_stage, count) through
-- the `graph` contract: anofox_plot_diagram. Lanes say what each stage is for
-- (data in, the twin, its correction, the plan); a sparkline in each box shows
-- the evidence behind the plan over the run (demand landed, alarms raised,
-- counts the twin matched, corrections in effect, stockout risk ahead); edges
-- move faster the more records they carry ('flow-volume'). Each box's
-- number is named in its title; the line under it is its evidence per week.
SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT anofox_plot_diagram(n, edges := (SELECT list(e) FROM it_edges e), style := 'flow-volume'),
       'Can we trust the plan? What fed it, what checked it, what corrected it'::TITLE
FROM (
  SELECT m.stage, m.seq,
         CASE m.stage WHEN 'ingest' THEN 'Ingest · records' WHEN 'build_twin' THEN 'Build twins' WHEN 'calibrate' THEN 'Calibrate · in effect'
                      WHEN 'stream' THEN 'Stream · alarms' WHEN 'replay_history' THEN 'Replay · % counts matched' WHEN 'project' THEN 'Project 4 weeks' END AS title,
         CASE m.stage WHEN 'ingest' THEN m.records_accepted WHEN 'calibrate' THEN m.proposals_applied WHEN 'stream' THEN m.alarms
                      WHEN 'replay_history' THEN (SELECT round(100 * avg(within_tolerance::INTEGER)) FROM it_replay) ELSE m.twins END AS mainstat,
         CASE m.stage WHEN 'ingest' THEN 'Data in' WHEN 'project' THEN 'The plan' WHEN 'calibrate' THEN 'Correction' ELSE 'The twin' END AS lane,
         CASE WHEN m.stage = 'stream' AND m.alarms > 0 THEN 'alarm'
              WHEN m.stage = 'replay_history' AND (SELECT avg(within_tolerance::INTEGER) FROM it_replay) < 0.9 THEN 'warn'
              WHEN m.records_rejected > 0 THEN 'warn' ELSE 'ok' END AS status,
         CASE m.stage
           WHEN 'ingest' THEN (SELECT list(d ORDER BY w) FROM (SELECT period // 7 AS w, sum(demand) AS d FROM it_trajectory GROUP BY ALL))
           WHEN 'stream' THEN (SELECT list(coalesce(a.n, 0) ORDER BY w.w) FROM (SELECT DISTINCT period // 7 AS w FROM it_trajectory) w
                               LEFT JOIN (SELECT period // 7 AS w, count(*) AS n FROM it_alarms GROUP BY ALL) a USING (w))
           WHEN 'replay_history' THEN (SELECT list(m ORDER BY w) FROM (SELECT period // 7 AS w, avg(within_tolerance::INTEGER) AS m FROM it_replay GROUP BY ALL))
           WHEN 'calibrate' THEN (SELECT list(c ORDER BY w) FROM (SELECT w, sum(n) OVER (ORDER BY w) AS c
                                  FROM (SELECT w.w, count(ch.period) AS n FROM (SELECT DISTINCT period // 7 AS w FROM it_trajectory) w
                                        LEFT JOIN it_changes ch ON ch.period // 7 = w.w GROUP BY w.w)))
           WHEN 'project' THEN (SELECT list(p ORDER BY period) FROM (SELECT period, avg(stockout_probability) AS p FROM it_projection
                                WHERE stockout_probability IS NOT NULL GROUP BY ALL))
         END AS sparkline
  FROM it_manifest m
) n;

SELECT 12::COL;
SELECT CASE stage WHEN 'ingest' THEN 'Ingest' WHEN 'build_twin' THEN 'Build twins' WHEN 'calibrate' THEN 'Calibrate'
                  WHEN 'stream' THEN 'Stream 120 days' WHEN 'replay_history' THEN 'Replay vs counts' WHEN 'project' THEN 'Project 4 weeks' END AS "Stage" ::TABLE,
       strftime(it_ds(started_period), '%-d %b') || ' – ' || strftime(it_ds(ended_period), '%-d %b') AS "Days",
       records_in AS "Records in", records_accepted AS "Accepted", records_rejected AS "Rejected",
       twins AS "Twins", alarms AS "Alarms", proposals AS "Proposals", proposals_applied AS "In effect", artefact AS "Writes",
       'The run manifest'::TITLE
FROM it_manifest ORDER BY seq;
