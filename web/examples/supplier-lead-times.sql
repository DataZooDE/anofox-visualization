-- @title How reliable are our suppliers, and what does it cost us in stock?
-- @group Logistics & production
-- @tags  inventory, suppliers, lead time
--
-- Supplier lead times: how reliable each supplier is, whether one is getting
-- slower, and what the unreliability costs in safety stock and fill rate.
--
-- STORED DATA, SYNTHETIC. The tables in web/data/supplier-lead-times/*.csv
-- are written by examples/supplier-lead-times/synthetic.py (Python standard
-- library, seed 7): 6 made-up suppliers, 15 made-up items, purchase-order
-- lines from Jan 2025 to Sep 2026 in the layouts inventory-rs
-- (DataZooDE/inventory-rs, private) reads for lead-time calibration —
-- contract v1 `po_history` and `po_receipt` — plus the lead-time error the
-- monitor watches (`lead_time_error`, with a CUSUM) and the calibration
-- proposals for `supplier.lead_time_mean` / `supplier.lead_time_std_dev`.
-- Nothing in them is inventory-rs output or real data;
-- web/data/supplier-lead-times/provenance.csv records the source.
--
-- Lead time is inventory-calibration's default rule (WeightedMeanReceipt):
-- the quantity-weighted mean receipt day minus the order day. A line is on
-- time when its last receipt arrives by the promised date. Safety stock is
-- King's formula (inventory-analytics `safety_stock_kings`), which adds
-- lead-time variance to demand variance.
--
-- The question: how reliable are our suppliers' lead times, and how much
-- safety stock does that cost us? Asked by the purchasing lead of a
-- distributor at the end of Q3 2026.
--
-- Tabs: Reliability · Over time · Safety stock · Orders.
-- Natively, run from web/:  dashboard --check ../examples/supplier-lead-times/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'How reliable are our suppliers, and what does it cost us in stock?'::DASHBOARD_TITLE;
-- The house palette, with grey second: the selected supplier is drawn in the
-- accent, every other supplier (the second series) muted.
SELECT '{"extends": "light", "palette": ["#2563eb", "#b8bec8", "#eb6834", "#1baf7a", "#eda100", "#4a3aa7"],
         "dark": {"palette": ["#3b82f6", "#5b6472", "#d95926", "#199e70", "#c98500", "#9085e9"]}}'::THEME;

-- ── Data ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE lt_po AS FROM read_csv('data/supplier-lead-times/po_history.csv');
CREATE OR REPLACE TABLE lt_receipt AS FROM read_csv('data/supplier-lead-times/po_receipt.csv');
CREATE OR REPLACE TABLE lt_items AS FROM read_csv('data/supplier-lead-times/items.csv');
CREATE OR REPLACE TABLE lt_suppliers AS FROM read_csv('data/supplier-lead-times/suppliers.csv');
CREATE OR REPLACE TABLE lt_disruptions AS FROM read_csv('data/supplier-lead-times/disruptions.csv');
CREATE OR REPLACE TABLE lt_monitor AS FROM read_csv('data/supplier-lead-times/lead_time_monitor.csv');
CREATE OR REPLACE TABLE lt_proposals AS FROM read_csv('data/supplier-lead-times/proposals.csv');

-- One row per closed order line: its lead time (quantity-weighted mean
-- receipt day − order day), the promise and how late the last receipt was.
CREATE OR REPLACE TABLE lt_lines AS
SELECT *, datediff('day', ordered_at, promised_at) AS promised_lt, datediff('day', promised_at, closed_at) AS days_late
FROM (SELECT h.po_id, any_value(s.name) AS supplier, any_value(h.sku_id) AS sku_id, any_value(h.ordered_at) AS ordered_at,
             any_value(h.promised_at) AS promised_at, any_value(h.ordered_qty) AS ordered_qty,
             max(r.received_at) AS closed_at, count(*) AS receipts,
             sum(datediff('day', h.ordered_at, r.received_at) * r.received_qty) / sum(r.received_qty) AS lead_time
      FROM lt_po h JOIN lt_receipt r USING (po_id, line_id) JOIN lt_suppliers s USING (supplier_id)
      WHERE h.status = 'received'
      GROUP BY h.po_id);

CREATE OR REPLACE MACRO lt_today() AS (SELECT max(received_at) FROM lt_receipt);
-- This quarter and the one before (lines by the day they closed).
CREATE OR REPLACE MACRO lt_q0() AS date_trunc('quarter', lt_today())::DATE;
CREATE OR REPLACE MACRO lt_q1() AS (date_trunc('quarter', lt_today()) - INTERVAL 3 MONTH)::DATE;
CREATE OR REPLACE MACRO lt_qname(d) AS 'Q' || quarter(d) || ' ' || year(d);

-- The inputs.
CREATE OR REPLACE MACRO lt_sup() AS coalesce(getvariable('supplier'), 'Castings');
-- The selected supplier as its own series, the rest as one muted series
-- ("other suppliers" sorts after every capitalised name, so it takes the
-- second, grey palette slot).
CREATE OR REPLACE MACRO lt_pick(supplier) AS CASE WHEN supplier = lt_sup() THEN supplier ELSE 'other suppliers' END;
CREATE OR REPLACE MACRO lt_level_txt() AS coalesce(getvariable('service_level'), '95 %');
CREATE OR REPLACE MACRO lt_alpha() AS replace(lt_level_txt(), ' %', '')::INTEGER / 100;
CREATE OR REPLACE MACRO lt_z() AS
    CASE lt_level_txt() WHEN '90 %' THEN 1.2816 WHEN '98 %' THEN 2.0537 WHEN '99 %' THEN 2.3263 ELSE 1.6449 END;

-- The standard normal: density, upper tail (Abramowitz & Stegun 26.2.17) and
-- the loss function G(z) = φ(z) − z·(1 − Φ(z)), the expected units short per
-- unit of σ in a replenishment cycle.
CREATE OR REPLACE MACRO lt_pdf(x) AS exp(-x * x / 2) / sqrt(2 * pi());
CREATE OR REPLACE MACRO lt_poly(t) AS
    t * (0.319381530 + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
CREATE OR REPLACE MACRO lt_upper(x) AS
    CASE WHEN x >= 0 THEN lt_pdf(x) * lt_poly(1 / (1 + 0.2316419 * x))
         ELSE 1 - lt_pdf(x) * lt_poly(1 / (1 - 0.2316419 * x)) END;
CREATE OR REPLACE MACRO lt_loss(z) AS lt_pdf(z) - z * lt_upper(z);

-- Per supplier over closed lines in [d0, d1): mean, spread and p90 of the
-- lead time, on-time share, days late when late.
CREATE OR REPLACE MACRO lt_stats(d0, d1) AS TABLE
SELECT supplier, count(*) AS lines, avg(lead_time) AS mean_lt, stddev_samp(lead_time) AS sd_lt,
       stddev_samp(lead_time) / avg(lead_time) AS cv,
       quantile_cont(lead_time, 0.9) AS p90_lt, quantile_cont(lead_time, 0.5) AS p50_lt,
       avg((days_late <= 0)::INTEGER) AS on_time,
       avg(days_late) FILTER (WHERE days_late > 0) AS late_by,
       any_value(promised_lt) AS promised
FROM lt_lines WHERE closed_at >= d0 AND closed_at < d1 GROUP BY supplier;

-- Safety stock per item at the chosen service level, from the lead times of
-- closed lines in [d0, d1) (King's formula), against the textbook formula
-- that treats the lead time as fixed, and the fill rate each one buys with
-- the item's order quantity (one review period of demand).
CREATE OR REPLACE MACRO lt_ss(d0, d1) AS TABLE
WITH k AS (
    SELECT i.sku_id, i.description, s.supplier, i.demand_mean AS d, i.demand_std_dev AS sd_d,
           i.unit_cost, i.demand_mean * i.review_days AS q, s.mean_lt AS l, s.sd_lt AS sd_l,
           sqrt(s.mean_lt * i.demand_std_dev ^ 2 + i.demand_mean ^ 2 * s.sd_lt ^ 2) AS sd_ltd
    FROM lt_items i JOIN lt_suppliers p USING (supplier_id) JOIN lt_stats(d0, d1) s ON s.supplier = p.name)
SELECT *, lt_z() * sd_d * sqrt(l) AS ss_fixed, lt_z() * sd_ltd AS ss_kings,
       1 - sd_ltd * lt_loss(lt_z() * sd_d * sqrt(l) / sd_ltd) / q AS fill_fixed,
       1 - sd_ltd * lt_loss(lt_z()) / q AS fill_kings
FROM k;

-- Alarm spells of the lead-time CUSUM: the line that raised each alarm.
CREATE OR REPLACE TABLE lt_alarms AS
SELECT s.name AS supplier, m.* FROM (
    SELECT *, lag(state, 1, 'ok') OVER (PARTITION BY supplier_id ORDER BY closed_at, po_id) AS before
    FROM lt_monitor) m JOIN lt_suppliers s USING (supplier_id)
WHERE m.state = 'alarm' AND m.before <> 'alarm';

-- The selected supplier's story, week by week: disruptions, drift alarms,
-- recalibrations of the planned lead time and the planner's approvals.
CREATE OR REPLACE MACRO lt_story() AS TABLE
WITH steps AS (
    SELECT start_date AS d, 1 AS k, 'Disruption from ' || strftime(start_date, '%-d %b') || ': ' || what AS caption
    FROM lt_disruptions d JOIN lt_suppliers s USING (supplier_id) WHERE s.name = lt_sup()
    UNION ALL
    SELECT closed_at, 2, 'Drift alarm: ' || po_id || ' took ' || round(lead_time_days, 1) || ' days against the ' ||
           round(assumed_lead_time, 1) || ' planning assumed; the lead-time CUSUM crosses its alarm line'
    FROM lt_alarms WHERE supplier = lt_sup()
    UNION ALL
    SELECT ds, 3, 'Recalibration: planned lead time ' || round(old_num, 1) || ' → ' || round(new_num, 1) || ' days, ' ||
           CASE status WHEN 'applied' THEN 'applied on its own (a small step)'
                       ELSE 'too large to apply on its own — sent to the planner' END
    FROM lt_proposals p JOIN lt_suppliers s USING (supplier_id)
    WHERE s.name = lt_sup() AND parameter = 'supplier.lead_time_mean' AND (trigger = 'alarm' OR status <> 'applied')
    UNION ALL
    SELECT decided_at, 4, 'The planner approves ' || round(new_num, 1) || ' days: reorder points and safety stock rise with it'
    FROM lt_proposals p JOIN lt_suppliers s USING (supplier_id)
    WHERE s.name = lt_sup() AND status = 'approved')
SELECT date_trunc('week', d)::DATE AS wk, string_agg(caption, ' · ' ORDER BY d, k) AS caption
FROM steps WHERE d >= DATE '2025-03-03' GROUP BY 1;

-- ── Header: the question, the answer, the inputs, the fleet ─────────────────
SELECT 12::COL;
SELECT ('**The purchasing lead** of a distributor buys ' || (SELECT count(*) FROM lt_items) || ' items from ' ||
        (SELECT count(*) FROM lt_suppliers) || ' suppliers. Every late or erratic delivery has to be covered by safety stock. ' ||
        'How reliable are the suppliers'' lead times, and how much stock does that cost? ' ||
        '**Answer for ' || lt_qname(lt_q0()) || ':** ' ||
        (SELECT round(100 * avg((days_late <= 0)::INTEGER))::INTEGER FROM lt_lines WHERE closed_at >= lt_q0()) ||
        ' % of the order lines arrived on time (' ||
        (SELECT round(100 * avg((days_late <= 0)::INTEGER))::INTEGER FROM lt_lines WHERE closed_at >= lt_q1() AND closed_at < lt_q0()) ||
        ' % in ' || lt_qname(lt_q1()) || '). Least reliable: **' ||
        (SELECT supplier FROM lt_stats(lt_q0(), lt_today() + 1) ORDER BY on_time, supplier LIMIT 1) ||
        '** (*Reliability*). **' ||
        (SELECT a.supplier || '** got slower: its lead time rose from ' || round(b.mean_lt, 1) || ' days a year ago to ' || round(a.mean_lt, 1)
         FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats((lt_q0() - INTERVAL 1 YEAR)::DATE, (lt_q0() - INTERVAL 9 MONTH)::DATE) b USING (supplier)
         ORDER BY a.mean_lt / b.mean_lt DESC LIMIT 1) ||
        ' days (*Over time*). At a ' || lt_level_txt() || ' service level, lead-time variability ties up **€' ||
        (SELECT round(sum((ss_kings - ss_fixed) * unit_cost) / 1000, 1) FROM lt_ss(lt_q0(), lt_today() + 1)) ||
        'k** of safety stock, ' ||
        (SELECT round(100 * sum((ss_kings - ss_fixed) * unit_cost) / sum(ss_kings * unit_cost))::INTEGER FROM lt_ss(lt_q0(), lt_today() + 1)) ||
        ' % of it (*Safety stock*).')::MARKDOWN;

SELECT 'Inputs'::GROUP;
SELECT s.name AS supplier ::DROPDOWN, 'Supplier (highlighted)'::LABEL,
       replace(s.region, ', sea freight', ' (sea)') ::HINT
FROM lt_suppliers s ORDER BY s.name = 'Castings' DESC, s.supplier_id;
SELECT service_level ::BUTTONS, 'Service level'::LABEL, hint ::HINT
FROM (VALUES (1, '95 %', 'no stockout in 95 of 100 replenishment cycles'),
             (2, '90 %', 'no stockout in 90 of 100 cycles'),
             (3, '98 %', 'no stockout in 98 of 100 cycles'),
             (4, '99 %', 'no stockout in 99 of 100 cycles')) t(k, service_level, hint) ORDER BY k;
SELECT 1::ENDGROUP;

-- The selected supplier's quarter against the one before; the caption
-- carries the same figure over all suppliers.
SELECT (lt_sup() || ', ' || lt_qname(lt_q0()) || ' — captions: all suppliers')::GROUP;
SELECT 100 * a.on_time ::PERCENT, 100 * b.on_time ::DELTA,
       'abs'::DELTA_FORMAT, ('vs ' || lt_qname(lt_q1()))::DELTA_LABEL,
       ('On time · all ' || (SELECT round(100 * avg((days_late <= 0)::INTEGER))::INTEGER FROM lt_lines WHERE closed_at >= lt_q0()) || ' %')::LABEL,
       '<80:bad; <90:warn; good'::THRESHOLDS,
       'Share of the supplier''s order lines whose last receipt arrived by the date it promised. The caption: all suppliers'' lines.'::INFO
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier) WHERE a.supplier = lt_sup();
SELECT round(a.mean_lt, 1) ::METRIC, round(b.mean_lt, 1) ::DELTA_LOWER_GOOD,
       'abs'::DELTA_FORMAT, ('vs ' || lt_qname(lt_q1()))::DELTA_LABEL,
       ('Mean lead time, days · promised ' || a.promised)::LABEL,
       ('>=' || round(1.2 * a.promised, 1) || ':bad; >' || a.promised || ':warn; good')::THRESHOLDS,
       'Order day to the quantity-weighted mean receipt day, lines closed in the quarter. Amber above the promise, red 20 % above it.'::INFO
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier) WHERE a.supplier = lt_sup();
SELECT round(a.p90_lt, 1) ::METRIC, round(b.p90_lt, 1) ::DELTA_LOWER_GOOD,
       'abs'::DELTA_FORMAT, ('vs ' || lt_qname(lt_q1()))::DELTA_LABEL,
       '90 % arrive within, days'::LABEL,
       ('>=' || round(1.5 * a.promised, 1) || ':bad; >' || round(1.2 * a.promised, 1) || ':warn; good')::THRESHOLDS,
       'The 90 % quantile of the lead time: what to plan for when one late delivery in ten is acceptable.'::INFO
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier) WHERE a.supplier = lt_sup();
SELECT 100 * a.cv ::PERCENT, 100 * b.cv ::DELTA_LOWER_GOOD,
       'abs'::DELTA_FORMAT, ('vs ' || lt_qname(lt_q1()))::DELTA_LABEL,
       ('Variability (CV) · all ' || (SELECT round(100 * sum(cv * lines) / sum(lines))::INTEGER FROM lt_stats(lt_q0(), lt_today() + 1)) || ' %')::LABEL,
       '>=20:bad; >=10:warn; good'::THRESHOLDS,
       'Coefficient of variation: standard deviation / mean of the lead time. 10 % means a 20-day lead time typically varies by ±2 days. The caption: averaged over all suppliers by order lines.'::INFO
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier) WHERE a.supplier = lt_sup();
SELECT (SELECT round(sum((ss_kings - ss_fixed) * unit_cost)) FROM lt_ss(lt_q0(), lt_today() + 1) WHERE supplier = lt_sup()) ::METRIC,
       (SELECT round(sum((ss_kings - ss_fixed) * unit_cost)) FROM lt_ss(lt_q1(), lt_q0()) WHERE supplier = lt_sup()) ::DELTA_LOWER_GOOD,
       ('vs ' || lt_qname(lt_q1()))::DELTA_LABEL,
       ('Stock for lead-time variability, EUR · all ' ||
        (SELECT round(sum((ss_kings - ss_fixed) * unit_cost) / 1000, 1) FROM lt_ss(lt_q0(), lt_today() + 1)) || 'k')::LABEL,
       'Value of the safety stock that only exists because lead times vary: King''s formula minus the same stock with a fixed lead time, at the chosen service level, at unit cost. The caption: all suppliers.'::INFO;
SELECT 1::ENDGROUP;

-- ── 1 · Reliability ──────────────────────────────────────────────────────────
SELECT 'Reliability'::TAB;

SELECT 12::COL;
SELECT ('**Which suppliers deliver when they promise?** Over the last two quarters, **' ||
        (SELECT string_agg(supplier, '** and **' ORDER BY on_time DESC) FROM (SELECT * FROM lt_stats(lt_q1(), lt_today() + 1) ORDER BY on_time DESC LIMIT 2)) ||
        '** kept their promise most often; **' ||
        (SELECT supplier || '** least often (' || round(100 * on_time)::INTEGER || ' % on time, ' || round(late_by, 1) || ' days late when late)'
         FROM lt_stats(lt_q1(), lt_today() + 1) ORDER BY on_time LIMIT 1) ||
        '. A box above the zero line is a supplier whose promise is optimistic; a tall box one whose lead time you cannot plan on. ' ||
        'The selected supplier is drawn in blue; click another supplier in a chart or the table to select it everywhere.')::MARKDOWN;

SELECT 7::COL;
SELECT 340::HEIGHT;
SELECT supplier ::XAXIS, lt_pick(supplier) ::CATEGORY, lead_time - promised_lt ::BOXPLOT,
       0 ::REFLINE,
       '?v.supplier={value}' ::LINK,
       'Days late (+) or early (−) against the promise (0), per order line, last two quarters'::TITLE,
       'Lead time of each closed line minus the lead time the supplier promised for it. The box holds the middle half of the lines, the whiskers the rest up to 1.5 box heights, dots beyond are outliers. Lead time: order day to the quantity-weighted mean receipt day.'::INFO
FROM lt_lines WHERE closed_at >= lt_q1()
ORDER BY supplier;

SELECT 5::COL;
SELECT 340::HEIGHT;
SELECT supplier ::XAXIS, kind ::CATEGORY, v ::SCATTER, lo ::YMIN, hi ::YMAX, 1 ::FLIP,
       '?v.supplier={value}' ::LINK,
       'Promised vs actual lead time (days)'::TITLE,
       'Promised: the supplier''s contracted lead time. Actual (blue: the selected supplier): the median of the last two quarters, the line from the 10 % to the 90 % quantile.'::INFO
FROM (SELECT supplier, 'promised' AS kind, promised::DOUBLE AS v, NULL::DOUBLE AS lo, NULL::DOUBLE AS hi
      FROM lt_stats(lt_q1(), lt_today() + 1)
      UNION ALL
      SELECT supplier, lt_pick(supplier), quantile_cont(lead_time, 0.5), quantile_cont(lead_time, 0.1), quantile_cont(lead_time, 0.9)
      FROM lt_lines WHERE closed_at >= lt_q1() GROUP BY supplier)
ORDER BY supplier, kind;

SELECT 12::COL;
SELECT 'On time %: <80:bad; <90:warn; good | CV %: >=20:bad; >=10:warn; good'::THRESHOLDS;
SELECT a.supplier AS "Supplier" ::TABLE,
       CASE WHEN a.supplier = lt_sup() THEN 'selected' END AS "Shown" ::BADGE,
       s.region AS "Region", a.promised AS "Promised (days)",
       round(a.mean_lt, 1) AS "Mean (days)", round(a.p90_lt, 1) AS "90 % within (days)",
       round(100 * a.cv) AS "CV %",
       round(100 * a.on_time) AS "On time %", round(100 * b.on_time) AS "On time % before",
       round(a.late_by, 1) AS "Days late, when late",
       a.lines AS "Lines",
       (SELECT list(m ORDER BY mo) FROM (SELECT date_trunc('month', closed_at) AS mo, avg(lead_time) AS m
                                        FROM lt_lines l WHERE l.supplier = a.supplier GROUP BY 1)) AS "Mean lead time by month" ::SPARKLINE,
       '?v.supplier={value}' AS "Highlight" ::LINK,
       ('Every supplier, ' || lt_qname(lt_q0()) || ' (before: ' || lt_qname(lt_q1()) || ')')::TITLE,
       'Mean and 90 % quantile of the lead time of the lines closed in the quarter; CV = standard deviation / mean (the variability). The sparkline runs from Jan 2025.'::INFO
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier)
JOIN lt_suppliers s ON s.name = a.supplier
ORDER BY a.on_time, a.supplier;

-- ── 2 · Over time ────────────────────────────────────────────────────────────
SELECT 'Over time'::TAB;

SELECT 12::COL;
SELECT ('**Is ' || lt_sup() || ' getting slower?** In ' || lt_qname(lt_q0()) || ' its lead time averaged **' || round(a.mean_lt, 1) ||
        ' days** (90 % within ' || round(a.p90_lt, 1) || '), against ' || round(b.mean_lt, 1) || ' in ' || lt_qname(lt_q1()) ||
        ' and a promise of ' || a.promised || '. The lead-time monitor raised **' ||
        (SELECT count(*) FROM lt_alarms WHERE supplier = lt_sup()) || '** drift alarms over the period; planning now assumes **' ||
        (SELECT round(arg_max(assumed_lead_time, closed_at), 1) FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id) WHERE s.name = lt_sup()) ||
        ' days**. ' ||
        CASE WHEN EXISTS (SELECT 1 FROM lt_story()) THEN 'Step through the chart (‹ / Play / ›) to follow what happened.' ELSE '' END)::MARKDOWN
FROM lt_stats(lt_q0(), lt_today() + 1) a JOIN lt_stats(lt_q1(), lt_q0()) b USING (supplier)
WHERE a.supplier = lt_sup();

SELECT 12::COL;
SELECT 360::HEIGHT;
WITH wk AS (SELECT unnest(generate_series(DATE '2025-03-03', lt_today(), INTERVAL 7 DAY))::DATE AS wk),
fan AS (
    SELECT w.wk, count(*) AS n,
           quantile_cont(l.lead_time, 0.5) AS p50,
           quantile_cont(l.lead_time, 0.025) AS lo95, quantile_cont(l.lead_time, 0.975) AS hi95,
           quantile_cont(l.lead_time, 0.1) AS lo80, quantile_cont(l.lead_time, 0.9) AS hi80,
           quantile_cont(l.lead_time, 0.25) AS lo50, quantile_cont(l.lead_time, 0.75) AS hi50
    FROM wk w JOIN lt_lines l ON l.supplier = lt_sup() AND l.closed_at > w.wk - 49 AND l.closed_at <= w.wk + 6
    GROUP BY w.wk HAVING count(*) >= 5),
plan AS (
    SELECT w.wk, (SELECT arg_max(m.assumed_lead_time, m.closed_at) FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id)
                  WHERE s.name = lt_sup() AND m.closed_at <= w.wk + 6) AS assumed
    FROM wk w)
SELECT f.wk ::XAXIS, 'actual (median)' ::CATEGORY, f.p50 ::LINECHART,
       f.lo95 ::BAND_LOWER, f.hi95 ::BAND_UPPER, f.lo80 ::BAND_LOWER, f.hi80 ::BAND_UPPER,
       f.lo50 ::BAND_LOWER, f.hi50 ::BAND_UPPER,
       'solid' ::LINESTYLE,
       st.caption ::STORY,
       (SELECT promised_days FROM lt_suppliers WHERE name = lt_sup()) ::REFLINE, 'promised' ::REFLINE_LABEL,
       (lt_sup() || ': lead time in days, the last 8 weeks at each week (bands: 95 / 80 / 50 % of the lines)')::TITLE,
       'Each week: the lines that closed in the 8 weeks up to it. The line is their median lead time; the bands hold 95 %, 80 % and 50 % of them (darker = narrower). Dashed: the lead time planning assumed that week, as calibration moved it. The strips below share the time axis.'::INFO
FROM fan f LEFT JOIN lt_story() st ON st.wk = f.wk
UNION ALL
SELECT p.wk, 'assumed by planning', p.assumed, NULL, NULL, NULL, NULL, NULL, NULL, 'dashed', NULL, NULL, NULL, NULL, NULL
FROM plan p WHERE p.wk >= (SELECT min(wk) FROM fan) AND p.assumed IS NOT NULL
ORDER BY 2, 1;
SELECT closed_at ::XAXIS, 'Late lines' ::TRACK,
       po_id || ' (' || sku_id || '): ' || days_late || ' days late' ::LABEL
FROM lt_lines WHERE supplier = lt_sup() AND days_late > 0 AND closed_at >= DATE '2025-03-03' ORDER BY closed_at;
SELECT w.wk ::XAXIS, d.kind ::CATEGORY, 1 ::HEATMAP, d.what || ' (' || strftime(d.start_date, '%-d %b') || ' – ' || strftime(d.end_date, '%-d %b %Y') || ')' ::LABEL,
       'Disruptions' ::TRACK
FROM (SELECT unnest(generate_series(DATE '2025-03-03', lt_today(), INTERVAL 7 DAY))::DATE AS wk) w
JOIN lt_disruptions d ON d.start_date <= w.wk + 6 AND d.end_date >= w.wk
JOIN lt_suppliers s USING (supplier_id)
WHERE s.name = lt_sup() ORDER BY w.wk;
SELECT date_trunc('week', m.closed_at)::DATE ::XAXIS,
       CASE max(m.state) WHEN 'warning' THEN '#e8a23b' ELSE '#d64545' END ::CATEGORY,
       1 ::HEATMAP,
       'drift ' || max(m.state) || ': CUSUM ' || round(max(m.cusum), 1) || ' (warning at ' || any_value(m.warning_threshold) || ', alarm at ' || any_value(m.alarm_threshold) || ')' ::LABEL,
       'Drift monitor' ::TRACK
FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id)
WHERE s.name = lt_sup() AND m.state <> 'ok' AND m.closed_at >= DATE '2025-03-03'
GROUP BY 1 ORDER BY 1;

SELECT 12::COL;
SELECT 280::HEIGHT;
SELECT m.closed_at ::XAXIS, m.cusum ::LINECHART,
       CASE m.closed_at WHEN f.d0 THEN m.warning_threshold WHEN f.d1 THEN m.alarm_threshold END ::REFLINE,
       CASE m.closed_at WHEN f.d0 THEN 'warning' WHEN f.d1 THEN 'alarm' END ::REFLINE_LABEL,
       CASE WHEN m.state = 'alarm' THEN m.closed_at END ::MARKAREA,
       (lt_sup() || ': drift monitor on the lead-time error (shaded: in alarm)')::TITLE,
       'One-sided CUSUM of (actual − assumed lead time) / assumed spread, minus a slack of 0.5. It climbs while lines keep arriving later than planning assumes and drops back to zero when a recalibration moves the assumption.'::INFO
FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id),
     (SELECT min(closed_at) AS d0, min(closed_at) FILTER (WHERE closed_at > (SELECT min(closed_at) FROM lt_monitor x WHERE x.supplier_id = m.supplier_id)) AS d1
      FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id) WHERE s.name = lt_sup()) f
WHERE s.name = lt_sup()
ORDER BY m.closed_at, m.po_id;

SELECT 12::COL;
SELECT 'status = escalated'::FLAG;
SELECT strftime(p.ds, '%-d %b %Y') AS "Day" ::TABLE,
       CASE p.parameter WHEN 'supplier.lead_time_mean' THEN 'Lead time' ELSE 'Spread' END AS "Parameter",
       round(p.old_num, 1) AS "Before (days)", round(p.new_num, 1) AS "After (days)", p.sample_size AS "Lines",
       p.trigger AS "Trigger", p.status AS "Status" ::BADGE,
       coalesce(strftime(p.decided_at, '%-d %b'), '—') AS "In effect from",
       (lt_sup() || ': how calibration moved the planned lead time')::TITLE,
       'Calibration re-estimates the planned lead time on the first of each month from the last 90 days of lines, and the day after a drift alarm from the last 45. A change of the mean up to 10 % applies on its own; a larger one is sent to the planner.'::INFO
FROM lt_proposals p JOIN lt_suppliers s USING (supplier_id)
WHERE s.name = lt_sup() AND (p.trigger = 'alarm' OR p.status <> 'applied' OR p.ds >= lt_q1())
ORDER BY p.ds DESC, p.parameter;

-- ── 3 · Safety stock ─────────────────────────────────────────────────────────
SELECT 'Safety stock'::TAB;

SELECT 12::COL;
SELECT '**What does lead-time variability cost?** Safety stock covers the demand that can arrive before the next delivery. With a fixed lead time it only has to cover the variation of demand; when the lead time varies too, King''s formula adds a second term:

$$SS = z\sqrt{\bar L\,\sigma_D^2 + \bar D^2\,\sigma_L^2}$$

$z$ is the service-level factor ({{z}} at {{lvl}}), $\bar L$ and $\sigma_L$ the mean and standard deviation of the lead time (days, lines closed in {{q}}), $\bar D$ and $\sigma_D$ those of daily demand. With $\sigma_L = 0$ it falls back to $z\,\sigma_D\sqrt{\bar L}$. Over all items the second term adds **€{{extra}}k** of safety stock to the **€{{fixed}}k** a fixed lead time would need; **{{worst}}** accounts for the most; the selected **{{sup}}** for €{{sel}}k (blue below). Change the service level to see the charts move.'::MARKDOWN,
       lt_sup() AS sup,
       (SELECT round(sum((ss_kings - ss_fixed) * unit_cost) / 1000, 1) FROM lt_ss(lt_q0(), lt_today() + 1) WHERE supplier = lt_sup()) AS sel,
       round(lt_z(), 2) AS z, lt_level_txt() AS lvl, lt_qname(lt_q0()) AS q,
       (SELECT round(sum((ss_kings - ss_fixed) * unit_cost) / 1000, 1) FROM lt_ss(lt_q0(), lt_today() + 1)) AS extra,
       (SELECT round(sum(ss_fixed * unit_cost) / 1000, 1) FROM lt_ss(lt_q0(), lt_today() + 1)) AS fixed,
       (SELECT supplier FROM lt_ss(lt_q0(), lt_today() + 1) GROUP BY supplier ORDER BY sum((ss_kings - ss_fixed) * unit_cost) DESC LIMIT 1) AS worst;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT supplier ::XAXIS, lt_pick(supplier) ::CATEGORY, round(sum((ss_kings - ss_fixed) * unit_cost)) ::BARCHART, 1 ::FLIP, '€' ::YFORMAT,
       'tween' ::TRANSITION, '?v.supplier={value}' ::LINK,
       ('Safety stock added by lead-time variability at ' || lt_level_txt() || ' (EUR)')::TITLE,
       'King''s formula minus the stock a fixed lead time would need, at unit cost, summed over each supplier''s items. Blue: the selected supplier.'::INFO
FROM lt_ss(lt_q0(), lt_today() + 1) GROUP BY ALL ORDER BY supplier;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT supplier ::XAXIS, lt_pick(supplier) ::CATEGORY, round(sum((fill_kings - fill_fixed) * d * 365 * unit_cost)) ::BARCHART, 1 ::FLIP, '€' ::YFORMAT,
       'tween' ::TRANSITION, '?v.supplier={value}' ::LINK,
       ('Sales a fixed-lead-time plan would lose per year at ' || lt_level_txt() || ' (EUR)')::TITLE,
       'Demand not served from stock in a year at unit cost when the safety stock is planned as if the lead time were fixed, minus the same with King''s formula. Fill rate per order cycle of one review period: 1 − σ·G(k)/Q, where σ is the spread of demand over the real (varying) lead time, k the safety stock in units of σ, G the normal loss function and Q the order quantity. Blue: the selected supplier.'::INFO
FROM lt_ss(lt_q0(), lt_today() + 1) GROUP BY ALL ORDER BY supplier;

SELECT 12::COL;
SELECT sku_id AS "Item" ::TABLE, description AS "Description", supplier AS "Supplier",
       CASE WHEN supplier = lt_sup() THEN 'selected' END AS "Shown" ::BADGE,
       round(d, 1) AS "Demand per day", round(sd_d, 1) AS "Demand σ",
       round(l, 1) AS "Lead time (days)", round(sd_l, 1) AS "Lead time σ",
       round(ss_fixed) AS "Safety stock, fixed lead time", round(ss_kings) AS "Safety stock, King",
       round((ss_kings - ss_fixed) * unit_cost) AS "Extra for lead-time variability (EUR)",
       round(100 * fill_kings, 1) AS "Fill rate %",
       ('Every item at ' || lt_level_txt() || ', lead times of ' || lt_qname(lt_q0()) || ': ' || lt_sup() || ' first, then by the extra')::TITLE
FROM lt_ss(lt_q0(), lt_today() + 1)
ORDER BY supplier = lt_sup() DESC, (ss_kings - ss_fixed) * unit_cost DESC;

-- ── 4 · Orders ───────────────────────────────────────────────────────────────
SELECT 'Orders'::TAB;

SELECT 12::COL;
SELECT ('**' || lt_sup() || ': every order line.** ' || count(*) FILTER (WHERE closed_at >= lt_q1()) || ' lines closed in the last two quarters, ' ||
        count(*) FILTER (WHERE closed_at >= lt_q1() AND days_late > 0) || ' of them late. ' ||
        (SELECT count(*) FROM lt_po h JOIN lt_suppliers s USING (supplier_id) WHERE s.name = lt_sup() AND h.status IN ('open', 'partially_received')) ||
        ' lines are still on order. Pick another supplier at the top.')::MARKDOWN
FROM lt_lines WHERE supplier = lt_sup();

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT ordered_at ::XAXIS, CASE WHEN days_late > 0 THEN 'late' ELSE 'on time' END ::CATEGORY, lead_time ::SCATTER,
       promised_lt ::REFLINE, 'promised' ::REFLINE_LABEL,
       (lt_sup() || ': lead time of each order line by order day (days)')::TITLE
FROM lt_lines WHERE supplier = lt_sup() ORDER BY ordered_at;

SELECT 12::COL;
SELECT h.po_id AS "Order" ::TABLE, h.sku_id AS "Item", strftime(h.ordered_at, '%-d %b %Y') AS "Ordered", h.ordered_qty AS "Qty",
       strftime(h.promised_at, '%-d %b') AS "Promised",
       strftime(h.ordered_at + round(m.assumed)::INTEGER, '%-d %b') AS "Expected (planning)",
       coalesce((SELECT sum(received_qty) FROM lt_receipt r WHERE r.po_id = h.po_id), 0) AS "Received",
       CASE WHEN h.promised_at < lt_today() THEN 'overdue' ELSE replace(h.status, '_', ' ') END AS "Status" ::BADGE,
       (lt_sup() || ': on order now')::TITLE,
       'Open and partly received lines. Expected: the order day plus the lead time planning assumes now.'::INFO
FROM lt_po h JOIN lt_suppliers s USING (supplier_id),
     (SELECT arg_max(assumed_lead_time, closed_at) AS assumed FROM lt_monitor m JOIN lt_suppliers s USING (supplier_id) WHERE s.name = lt_sup()) m
WHERE s.name = lt_sup() AND h.status IN ('open', 'partially_received')
ORDER BY h.ordered_at;
SELECT 12::COL;
SELECT 'Days late: >=4:bad; >=1:warn; good'::THRESHOLDS;
SELECT po_id AS "Order" ::TABLE, sku_id AS "Item", strftime(ordered_at, '%-d %b %Y') AS "Ordered",
       strftime(promised_at, '%-d %b') AS "Promised", strftime(closed_at, '%-d %b') AS "Received",
       round(lead_time, 1) AS "Lead time (days)", days_late AS "Days late", receipts AS "Shipments",
       CASE WHEN days_late > 0 THEN 'late' ELSE 'on time' END AS "On time" ::BADGE,
       (lt_sup() || ': closed lines, newest first')::TITLE,
       'Lead time: order day to the quantity-weighted mean receipt day. Days late: last receipt minus the promised date (negative = early).'::INFO
FROM lt_lines WHERE supplier = lt_sup()
ORDER BY closed_at DESC, po_id;
