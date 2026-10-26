-- @title Where does our supply chain amplify demand, and what would stabilise it?
-- @group Logistics & production
-- @tags  inventory, simulation, bullwhip
-- @description How orders swing more at each stage of a four-stage chain, and which policy calms them.
-- @tour amplify_tour Where the bullwhip comes from
--   page: A four-stage supply chain, simulated for 52 weeks — see where small swings in consumer sales turn into big swings in orders upstream, and what would calm them.
--   chain_diagram: Each stage's bullwhip ratio, and the goods and orders flowing between them.
--   stock_backlog_chart: Stock on hand and backlog over the 52 weeks, for the stage you've picked.
--   policy_table: Every policy compared — pick the calmest, or the cheapest.
-- @tour_autostart amplify_tour
--
-- Where does our supply chain amplify demand, and what would stabilise it?
--
-- A four-stage supply chain simulated for 52 weeks: consumers buy from a
-- retailer, the retailer orders from a distribution centre, the DC from a
-- factory (capacity 150 units/week), the factory buys components from a
-- supplier. Every stage forecasts, orders up to a target stock and keeps
-- unserved orders open. Small swings in consumer sales grow into large
-- swings in orders further up the chain: the bullwhip effect.
--
-- The runs are stored, not computed here: examples/supply-chain/simulate.py
-- (Python standard library, seed 7) simulates every scenario x information
-- policy x order smoothing (18 runs) and writes
-- web/data/supply-chain/weeks.csv (one row per run, stage and week). The
-- inputs pick one run. See examples/supply-chain/README.md for the model.
--
-- Natively, run from web/:  dashboard --check --strict examples/supply-chain.sql

SELECT 'Where does our supply chain amplify demand, and what would stabilise it?'::DASHBOARD_TITLE;

-- ── Data ──────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE sc_weeks AS FROM read_csv('data/supply-chain/weeks.csv');

CREATE OR REPLACE TABLE sc_stage AS SELECT * FROM (VALUES
  (0, 'Consumers', 'Consumers', 'Consumers'),
  (1, 'Retailer', 'Retailer', 'Retailer'),
  (2, 'Distribution centre', 'Distribution centre', 'DC'),
  (3, 'Factory', 'Factory', 'Factory'),
  (4, 'Supplier', 'Component supplier', 'Supplier')
) t(stage_no, stage, title, short);

-- What each scenario does to the chain (the event strip reads these).
CREATE OR REPLACE TABLE sc_windows AS SELECT * FROM (VALUES
  ('Promotion', 'Promotion', 13, 16, 'Promotion: consumer demand +60 % for four weeks'),
  ('Promotion', 'Promotion', 17, 18, 'After the promotion: demand 15 % below normal (bought ahead)'),
  ('Supplier outage', 'Supplier outage', 19, 26, 'Supplier outage: at most 35 components a week')
) t(scenario, lane, w0, w1, note);

-- The inputs (unset in a fresh session: the first option).
CREATE OR REPLACE MACRO sc_scenario() AS coalesce(getvariable('scenario'), 'Promotion');
CREATE OR REPLACE MACRO sc_policy() AS coalesce(getvariable('sharing'), 'Own forecasts');
CREATE OR REPLACE MACRO sc_smoothing() AS coalesce(getvariable('smoothing'), 'None');
-- The clicked stage; the retailer when nothing (or the consumers) is clicked.
CREATE OR REPLACE MACRO sc_pick() AS
  CASE WHEN coalesce(getvariable('stage'), '') IN ('', 'Consumers') THEN 'Retailer' ELSE getvariable('stage') END;

-- The selected run, and the baseline every change is measured against.
CREATE OR REPLACE MACRO sc_run() AS TABLE
  SELECT * FROM sc_weeks WHERE scenario = sc_scenario() AND policy = sc_policy() AND smoothing = sc_smoothing();

