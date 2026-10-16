-- Delivery routing — anofox_optimize plans tomorrow's vans for a food wholesaler.
--
-- STORED RESULTS, NOT LIVE. The routes below were computed natively with the
-- anofox_optimize extension (examples/optimize-delivery/compute.sql, written
-- to web/data/optimize-delivery/*.csv by scripts/gen-optimize-cases.sh
-- delivery). Its DuckDB-Wasm build doesn't load yet (see
-- examples/optimize-showcase/README.md), so this dashboard only reads the
-- stored plans: the inputs pick among stored van counts and plans, and price
-- them; nothing is re-solved in the browser.
--
-- The question: which van serves which of tomorrow's 40 stops, in which
-- order, so every customer is served before its deadline at the fewest
-- kilometres? Asked by the dispatcher the afternoon before.
--
-- Tabs: Map · Vans · Stop list · Van count · Decision.
-- Natively, run from web/:  dashboard --check ../examples/optimize-delivery/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_optimize'::BRAND;
SELECT 'Which van serves which stop tomorrow, so every customer gets its delivery on time?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_optimize plans (CSV) ─────────────────────────────
CREATE OR REPLACE TABLE dr_stops AS FROM read_csv('data/optimize-delivery/dr_stops.csv');
CREATE OR REPLACE TABLE dr_summary AS FROM read_csv('data/optimize-delivery/dr_summary.csv');
CREATE OR REPLACE TABLE dr_vans AS FROM read_csv('data/optimize-delivery/dr_vans.csv');
CREATE OR REPLACE TABLE dr_route AS FROM read_csv('data/optimize-delivery/dr_route.csv');
CREATE OR REPLACE TABLE dr_legs AS FROM read_csv('data/optimize-delivery/dr_legs.csv');
CREATE OR REPLACE TABLE dr_provenance AS FROM read_csv('data/optimize-delivery/provenance.csv');

-- The inputs and helpers.
CREATE OR REPLACE MACRO dr_n() AS coalesce(TRY_CAST(getvariable('vans') AS INTEGER), 4);
CREATE OR REPLACE MACRO dr_plan() AS coalesce(getvariable('plan'), 'deadline-aware');
CREATE OR REPLACE MACRO dr_van() AS least(dr_n(), coalesce(TRY_CAST(replace(getvariable('van'), 'Van ', '') AS INTEGER), 1));
CREATE OR REPLACE MACRO dr_today() AS 'today: districts, order list';
CREATE OR REPLACE MACRO dr_cap() AS 32;   -- roll cages per van
CREATE OR REPLACE MACRO dr_eur_km() AS coalesce(TRY_CAST(getvariable('eur_per_km') AS DOUBLE), 0.45);
CREATE OR REPLACE MACRO dr_eur_h() AS coalesce(TRY_CAST(getvariable('eur_per_driver_hour') AS DOUBLE), 32);
CREATE OR REPLACE MACRO dr_eur_van() AS coalesce(TRY_CAST(getvariable('eur_per_van_day') AS DOUBLE), 90);
CREATE OR REPLACE MACRO dr_clock(m) AS strftime(TIMESTAMP '2026-10-20 06:00' + to_minutes(m::BIGINT), '%H:%M');
CREATE OR REPLACE MACRO dr_cost(km, driver_h, vans) AS round(km * dr_eur_km() + driver_h * dr_eur_h() + vans * dr_eur_van());

-- ── Header: the question, who asks it, inputs, KPIs ──────────────────────────
SELECT 12::COL;
SELECT ('**The dispatcher** of a food wholesaler plans tomorrow''s deliveries at 15:00: 40 bakeries, cafés, restaurants, shops and a canteen, each with a deadline (bakeries before they open at 07:30, restaurants before lunch prep) and 1–3 roll cages; a van takes ' || dr_cap() || ' cages and leaves at 06:00. Today each driver has a fixed district and drives the order list. '
        || 'The optimised plans assign stops by a capacity-balanced sweep around the depot and order each van with `opt_sequence_best_of` (shortest route) or `opt_schedule_exact` (least lateness against the deadlines).')::MARKDOWN;

SELECT 'Planning inputs'::GROUP;
SELECT plan ::BUTTONS, 'Plan (vs. today''s districts)'::LABEL FROM (VALUES (1, 'deadline-aware'), (2, 'shortest route')) t(k, plan) ORDER BY k;
SELECT vans ::BUTTONS, 'Vans on the road'::LABEL FROM (VALUES (1, '4'), (2, '5'), (3, '6')) t(k, vans) ORDER BY k;
SELECT ('Van ' || k) AS van ::DROPDOWN, 'Van to drill into'::LABEL FROM (SELECT DISTINCT van AS k FROM dr_vans) ORDER BY k;
SELECT 0.45 AS eur_per_km ::NUMBER, 'Cost per km (€)'::LABEL;
SELECT 32 AS eur_per_driver_hour ::NUMBER, 'Driver cost per hour (€)'::LABEL;
SELECT 90 AS eur_per_van_day ::NUMBER, 'Van cost per day (€)'::LABEL;
SELECT 1::ENDGROUP;

SELECT 'Selected plan vs. today''s districts'::GROUP;
SELECT (40 - (SELECT late_stops FROM dr_summary WHERE plan = dr_plan() AND vans = dr_n()))::METRIC,
       (40 - (SELECT late_stops FROM dr_summary WHERE plan = dr_today() AND vans = dr_n()))::DELTA,
       'abs'::DELTA_FORMAT, 'vs today''s districts'::DELTA_LABEL, '<40:bad; good'::THRESHOLDS,
       'Stops served by their deadline (of 40)'::LABEL;
SELECT ((SELECT km FROM dr_summary WHERE plan = dr_today() AND vans = dr_n()) - (SELECT km FROM dr_summary WHERE plan = dr_plan() AND vans = dr_n()))::METRIC,
       'Kilometres saved'::LABEL;
SELECT ((SELECT dr_cost(km, driver_h, vans) FROM dr_summary WHERE plan = dr_today() AND vans = dr_n())
        - (SELECT dr_cost(km, driver_h, vans) FROM dr_summary WHERE plan = dr_plan() AND vans = dr_n()))::METRIC,
       'Saved per day (€: km, drivers, vans)'::LABEL;
SELECT round((SELECT 100.0 * sum(least(cages, dr_cap())) / sum(cages) FROM dr_vans WHERE plan = dr_plan() AND vans = dr_n()), 1)::PERCENT,
       round((SELECT 100.0 * sum(least(cages, dr_cap())) / sum(cages) FROM dr_vans WHERE plan = dr_today() AND vans = dr_n()), 1)::DELTA,
       'abs'::DELTA_FORMAT, 'vs today''s districts'::DELTA_LABEL, '<100:bad; good'::THRESHOLDS,
       'Cages that fit the first trip'::LABEL,
       'A van takes 32 roll cages; cages beyond that wait at the depot for a second trip.'::INFO;
SELECT 1::ENDGROUP;

-- ── 1 · Map ──────────────────────────────────────────────────────────────────
SELECT 'Map'::TAB;

SELECT 12::COL;
SELECT ('**' || dr_n() || ' vans.** Left: today''s fixed districts driven in order-list sequence — routes criss-cross, and the busiest district carries ' ||
        (SELECT max(cages) FROM dr_vans WHERE plan = dr_today() AND vans = dr_n()) || ' cages for a ' || dr_cap() || '-cage van. Right: the **' || dr_plan() ||
        '** plan — compact wedges around the depot (the dot in the middle), balanced by load. Axes in km; each colour is one van.')::MARKDOWN;

SELECT 6::COL;
SELECT 480::HEIGHT;
SELECT wkt ::MAP, 'Van ' || van ::CATEGORY,
       ('Today: ' || (SELECT round(km) FROM dr_summary WHERE plan = dr_today() AND vans = dr_n()) || ' km, ' ||
        (SELECT late_stops FROM dr_summary WHERE plan = dr_today() AND vans = dr_n()) || ' stops late')::TITLE
FROM (SELECT wkt, van FROM dr_legs WHERE plan = dr_today() AND vans = dr_n()
      UNION ALL SELECT 'POINT(' || x_km || ' ' || y_km || ')', van FROM dr_route WHERE plan = dr_today() AND vans = dr_n() AND stop_id > 0)
ORDER BY van;

SELECT 6::COL;
SELECT 480::HEIGHT;
SELECT wkt ::MAP, 'Van ' || van ::CATEGORY,
       (dr_plan() || ': ' || (SELECT round(km) FROM dr_summary WHERE plan = dr_plan() AND vans = dr_n()) || ' km, ' ||
        (SELECT late_stops FROM dr_summary WHERE plan = dr_plan() AND vans = dr_n()) || ' stops late')::TITLE
FROM (SELECT wkt, van FROM dr_legs WHERE plan = dr_plan() AND vans = dr_n()
      UNION ALL SELECT 'POINT(' || x_km || ' ' || y_km || ')', van FROM dr_route WHERE plan = dr_plan() AND vans = dr_n() AND stop_id > 0)
ORDER BY van;

-- ── 2 · Vans ─────────────────────────────────────────────────────────────────
SELECT 'Vans'::TAB;

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT 'Van ' || van ::XAXIS, CASE WHEN plan = dr_today() THEN 'today' ELSE dr_plan() END ::CATEGORY, cages ::BARCHART,
       dr_cap() ::REFLINE, 'van capacity'::REFLINE_LABEL, '#Stop list?v.van={x}'::LINK,
       'Roll cages per van — today''s districts overload one van; click a van for its stop list'::TITLE
FROM dr_vans WHERE vans = dr_n() AND plan IN (dr_today(), dr_plan()) ORDER BY van, 2 DESC;

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT 'Van ' || van ::XAXIS, CASE WHEN plan = dr_today() THEN 'today' ELSE dr_plan() END ::CATEGORY, round(back_min / 60, 2) ::BARCHART, ' h'::YFORMAT,
       '#Stop list?v.van={x}'::LINK,
       'Hours on the road per van (06:00 → back at the depot); click a van for its stop list'::TITLE
FROM dr_vans WHERE vans = dr_n() AND plan IN (dr_today(), dr_plan()) ORDER BY van, 2 DESC;

SELECT 12::COL;
SELECT 'sum'::TOTALS;
SELECT ('cages > ' || dr_cap() || ', late_stops > 0')::FLAG;
SELECT 'Load: stops, cages | Route: km, back | Deadlines: late_stops, weighted_late_min'::COLGROUP;
SELECT 'plan'::SUBTOTAL;
SELECT 'Van ' || van AS "Van" ::TABLE, CASE WHEN plan = dr_today() THEN 'today' ELSE dr_plan() END AS plan,
       stops, cages, km, dr_clock(back_min) AS back ::PLAIN, late_stops, weighted_late_min AS "Weighted late min",
       ('Van by van, ' || dr_n() || ' vans: today vs. ' || dr_plan())::TITLE
FROM dr_vans WHERE vans = dr_n() AND plan IN (dr_today(), dr_plan()) ORDER BY plan = dr_today() DESC, van;

-- ── 3 · Stop list (one van) ──────────────────────────────────────────────────
SELECT 'Stop list'::TAB;

SELECT 12::COL;
SELECT ('**Van ' || dr_van() || '** (' || dr_plan() || ', ' || dr_n() || ' vans): the driver''s list for tomorrow. Bars show the slack between arrival and deadline — below zero means late. The short bars are the stops that dictate the order; where the slack is large, the van simply takes the shortest way.')::MARKDOWN;

SELECT 12::COL;
SELECT 280::HEIGHT;
SELECT r.pos ::XAXIS, (r.deliver_by_min - r.arrive_min) ::BARCHART, 0 ::REFLINE, ' min'::YFORMAT,
       'Slack at each stop in driving order (x = stop #, y = deadline − arrival in minutes)'::TITLE
FROM dr_route r
WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van() AND r.stop_id > 0 ORDER BY r.pos;

SELECT 12::COL;
SELECT 'late_min > 0'::FLAG;
SELECT 'sum'::TOTALS;
SELECT r.pos AS "#" ::PLAIN, s.name AS "Stop" ::TABLE, s.kind AS "Kind" ::BADGE, s.district AS "District", s.cages AS "Cages",
       r.leg_km AS "Km from previous", dr_clock(r.arrive_min) AS "Arrive" ::PLAIN, dr_clock(r.deliver_by_min) AS "Deadline" ::PLAIN,
       r.late_min,
       ('Van ' || dr_van() || ': stop by stop')::TITLE
FROM dr_route r JOIN dr_stops s USING (stop_id)
WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van() ORDER BY r.pos;

-- ── 4 · Van count ────────────────────────────────────────────────────────────
SELECT 'Van count'::TAB;

SELECT 12::COL;
SELECT '**How many vans do we need?** Every plan was stored for 4, 5 and 6 vans. More vans mean fewer kilometres per van but more kilometres in total (each one drives out and back) and another day rate. Change the cost inputs above to re-price the stored plans.'::MARKDOWN;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT vans || ' vans' ::XAXIS, CASE WHEN plan = dr_today() THEN 'today' ELSE plan END ::CATEGORY, late_stops ::BARCHART,
       'Stops late by van count — routing for deadlines makes 4 vans enough'::TITLE
FROM dr_summary ORDER BY vans, ord;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT vans || ' vans' ::XAXIS, CASE WHEN plan = dr_today() THEN 'today' ELSE plan END ::CATEGORY, dr_cost(km, driver_h, vans) ::BARCHART, '€'::YFORMAT,
       'tween'::TRANSITION, 'Daily cost (km + driver hours + van day rate) — change the cost inputs to re-price'::TITLE
FROM dr_summary ORDER BY vans, ord;

SELECT 12::COL;
SELECT vans || ' vans' ::XAXIS, CASE WHEN plan = dr_today() THEN 'today' ELSE plan END ::YAXIS, round(km) ::HEATMAP, ''::DATALABELS, 220::HEIGHT,
       'Total kilometres by plan and van count — the shortest route is the floor, the deadline-aware plan pays a few km for punctuality'::TITLE
FROM dr_summary ORDER BY ord, vans;

-- ── 5 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

-- The cheapest stored plan that serves every stop on time.
CREATE OR REPLACE MACRO dr_best() AS TABLE
    SELECT * FROM dr_summary WHERE late_stops = 0 ORDER BY dr_cost(km, driver_h, vans), km LIMIT 1;

SELECT 12::COL;
SELECT ('### Recommendation' || chr(10) || chr(10) ||
        '**Send ' || b.vans || ' vans on the ' || b.plan || ' plan.** It is the cheapest stored plan that serves all 40 stops by their deadline: **' ||
        round(b.km)::INTEGER || ' km** and **' || b.driver_h || ' driver hours** for **€' || dr_cost(b.km, b.driver_h, b.vans)::INTEGER || '** a day, against **' ||
        round(t.km)::INTEGER || ' km**, **' || t.late_stops || ' late stops** and **€' || dr_cost(t.km, t.driver_h, t.vans)::INTEGER || '** with today''s districts. The last van is back at **' || dr_clock(b.last_back_min) || '**.' ||
        chr(10) || chr(10) ||
        'The pure shortest route with ' || b.vans || ' vans is ' || round(b.km - s.km)::INTEGER || ' km shorter but leaves **' || s.late_stops ||
        '** stops late — the extra kilometres buy the bakeries'' 07:30 deadlines. At your cost inputs (€' || dr_eur_km() || '/km, €' || dr_eur_h() || '/h, €' || dr_eur_van() || '/van) a fifth van ' ||
        CASE WHEN (SELECT dr_cost(km, driver_h, vans) FROM dr_summary WHERE plan = b.plan AND vans = b.vans + 1) > dr_cost(b.km, b.driver_h, b.vans)
             THEN 'would only add cost.' ELSE 'would pay for itself.' END)::MARKDOWN
FROM dr_best() b
JOIN dr_summary t ON t.plan = dr_today() AND t.vans = b.vans
JOIN dr_summary s ON s.plan = 'shortest route' AND s.vans = b.vans;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT step ::XAXIS, eur ::WATERFALL, '€'::YFORMAT, 'tween'::TRANSITION,
       ('Where the recommended plan saves, per day, against today''s districts with ' || dr_n() || ' vans')::TITLE
FROM (SELECT 1 AS i, 'Fewer kilometres' AS step, round((t.km - b.km) * dr_eur_km()) AS eur FROM dr_best() b, dr_summary t WHERE t.plan = dr_today() AND t.vans = dr_n()
      UNION ALL SELECT 2, 'Fewer driver hours', round((t.driver_h - b.driver_h) * dr_eur_h()) FROM dr_best() b, dr_summary t WHERE t.plan = dr_today() AND t.vans = dr_n()
      UNION ALL SELECT 3, 'Van day rates', round((t.vans - b.vans) * dr_eur_van()) FROM dr_best() b, dr_summary t WHERE t.plan = dr_today() AND t.vans = dr_n()
      UNION ALL SELECT 4, 'Saved per day', NULL)
ORDER BY i;

SELECT 12::COL;
SELECT 'late_stops > 0'::FLAG;
SELECT 'none'::TOTALS;
SELECT vans || ' vans' AS "Vans" ::TABLE, CASE WHEN plan = dr_today() THEN 'today' ELSE plan END AS plan,
       km, late_stops, weighted_late_min AS "Weighted late min", driver_h AS "Driver hours",
       dr_clock(last_back_min) AS "Last van back" ::PLAIN, max_cages AS "Fullest van (cages)",
       dr_cost(km, driver_h, vans) AS "Daily cost (€)",
       CASE WHEN plan = (SELECT plan FROM dr_best()) AND vans = (SELECT vans FROM dr_best()) THEN 'ok: recommended'
            WHEN late_stops > 0 THEN 'misses deadlines' ELSE 'on time, dearer' END AS "Verdict" ::BADGE,
       'Every stored plan, priced at the cost inputs'::TITLE
FROM dr_summary ORDER BY ord, vans;