-- Per run and stage: the bullwhip ratio (variance of the orders a stage
-- places / variance of consumer demand), service and cost.
CREATE OR REPLACE TABLE sc_summary AS
SELECT scenario, policy, smoothing, stage, any_value(stage_no) AS stage_no,
       var_samp(order_out) / var_samp(consumer_demand) AS bullwhip,
       min(order_out) AS order_min, max(order_out) AS order_max,
       arg_max(week, order_out) AS order_peak_week,
       100 * sum(on_time) / sum(demand_in) AS fill_rate,
       count(*) FILTER (WHERE backlog > 0) AS weeks_short,
       max(backlog) AS peak_backlog, avg(on_hand) AS avg_stock,
       sum(cost_holding) AS cost_holding, sum(cost_backlog) AS cost_backlog,
       sum(shipped) AS shipped,
       count(*) FILTER (WHERE capacity_bound) AS weeks_at_capacity,
       count(*) FILTER (WHERE late_shipment) AS late_shipments
FROM sc_weeks GROUP BY ALL;

CREATE OR REPLACE TABLE sc_runs AS
SELECT scenario, policy, smoothing,
       max(bullwhip) FILTER (WHERE stage = 'Supplier') AS bullwhip_top,
       max(fill_rate) FILTER (WHERE stage = 'Retailer') AS customer_fill,
       max(weeks_short) FILTER (WHERE stage = 'Retailer') AS customer_weeks_short,
       sum(cost_holding + cost_backlog) AS cost,
       sum(cost_holding) AS cost_holding, sum(cost_backlog) AS cost_backlog
FROM sc_summary GROUP BY ALL;

CREATE OR REPLACE MACRO sc_cur() AS TABLE
  SELECT * FROM sc_summary WHERE scenario = sc_scenario() AND policy = sc_policy() AND smoothing = sc_smoothing();
CREATE OR REPLACE MACRO sc_now() AS TABLE
  SELECT * FROM sc_runs WHERE scenario = sc_scenario() AND policy = sc_policy() AND smoothing = sc_smoothing();
CREATE OR REPLACE MACRO sc_base() AS TABLE
  SELECT * FROM sc_runs WHERE scenario = sc_scenario() AND policy = 'Own forecasts' AND smoothing = 'None';

-- ── Header: the inputs and the run at a glance ───────────────────────────────
SELECT 'Run'::GROUP;
SELECT scenario ::BUTTONS, 'Scenario'::LABEL
FROM (VALUES (1, 'Promotion'), (2, 'Supplier outage'), (3, 'Normal')) t(k, scenario) ORDER BY k;
SELECT sharing ::BUTTONS, 'Who sees consumer sales'::LABEL, hint ::HINT
FROM (VALUES (1, 'Own forecasts', 'each stage forecasts from the orders it receives'),
             (2, 'Shared sales data', 'every stage plans on consumer sales (vendor-managed inventory)')) t(k, sharing, hint)
ORDER BY k;
SELECT smoothing ::BUTTONS, 'Order smoothing'::LABEL, hint ::HINT
FROM (VALUES (1, 'None', 'close the whole stock gap with this week''s order'),
             (2, '2 weeks', 'close a stock gap over two weeks'),
             (3, '4 weeks', 'close a stock gap over four weeks')) t(k, smoothing, hint)
ORDER BY k;
SELECT 1::ENDGROUP;

SELECT 'This run against each stage ordering on its own, unsmoothed'::GROUP;
SELECT n.bullwhip_top::METRIC, CASE WHEN sc_policy() <> 'Own forecasts' OR sc_smoothing() <> 'None' THEN b.bullwhip_top END::DELTA_LOWER_GOOD, 'vs own forecasts, no smoothing'::DELTA_LABEL,
       'Bullwhip at the supplier (×)'::LABEL, '>=8:bad; >=3:warn; good'::THRESHOLDS,
       'How many times more the supplier''s orders vary from week to week than consumer sales do (variance ratio). 1 = no amplification.'::INFO
FROM sc_now() n, sc_base() b;
SELECT n.customer_fill::PERCENT, CASE WHEN sc_policy() <> 'Own forecasts' OR sc_smoothing() <> 'None' THEN b.customer_fill END::DELTA, 'vs own forecasts, no smoothing'::DELTA_LABEL,
       'Consumer fill rate'::LABEL, '<90:bad; <97:warn; good'::THRESHOLDS,
       'Share of consumer demand the retailer served from stock in the week it was asked for. The rest waits (backlog).'::INFO
FROM sc_now() n, sc_base() b;
SELECT n.customer_weeks_short::METRIC, CASE WHEN sc_policy() <> 'Own forecasts' OR sc_smoothing() <> 'None' THEN b.customer_weeks_short END::DELTA_LOWER_GOOD, 'vs own forecasts, no smoothing'::DELTA_LABEL,
       'Weeks consumers waited'::LABEL, '>=8:bad; >=3:warn; good'::THRESHOLDS
FROM sc_now() n, sc_base() b;
SELECT n.cost::MONEY, CASE WHEN sc_policy() <> 'Own forecasts' OR sc_smoothing() <> 'None' THEN b.cost END::DELTA_LOWER_GOOD, 'vs own forecasts, no smoothing'::DELTA_LABEL,
       'Chain cost, 52 weeks'::LABEL,
       'Holding cost ($1.00 / 0.60 / 0.40 / 0.20 per unit and week at retailer / DC / factory / supplier) plus backlog cost ($4 per unit and week a consumer waits, $1.50 for a stage waiting).'::INFO
FROM sc_now() n, sc_base() b;
SELECT 1::ENDGROUP;

-- ── 1 · Where it amplifies ───────────────────────────────────────────────────
SELECT 'Where it amplifies'::TAB;

SELECT 12::COL;
SELECT 260::HEIGHT;
SELECT 'stage'::SELECT_AS;
-- @name chain_diagram
WITH s AS (
  SELECT st.stage_no, st.stage, st.title, st.short,
         CASE WHEN st.stage = 'Consumers' THEN 1.0 ELSE c.bullwhip END AS ratio,
         c.order_max, c.order_peak_week, c.weeks_at_capacity, c.weeks_short, c.peak_backlog, c.late_shipments,
         (SELECT list(CASE WHEN st.stage = 'Consumers' THEN r.consumer_demand ELSE r.order_out END ORDER BY r.week)
          FROM sc_run() r WHERE r.stage = CASE WHEN st.stage = 'Consumers' THEN 'Retailer' ELSE st.stage END) AS orders
  FROM sc_stage st LEFT JOIN sc_cur() c USING (stage)),
d AS (SELECT max(consumer_demand) AS peak, arg_max(week, consumer_demand) AS peak_week,
             round(stddev_samp(consumer_demand)) AS sd FROM sc_run() WHERE stage = 'Retailer')
SELECT s.stage ::DIAGRAM, s.title ::LABEL, round(s.ratio, 1) ::METRIC,
       '<3:good; <8:warn; bad'::THRESHOLDS,
       s.orders ::SPARKLINE,
       CASE s.stage
         WHEN 'Consumers' THEN 'Consumers buy up to ' || d.peak::INTEGER || ' units in week ' || d.peak_week || ' (± ' || d.sd::INTEGER || ' a week): the only real demand in the chain'
         ELSE s.title || ' orders vary ' || round(s.ratio, 1) || '× as much as consumer sales; its largest order, ' || s.order_max::INTEGER || ' units, goes out in week ' || s.order_peak_week ||
              CASE WHEN s.stage = 'Factory' AND s.weeks_at_capacity > 0 THEN '; production runs at its 150-unit limit in ' || s.weeks_at_capacity || ' weeks' ELSE '' END ||
              CASE WHEN s.stage = 'Supplier' AND sc_scenario() = 'Supplier outage' THEN '; during the outage its backlog to the factory peaks at ' || s.peak_backlog::INTEGER || ' units' ELSE '' END
       END ::STORY,
       (SELECT list(x.e ORDER BY x.k) FROM (
          -- material: what each stage shipped downstream, coloured by how often it fell short
          SELECT 10 + c.stage_no AS k,
                 {'from': c.stage, 'to': CASE c.stage WHEN 'Retailer' THEN 'Consumers' WHEN 'Distribution centre' THEN 'Retailer'
                                                      WHEN 'Factory' THEN 'Distribution centre' ELSE 'Factory' END,
                  'value': round(c.shipped), 'label': round(c.shipped / 1000, 1) || 'K',
                  'style': 'flow-volume',
                  'status': CASE WHEN c.weeks_short > 8 THEN 'failed' WHEN c.weeks_short > 2 THEN 'delayed' ELSE 'ok' END} AS e
          FROM sc_cur() c
          UNION ALL
          -- information: orders up the chain, and consumer sales when they are shared
          SELECT 20 + st.stage_no,
                 {'from': CASE st.stage WHEN 'Retailer' THEN 'Consumers' WHEN 'Distribution centre' THEN 'Retailer'
                                        WHEN 'Factory' THEN 'Distribution centre' ELSE 'Factory' END,
                  'to': st.stage, 'value': NULL::DOUBLE, 'label': 'orders', 'style': 'dashed', 'status': NULL::VARCHAR}
          FROM sc_stage st WHERE st.stage_no > 0
          UNION ALL
          SELECT 30 + st.stage_no,
                 {'from': 'Consumers', 'to': st.stage, 'value': NULL::DOUBLE, 'label': 'shared sales data', 'style': 'dashed', 'status': NULL::VARCHAR}
          FROM sc_stage st WHERE st.stage_no > 1 AND sc_policy() = 'Shared sales data') x(k, e)) ::EDGES,
       'tween'::TRANSITION,
       ('Goods flow to consumers, orders flow back: the ' || lower(sc_scenario()) || ' run, ' || lower(sc_policy()) || '. Click a stage for its stock')::TITLE,
       'Boxes: each stage with its bullwhip ratio (× the variance of consumer sales) and its weekly orders. Solid edges: goods shipped over 52 weeks, as wide as the volume, amber / red when the shipping stage fell short in 3+ / 9+ weeks. Dashed edges: information — the orders each stage sees, and with shared sales data the consumer sales every stage plans on.'::INFO
FROM s, d ORDER BY s.stage_no;

SELECT 'Bullwhip ratio by stage (×)'::GROUP;
SELECT round(bullwhip, 2)::METRIC, (stage || CASE stage_no WHEN 1 THEN '' ELSE ' (step: ×' || round(bullwhip / lag_b, 1) || ')' END)::LABEL,
       '>=8:bad; >=3:warn; good'::THRESHOLDS
FROM (SELECT *, lag(bullwhip) OVER (ORDER BY stage_no) AS lag_b FROM sc_cur()) WHERE stage = 'Retailer';
SELECT round(bullwhip, 2)::METRIC, (stage || ' (step: ×' || round(bullwhip / lag_b, 1) || ')')::LABEL,
       '>=8:bad; >=3:warn; good'::THRESHOLDS
FROM (SELECT *, lag(bullwhip) OVER (ORDER BY stage_no) AS lag_b FROM sc_cur()) WHERE stage = 'Distribution centre';
SELECT round(bullwhip, 2)::METRIC, (stage || ' (step: ×' || round(bullwhip / lag_b, 1) || ')')::LABEL,
       '>=8:bad; >=3:warn; good'::THRESHOLDS
FROM (SELECT *, lag(bullwhip) OVER (ORDER BY stage_no) AS lag_b FROM sc_cur()) WHERE stage = 'Factory';
SELECT round(bullwhip, 2)::METRIC, (stage || ' (step: ×' || round(bullwhip / lag_b, 1) || ')')::LABEL,
       '>=8:bad; >=3:warn; good'::THRESHOLDS,
       'The step factor is how much this stage amplifies the orders it receives: its ratio divided by the ratio of the stage below.'::INFO
FROM (SELECT *, lag(bullwhip) OVER (ORDER BY stage_no) AS lag_b FROM sc_cur()) WHERE stage = 'Supplier';
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT week ::XAXIS, series ::CATEGORY, units ::LINECHART,
       'tween'::TRANSITION,
       'Orders swing harder the further they are from the consumer'::TITLE,
       'Consumer sales and the order each stage places with the stage above it, per week. The strip below marks the promotion, the supplier outage, the weeks the factory ran at its capacity and late component deliveries.'::INFO
FROM (SELECT r.week, '0 Consumer sales' AS k, 'Consumer sales' AS series, r.consumer_demand AS units FROM sc_run() r WHERE r.stage = 'Retailer'
      UNION ALL
      SELECT r.week, r.stage_no::VARCHAR, st.short || ' orders', r.order_out FROM sc_run() r JOIN sc_stage st USING (stage))
ORDER BY k, week;
SELECT week ::XAXIS, lane ::YAXIS, lane ::CATEGORY, what ::LABEL, 'Events' ::TRACK
FROM (SELECT w.week, s.lane, s.note AS what
      FROM sc_windows s, range(1, 53) w(week)
      WHERE s.scenario = sc_scenario() AND w.week BETWEEN s.w0 AND s.w1
      UNION ALL
      SELECT week, 'Factory at capacity', 'Factory at its 150-unit limit, ' || produced::INTEGER || ' made' FROM sc_run() WHERE stage = 'Factory' AND capacity_bound
      UNION ALL
      SELECT week, 'Late delivery', 'Components arrive a week late' FROM sc_run() WHERE stage = 'Supplier' AND late_shipment)
ORDER BY week, lane;

SELECT 12::COL;
SELECT ('**How the bullwhip is measured.** For each stage $i$, with $O_{i,t}$ the order it places in week $t$ and $D_t$ consumer demand:' ||
        ' $$B_i = \frac{\operatorname{Var}(O_i)}{\operatorname{Var}(D)}, \qquad \text{step}_i = \frac{B_i}{B_{i-1}}$$' ||
        ' In this run the supplier''s orders vary **{{supplier_ratio:metric}}×** as much as consumer sales. Each stage orders' ||
        ' $O_t = \hat D_t + \frac{1}{T_s}\left(SS + \hat D_t (L-1) - IP_t\right)$:' ||
        ' its forecast $\hat D_t$, plus the gap to its safety stock $SS$ and a full pipeline over $L$ weeks lead time, closed over $T_s$ weeks of order smoothing.')::MARKDOWN,
       'note'::CALLOUT, n.bullwhip_top AS supplier_ratio
FROM sc_now() n;

-- ── 1b · Where it flows ───────────────────────────────────────────────────────
-- A flow map (::ORIGIN/::DEST, D17): goods shipped downstream (solid) and
-- orders placed upstream ('flow'::LINESTYLE, animated), width by peak
-- weekly quantity. In the default run (Promotion / Own forecasts / no
-- smoothing), order peaks grow at every step upstream — checked against
-- the data in the SQL comments below.
CREATE OR REPLACE TABLE sc_site AS
  SELECT stage_no, stage, lon::DOUBLE AS lon, lat::DOUBLE AS lat, city FROM (VALUES
  (1, 'Retailer', 10.00, 53.55, 'Hamburg'), (2, 'Distribution centre', 9.48, 51.31, 'Kassel'),
  (3, 'Factory', 9.18, 48.78, 'Stuttgart'), (4, 'Supplier', 12.92, 50.83, 'Chemnitz')) t(stage_no, stage, lon, lat, city);
SELECT 'Where it flows'::TAB;
SELECT 12::COL; SELECT 560::HEIGHT;
SELECT {'lon': a.lon, 'lat': a.lat, 'name': a.stage || ' (' || a.city || ')'} ::ORIGIN,
       {'lon': b.lon, 'lat': b.lat, 'name': b.stage || ' (' || b.city || ')'} ::DEST,
       f.peak AS "Peak units per week" ::SIZE, f.kind ::CATEGORY, f.style ::LINESTYLE,
       'de-states'::BASEMAP,
       'Order peaks grow at every step upstream'::TITLE,
       'Goods shipped downstream (solid) and orders placed upstream (dashed, animated), width by the peak weekly quantity; the four stages are fictional sites.'::INFO
FROM (SELECT stage_no AS src, stage_no - 1 AS dst, 'Goods shipped' AS kind, 'solid' AS style, max(shipped) AS peak
      FROM sc_run() WHERE stage_no > 1 GROUP BY ALL
      UNION ALL
      SELECT stage_no, stage_no + 1, 'Orders placed', 'flow', max(order_out) FROM sc_run() WHERE stage_no < 4 GROUP BY ALL) f
JOIN sc_site a ON a.stage_no = f.src JOIN sc_site b ON b.stage_no = f.dst ORDER BY peak DESC;

-- ── 2 · Stock and service ────────────────────────────────────────────────────
SELECT 'Stock and service'::TAB;

SELECT 12::COL;
SELECT ('**' || s.title || '** in the ' || lower(sc_scenario()) || ' run: it served **{{fill:percent}}** of what was asked for in the same week, ran short in **' ||
        c.weeks_short || '** of 52 weeks and held **{{stock:metric}}** units on average. Click a stage in the diagram (tab *Where it amplifies*) or a bar below to switch.')::MARKDOWN,
       round(c.fill_rate, 1) AS fill, round(c.avg_stock) AS stock
FROM sc_cur() c JOIN sc_stage s USING (stage) WHERE c.stage = sc_pick();

SELECT (sc_pick() || ', 52 weeks')::GROUP;
SELECT round(fill_rate, 1)::PERCENT, 'Fill rate'::LABEL, '<90:bad; <97:warn; good'::THRESHOLDS,
       'Share of the orders received (consumer demand at the retailer) shipped in the week they came in.'::INFO
FROM sc_cur() WHERE stage = sc_pick();
SELECT weeks_short::METRIC, 'Weeks with a backlog'::LABEL, '>=8:bad; >=3:warn; good'::THRESHOLDS
FROM sc_cur() WHERE stage = sc_pick();
SELECT round(peak_backlog)::METRIC, 'Peak backlog (units)'::LABEL FROM sc_cur() WHERE stage = sc_pick();
SELECT round(avg_stock)::METRIC, 'Average stock (units)'::LABEL,
       (SELECT list(on_hand ORDER BY week) FROM sc_run() WHERE stage = sc_pick())::SPARKLINE
FROM sc_cur() WHERE stage = sc_pick();
SELECT cost_holding + cost_backlog ::MONEY, 'Cost'::LABEL FROM sc_cur() WHERE stage = sc_pick();
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
-- @name stock_backlog_chart
SELECT r.week ::XAXIS, v.series ::CATEGORY, v.units ::LINECHART,
       CASE WHEN v.series = 'On hand' AND r.backlog > 0 THEN r.week END ::MARKAREA,
       0 ::REFLINE,
       'tween'::TRANSITION,
       (sc_pick() || ': stock on hand and backlog per week (shaded: out of stock, orders waiting)')::TITLE,
       'On hand: units in stock at the end of the week. Backlog: units ordered from this stage (by consumers at the retailer) and not yet shipped. Shaded weeks: the stage could not ship everything it owed.'::INFO
FROM sc_run() r CROSS JOIN LATERAL (VALUES ('On hand', r.on_hand), ('Backlog', r.backlog)) v(series, units)
WHERE r.stage = sc_pick()
ORDER BY v.series DESC, r.week;
SELECT week ::XAXIS, lane ::YAXIS, lane ::CATEGORY, what ::LABEL, 'Events' ::TRACK
FROM (SELECT w.week, s.lane, s.note AS what
      FROM sc_windows s, range(1, 53) w(week)
      WHERE s.scenario = sc_scenario() AND w.week BETWEEN s.w0 AND s.w1
      UNION ALL
      SELECT week, 'Factory at capacity', 'Factory at its 150-unit limit, ' || produced::INTEGER || ' made' FROM sc_run() WHERE stage = 'Factory' AND capacity_bound
      UNION ALL
      SELECT week, 'Late delivery', 'Components arrive a week late' FROM sc_run() WHERE stage = 'Supplier' AND late_shipment)
ORDER BY week, lane;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT 'stage'::SELECT_AS;
SELECT c.stage ::XAXIS, v.kind ::CATEGORY, v.eur ::BARCHART_STACKED, '$'::YFORMAT, 1::FLIP,
       'tween'::TRANSITION,
       'Where the chain''s cost lands (click a stage)'::TITLE,
       'Holding: stock on hand × the stage''s cost per unit and week. Backlog: units owed and not shipped × $4 (consumers) or $1.50 (a stage).'::INFO
FROM sc_cur() c CROSS JOIN LATERAL (VALUES ('holding', c.cost_holding), ('backlog', c.cost_backlog)) v(kind, eur)
ORDER BY c.cost_holding + c.cost_backlog, v.kind DESC;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT stage ::XAXIS, round(fill_rate, 1) ::BARCHART, 'percent'::YFORMAT, 97 ::REFLINE, 'target 97 %'::REFLINE_LABEL, 1::FLIP,
       'tween'::TRANSITION,
       'Fill rate by stage: who kept up with the orders it received'::TITLE
FROM sc_cur() ORDER BY fill_rate DESC;

-- ── 3 · What would stabilise it ──────────────────────────────────────────────
SELECT 'What would stabilise it'::TAB;

SELECT 12::COL;
SELECT ('**What would stabilise the chain in the ' || lower(sc_scenario()) || ' scenario?** Ordering on their own forecasts without smoothing, the stages amplify consumer sales **{{base_ratio:metric}}×** by the time orders reach the supplier. ' ||
        'The calmest option is **' || lower(best.policy) || ', ' || CASE best.smoothing WHEN 'None' THEN 'no smoothing' ELSE 'smoothing over ' || best.smoothing END ||
        '** (**{{calm_ratio:metric}}×**); the cheapest is **' || lower(cheap.policy) || ', ' || CASE cheap.smoothing WHEN 'None' THEN 'no smoothing' ELSE 'smoothing over ' || cheap.smoothing END ||
        '** at {{cheap_cost:money}} against {{base_cost:money}}. Sharing consumer sales removes most of the amplification; smoothing removes the rest, but reacts later to a real change in demand — compare the consumer fill rate.')::MARKDOWN,
       base.bullwhip_top AS base_ratio, best.bullwhip_top AS calm_ratio, cheap.cost AS cheap_cost, base.cost AS base_cost
FROM (SELECT * FROM sc_runs WHERE scenario = sc_scenario() ORDER BY bullwhip_top LIMIT 1) best,
     (SELECT * FROM sc_runs WHERE scenario = sc_scenario() ORDER BY cost LIMIT 1) cheap, sc_base() base;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT smoothing ::XAXIS, policy ::CATEGORY, round(bullwhip_top, 1) ::BARCHART, 1 ::REFLINE, 'no amplification'::REFLINE_LABEL,
       'tween'::TRANSITION,
       'Shared sales data and smoothing calm the supplier''s orders'::TITLE,
       'Bullwhip ratio at the supplier (the top of the chain) for every policy in this scenario.'::INFO
FROM sc_runs WHERE scenario = sc_scenario()
ORDER BY CASE smoothing WHEN 'None' THEN 1 WHEN '2 weeks' THEN 2 ELSE 3 END, policy;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT smoothing ::XAXIS, policy ::CATEGORY, round(cost) ::BARCHART, '$'::YFORMAT,
       'tween'::TRANSITION,
       'Calmer orders also cost less to hold'::TITLE,
       'Holding plus backlog cost of all four stages over 52 weeks, for every policy in this scenario.'::INFO
FROM sc_runs WHERE scenario = sc_scenario()
ORDER BY CASE smoothing WHEN 'None' THEN 1 WHEN '2 weeks' THEN 2 ELSE 3 END, policy;

SELECT 12::COL;
SELECT 'Bullwhip at supplier: >=8:bad; >=3:warn; good | Consumer fill rate: <90:bad; <97:warn; good'::THRESHOLDS;
SELECT 'Chain cost: bottom 1'::HIGHLIGHT;
SELECT 'Cost: Holding, Backlog, Chain cost'::COLGROUP;
-- @name policy_table
SELECT policy || ', ' || CASE smoothing WHEN 'None' THEN 'no smoothing' ELSE 'smoothing ' || smoothing END AS "Policy" ::TABLE,
       round(bullwhip_top, 1) AS "Bullwhip at supplier",
       round(customer_fill, 1) AS "Consumer fill rate" ::PERCENT,
       customer_weeks_short AS "Weeks consumers waited",
       round(cost_holding) AS "Holding" ::MONEY, round(cost_backlog) AS "Backlog" ::MONEY, round(cost) AS "Chain cost" ::MONEY,
       ('Every policy in the ' || lower(sc_scenario()) || ' scenario, calmest first')::TITLE
FROM sc_runs WHERE scenario = sc_scenario()
ORDER BY bullwhip_top;
