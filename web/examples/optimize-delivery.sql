-- @title Which van serves which stop tomorrow?
-- @group Logistics & production
-- @tags  optimize, routing
-- @description Tomorrow's van routes for a food wholesaler: stops on time, kilometres and cost saved.
--
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
-- The town is a street grid (a street every km each way, stops on crossings):
-- the solvers got street distances (|dx| + |dy| km at 40 km/h), every km on
-- the page is a street km and the routes are drawn along the streets.
--
-- Tabs: Map · Vans · Stop list · Decision. The plan and van-count inputs sit
-- above the tabs (the KPI row uses them); the van for the stop list sits in
-- the Stop list tab and the cost assumptions in the Decision tab. "Today's plan" is the
-- stored plan 'today: districts, order list'.
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


-- The inputs and helpers. "Today's plan" is how the vans drive now: each
-- driver a fixed district, the stops in order-list sequence.
CREATE OR REPLACE MACRO dr_n() AS coalesce(TRY_CAST(getvariable('vans') AS INTEGER), 4);
CREATE OR REPLACE MACRO dr_plan() AS coalesce(getvariable('plan'), 'deadline-aware');
CREATE OR REPLACE MACRO dr_van() AS least(dr_n(), coalesce(TRY_CAST(replace(getvariable('van'), 'Van ', '') AS INTEGER), 1));
CREATE OR REPLACE MACRO dr_today() AS 'today: districts, order list';
CREATE OR REPLACE MACRO dr_name(p) AS CASE WHEN p = dr_today() THEN 'today''s plan' ELSE p || ' plan' END;
CREATE OR REPLACE MACRO dr_title(p) AS upper(dr_name(p)[1]) || dr_name(p)[2:];
CREATE OR REPLACE MACRO dr_cap() AS 32;   -- roll cages per van
CREATE OR REPLACE MACRO dr_eur_km() AS coalesce(TRY_CAST(getvariable('eur_per_km') AS DOUBLE), 0.45);
CREATE OR REPLACE MACRO dr_eur_h() AS coalesce(TRY_CAST(getvariable('eur_per_driver_hour') AS DOUBLE), 32);
CREATE OR REPLACE MACRO dr_eur_van() AS coalesce(TRY_CAST(getvariable('eur_per_van_day') AS DOUBLE), 90);
CREATE OR REPLACE MACRO dr_clock(m) AS strftime(TIMESTAMP '2026-10-20 06:00' + to_minutes(m::BIGINT), '%H:%M');
CREATE OR REPLACE MACRO dr_eur_late() AS coalesce(TRY_CAST(getvariable('eur_per_late_stop') AS DOUBLE), 25);
-- The cost of a day: every street km, every driver hour (06:00 until the van
-- is back), a day rate per van on the road, a penalty per late delivery.
CREATE OR REPLACE MACRO dr_cost(km, driver_h, vans, late) AS
    round(km * dr_eur_km() + driver_h * dr_eur_h() + vans * dr_eur_van() + late * dr_eur_late());
CREATE OR REPLACE MACRO dr_late(n) AS CASE n WHEN 0 THEN 'all on time' WHEN 1 THEN '1 stop late' ELSE n || ' stops late' END;

-- One row per plan and van count: the selected plan beside today's plan.
CREATE OR REPLACE MACRO dr_cmp() AS TABLE
    SELECT t.km AS t_km, p.km AS p_km, t.late_stops AS t_late, p.late_stops AS p_late,
           t.driver_h AS t_h, p.driver_h AS p_h, t.last_back_min AS t_back, p.last_back_min AS p_back,
           dr_cost(t.km, t.driver_h, t.vans, t.late_stops) AS t_eur, dr_cost(p.km, p.driver_h, p.vans, p.late_stops) AS p_eur
    FROM dr_summary t JOIN dr_summary p ON p.plan = dr_plan() AND p.vans = t.vans
    WHERE t.plan = dr_today() AND t.vans = dr_n();

-- ── Header: the question, who asks it, inputs, KPIs ──────────────────────────
SELECT 12::COL;
SELECT ('**The dispatcher** of a food wholesaler plans tomorrow''s deliveries at 15:00: 40 bakeries, cafés, restaurants, shops and a canteen, each with a deadline (bakeries before they open at 07:30, restaurants before lunch prep) and 1–3 roll cages; a van takes ' || dr_cap() || ' cages, leaves the depot at 06:00 and drives the town''s street grid at 40 km/h (all distances are street km). '
        || '**Today''s plan:** each driver keeps a fixed district and drives the stops in order-list sequence. '
        || '**The new plans** split the stops into wedges around the depot, balanced by load, and order each van''s stops with `opt_schedule_exact` (deadline-aware: no customer late) or `opt_sequence_best_of` (shortest route).')::MARKDOWN;

SELECT 'Plan'::GROUP;
SELECT plan ::BUTTONS, 'New plan to compare with today''s'::LABEL FROM (VALUES (1, 'deadline-aware'), (2, 'shortest route')) t(k, plan) ORDER BY k;
SELECT vans ::BUTTONS, 'Vans on the road'::LABEL FROM (VALUES (1, '4'), (2, '5'), (3, '6')) t(k, vans) ORDER BY k;
SELECT 1::ENDGROUP;

SELECT ('Tomorrow with the ' || dr_name(dr_plan()) || ' vs today''s plan, ' || dr_n() || ' vans')::GROUP;
SELECT (40 - p_late)::METRIC, (40 - t_late)::DELTA, 'abs'::DELTA_FORMAT,
       ('vs ' || (40 - t_late) || ' with today''s plan')::DELTA_LABEL, '<40:bad; good'::THRESHOLDS,
       'Stops on time (of 40)'::LABEL,
       'On time: the van arrives before the customer''s deadline (bakeries 07:30, restaurants before lunch prep). Today''s plan: each driver''s fixed district, stops in order-list sequence.'::INFO
FROM dr_cmp();
SELECT round(p_km)::METRIC, round(t_km)::DELTA_LOWER_GOOD, 'abs'::DELTA_FORMAT,
       ('vs ' || round(t_km)::INTEGER || ' km with today''s plan')::DELTA_LABEL,
       'Kilometres driven (all vans)'::LABEL,
       'Street kilometres of all vans together, depot to depot. The town is a street grid (a street every km each way); a van drives along the streets, so a leg is its east-west plus its north-south distance.'::INFO
FROM dr_cmp();
SELECT p_h::METRIC, t_h::DELTA_LOWER_GOOD, 'abs'::DELTA_FORMAT,
       ('vs ' || t_h || ' h with today''s plan; last van back ' || dr_clock(p_back) || ' instead of ' || dr_clock(t_back))::DELTA_LABEL,
       'Driver hours (all vans)'::LABEL,
       'Hours from leaving the depot at 06:00 until the van is back, summed over the vans: driving plus unloading.'::INFO
FROM dr_cmp();
SELECT p_eur::METRIC, t_eur::DELTA_LOWER_GOOD, 'abs'::DELTA_FORMAT,
       ('vs €' || t_eur::INTEGER || ' with today''s plan')::DELTA_LABEL,
       'Cost per day (€)'::LABEL,
       ('Street km × €' || dr_eur_km() || ' + driver hours × €' || dr_eur_h() || ' + vans × €' || dr_eur_van() || ' day rate + late deliveries × €' || dr_eur_late() || ' — the cost assumptions on the Decision tab.')::INFO
FROM dr_cmp();
SELECT 1::ENDGROUP;

-- ── 1 · Map ──────────────────────────────────────────────────────────────────
SELECT 'Map'::TAB;

-- One plan's map layers, drawn in this order: each van's route, an arrow at
-- the middle of every leg (pointing where the van drives next), a red ring
-- around every late stop, the stops (van-coloured dots) and the depot. The
-- colours: ::COLORS pins 'Depot' and 'Late stop'; the vans take the palette
-- in order, so a van has the same colour on both maps. Each feature's ::LABEL
-- is its hover text.
CREATE OR REPLACE MACRO dr_quad(x1, y1, x2, y2, x3, y3, x4, y4) AS
    printf('((%.2f %.2f, %.2f %.2f, %.2f %.2f, %.2f %.2f, %.2f %.2f))', x1, y1, x2, y2, x3, y3, x4, y4, x1, y1);
CREATE OR REPLACE MACRO dr_ring(x, y) AS
    'MULTIPOLYGON(' || dr_quad(x, y + 1.1, x + 1.1, y, x + 0.6, y, x, y + 0.6) || ', ' ||
                       dr_quad(x + 1.1, y, x, y - 1.1, x, y - 0.6, x + 0.6, y) || ', ' ||
                       dr_quad(x, y - 1.1, x - 1.1, y, x - 0.6, y, x, y - 0.6) || ', ' ||
                       dr_quad(x - 1.1, y, x, y + 1.1, x, y + 0.6, x - 0.6, y) || ')';
-- An arrowhead 2·a km long, its middle at (mx, my), pointing along the unit
-- vector (ux, uy); dr_arrow puts one at the middle of the leg (x1, y1) → (x2, y2).
CREATE OR REPLACE MACRO dr_arrow_at(mx, my, ux, uy, a) AS
    printf('POLYGON((%.2f %.2f, %.2f %.2f, %.2f %.2f, %.2f %.2f))',
           mx + a * ux, my + a * uy,
           mx - a * ux - 0.75 * a * uy, my - a * uy + 0.75 * a * ux,
           mx - a * ux + 0.75 * a * uy, my - a * uy - 0.75 * a * ux,
           mx + a * ux, my + a * uy);
-- A leg runs along the streets: east-west from (x1, y1) to the corner
-- (x2, y1), then north-south to (x2, y2). Its arrow sits in the middle of the
-- longer stretch.
CREATE OR REPLACE MACRO dr_arrow(x1, y1, x2, y2, a) AS
    CASE WHEN abs(x2 - x1) >= abs(y2 - y1) THEN dr_arrow_at((x1 + x2) / 2, y1, sign(x2 - x1), 0, a)
         ELSE dr_arrow_at(x2, (y1 + y2) / 2, 0, sign(y2 - y1), a) END;
CREATE OR REPLACE MACRO dr_leg_wkt(x1, y1, x2, y2) AS
    'LINESTRING(' || x1 || ' ' || y1 || ', ' || x2 || ' ' || y1 || ', ' || x2 || ' ' || y2 || ')';
-- The town's streets as a backdrop: one every km each way, over the box
-- x0..x1, y0..y1 (whole km).
CREATE OR REPLACE MACRO dr_streets(x0, x1, y0, y1) AS TABLE
    SELECT printf('POLYGON((%.2f %d, %.2f %d, %.2f %d, %.2f %d, %.2f %d))', k - 0.07, y0, k + 0.07, y0, k + 0.07, y1, k - 0.07, y1, k - 0.07, y0) AS base
    FROM range(x0, x1 + 1) t(k)
    UNION ALL
    SELECT printf('POLYGON((%d %.2f, %d %.2f, %d %.2f, %d %.2f, %d %.2f))', x0, k - 0.07, x1, k - 0.07, x1, k + 0.07, x0, k + 0.07, x0, k - 0.07)
    FROM range(y0, y1 + 1) t(k);
CREATE OR REPLACE MACRO dr_stop_label(name, kind, van, pos, n, arrive_min, deliver_by_min, late_min, cages) AS
    name || ' — ' || kind || chr(10) || 'Van ' || van || ', stop ' || pos || ' of ' || n || chr(10) ||
    'arrives ' || dr_clock(arrive_min) || ', deadline ' || dr_clock(deliver_by_min) || chr(10) ||
    CASE WHEN late_min > 0 THEN round(late_min)::INTEGER || ' min LATE'
         ELSE round(deliver_by_min - arrive_min)::INTEGER || ' min to spare' END || chr(10) ||
    cages || CASE WHEN cages = 1 THEN ' roll cage' ELSE ' roll cages' END;
-- Every leg of a plan: from the previous stop (or the depot) to this one.
CREATE OR REPLACE MACRO dr_legs_of(p) AS TABLE
    SELECT b.van, b.pos, a.x_km AS ax, a.y_km AS ay, b.x_km AS bx, b.y_km AS by_, b.leg_km, b.arrive_min,
           coalesce(s.name, 'the depot') AS to_name, b.stop_id,
           count(*) FILTER (b.stop_id > 0) OVER (PARTITION BY b.van) AS n
    FROM dr_route b JOIN dr_route a ON a.plan = b.plan AND a.vans = b.vans AND a.van = b.van AND a.pos = b.pos - 1
    LEFT JOIN dr_stops s ON s.stop_id = b.stop_id
    WHERE b.plan = p AND b.vans = dr_n();
CREATE OR REPLACE MACRO dr_map(p) AS TABLE
    SELECT 0 AS layer, 0 AS van, NULL AS wkt, NULL AS series, NULL AS label, base FROM dr_streets(-19, 19, -19, 19)
    UNION ALL
    SELECT 1 AS layer, l.van, l.wkt, 'Van ' || l.van AS series,
           'Van ' || l.van || ' — ' || dr_name(p) || chr(10) || v.stops || ' stops, ' || round(v.km)::INTEGER || ' km' || chr(10) ||
           'leaves 06:00, back at the depot ' || dr_clock(v.back_min) || chr(10) || dr_late(v.late_stops) AS label, NULL
    FROM dr_legs l JOIN dr_vans v USING (plan, vans, van)
    WHERE l.plan = p AND l.vans = dr_n()
    UNION ALL
    SELECT 2, van, dr_arrow(ax, ay, bx, by_, 0.6), 'Van ' || van,
           'Van ' || van || ' → ' || CASE WHEN stop_id = 0 THEN 'back to the depot' ELSE 'stop ' || pos || ' of ' || n || ': ' || to_name END ||
           chr(10) || round(leg_km, 1) || ' street km, arrives ' || dr_clock(arrive_min), NULL
    FROM dr_legs_of(p) WHERE greatest(abs(bx - ax), abs(by_ - ay)) > 1.5
    UNION ALL
    SELECT 3, r.van, dr_ring(r.x_km, r.y_km), 'Late stop',
           dr_stop_label(s.name, s.kind, r.van, r.pos, v.stops, r.arrive_min, r.deliver_by_min, r.late_min, s.cages), NULL
    FROM dr_route r JOIN dr_stops s USING (stop_id) JOIN dr_vans v USING (plan, vans, van)
    WHERE r.plan = p AND r.vans = dr_n() AND r.stop_id > 0 AND r.late_min > 0
    UNION ALL
    SELECT 4, r.van, 'POINT(' || r.x_km || ' ' || r.y_km || ')', 'Van ' || r.van,
           dr_stop_label(s.name, s.kind, r.van, r.pos, v.stops, r.arrive_min, r.deliver_by_min, r.late_min, s.cages), NULL
    FROM dr_route r JOIN dr_stops s USING (stop_id) JOIN dr_vans v USING (plan, vans, van)
    WHERE r.plan = p AND r.vans = dr_n() AND r.stop_id > 0
    UNION ALL
    SELECT 5, 0, 'POLYGON((-0.8 -0.8, 0.8 -0.8, 0.8 0.8, -0.8 0.8, -0.8 -0.8))', 'Depot',
           'Depot' || chr(10) || 'every van leaves at 06:00 and ends here', NULL;

SELECT 12::COL;
SELECT ('**' || CASE WHEN p_late = 0 THEN 'All 40 stops on time' ELSE (40 - p_late) || ' of 40 stops on time' END ||
        ' with the ' || dr_name(dr_plan()) || ' (today''s plan: ' || t_late || ' late), ' ||
        round(p_km)::INTEGER || ' km instead of ' || round(t_km)::INTEGER || ' km (' ||
        CASE WHEN p_km <= t_km THEN round(t_km - p_km)::INTEGER || ' km less' ELSE round(p_km - t_km)::INTEGER || ' km more' END ||
        '), last van back at ' || dr_clock(p_back) || ' instead of ' || dr_clock(t_back) || '.** ' ||
        'Left today''s plan, right the new one, on the town''s street grid (light lines, a street every km): each colour is one van driving along the streets, the arrows point the way it goes, the grey square is the depot where every route starts and ends, a red ring marks a stop reached after its deadline. All distances are street km. ' ||
        '**Hover** a route, arrow or stop: that van lights up on both maps, with its stop order and times. **Click** a van to pick it out in the table below; click empty space to clear. ' ||
        '**Scroll or drag** to zoom — the two maps move together; double-click resets. The **Stop list** tab plays one van''s route stop by stop.')::MARKDOWN
FROM dr_cmp();

SELECT 6::COL;
SELECT 480::HEIGHT;
SELECT wkt ::MAP, series ::CATEGORY, label ::LABEL, base ::BASEMAP, 'routes'::SYNC_ZOOM, 'Depot:neutral; Late stop:bad'::COLORS,
       ('Today''s plan: ' || round(t_km)::INTEGER || ' km, ' || CASE WHEN t_late = 0 THEN 'all 40 stops on time' ELSE t_late || ' of 40 stops late' END)::TITLE
FROM dr_map(dr_today()), dr_cmp()
ORDER BY layer, van;

SELECT 6::COL;
SELECT 480::HEIGHT;
SELECT wkt ::MAP, series ::CATEGORY, label ::LABEL, base ::BASEMAP, 'routes'::SYNC_ZOOM, 'Depot:neutral; Late stop:bad'::COLORS,
       (dr_title(dr_plan()) || ': ' || round(p_km)::INTEGER || ' km, ' || CASE WHEN p_late = 0 THEN 'all 40 stops on time' ELSE p_late || ' of 40 stops late' END)::TITLE
FROM dr_map(dr_plan()), dr_cmp()
ORDER BY layer, van;

-- Van by van, today's plan vs. the new plan; the clicked van (or, for a
-- click on a ring, every van with a late stop today) alone. A click
-- elsewhere (another tab's chart) leaves every van in.
CREATE OR REPLACE MACRO dr_pick() AS
    CASE WHEN coalesce(getvariable('selected'), '') LIKE 'Van %' OR getvariable('selected') = 'Late stop'
         THEN getvariable('selected') ELSE '' END;
CREATE OR REPLACE MACRO dr_day(p) AS TABLE
    SELECT van, min(arrive_min) FILTER (stop_id > 0) AS first_min, max(arrive_min) FILTER (stop_id > 0) AS last_min,
           sum(late_min) AS late_min
    FROM dr_route WHERE plan = p AND vans = dr_n() GROUP BY van;
SELECT 12::COL;
SELECT 'sum'::TOTALS;
SELECT 'Km saved: <0:bad; >0:good | Hours saved: <0:bad; >0:good | Late today: >0:bad; good | Late plan: >0:bad; good'::THRESHOLDS;
SELECT 'Kilometres: Km today, Km plan, Km saved | Hours on the road: Hours today, Hours plan, Hours saved | Back at the depot: Back today, Back plan | Stops late: Late today, Late plan | New plan''s day: First stop, Last stop'::COLGROUP;
SELECT 'Van ' || t.van AS "Van" ::TABLE,
       t.km AS "Km today", p.km AS "Km plan", round(t.km - p.km, 1) AS "Km saved",
       round(t.back_min / 60, 1) AS "Hours today", round(p.back_min / 60, 1) AS "Hours plan", round((t.back_min - p.back_min) / 60, 1) AS "Hours saved",
       dr_clock(t.back_min) AS "Back today" ::PLAIN, dr_clock(p.back_min) AS "Back plan" ::PLAIN,
       t.late_stops AS "Late today", p.late_stops AS "Late plan",
       dr_clock(d.first_min) AS "First stop" ::PLAIN, dr_clock(d.last_min) AS "Last stop" ::PLAIN,
       '#Stop list?v.van=Van ' || t.van AS "Stop list" ::LINK,
       CASE dr_pick() WHEN '' THEN 'Van by van: today''s plan vs the ' || dr_name(dr_plan()) || ' — click a van on a map to pick it out'
                      WHEN 'Late stop' THEN 'The vans with a late stop today: today''s plan vs the ' || dr_name(dr_plan())
                      ELSE dr_pick() || ': today''s plan vs the ' || dr_name(dr_plan()) END ::TITLE
FROM dr_vans t JOIN dr_vans p ON p.plan = dr_plan() AND p.vans = t.vans AND p.van = t.van
JOIN dr_day(dr_plan()) d ON d.van = t.van
WHERE t.plan = dr_today() AND t.vans = dr_n()
  AND (dr_pick() IN ('', 'Van ' || t.van) OR (dr_pick() = 'Late stop' AND t.late_stops > 0))
ORDER BY t.van;

-- ── 2 · Vans ─────────────────────────────────────────────────────────────────
SELECT 'Vans'::TAB;

SELECT 12::COL;
SELECT ('**' || CASE WHEN t.max_cages > dr_cap() THEN 'Today''s plan overloads one van (' || t.max_cages || ' roll cages for ' || dr_cap() || ' places — a second trip) and keeps the busiest van out until '
                     ELSE 'Today''s plan loads the busiest van with ' || t.max_cages || ' roll cages and keeps one van out until ' END ||
        dr_clock(t.last_back_min) || '; the ' || dr_name(dr_plan()) || ' loads every van with at most ' || p.max_cages || ' cages and brings the last one back at ' || dr_clock(p.last_back_min) ||
        ' — ' || round(t.driver_h - p.driver_h, 1) || ' driver hours less.** Click a van''s bar for its stop list.')::MARKDOWN
FROM dr_summary t JOIN dr_summary p ON p.plan = dr_plan() AND p.vans = t.vans
WHERE t.plan = dr_today() AND t.vans = dr_n();

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT 'Van ' || van ::XAXIS, dr_name(plan) ::CATEGORY, 'today''s plan:neutral'::COLORS, cages ::BARCHART,
       dr_cap() ::REFLINE, 'van capacity (32 cages)'::REFLINE_LABEL, '#Stop list?v.van={x}'::LINK,
       'Roll cages per van: today''s plan vs the new plan'::TITLE
FROM dr_vans WHERE vans = dr_n() AND plan IN (dr_today(), dr_plan()) ORDER BY van, plan = dr_today() DESC;

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT 'Van ' || van ::XAXIS, dr_name(plan) ::CATEGORY, 'today''s plan:neutral'::COLORS, round(back_min / 60, 2) ::BARCHART, ' h'::YFORMAT,
       '#Stop list?v.van={x}'::LINK,
       'Hours on the road per van (06:00 until back at the depot): today''s plan vs the new plan'::TITLE
FROM dr_vans WHERE vans = dr_n() AND plan IN (dr_today(), dr_plan()) ORDER BY van, plan = dr_today() DESC;

SELECT 12::COL;
SELECT 'none'::TOTALS;
SELECT ('Cages: >' || dr_cap() || ':bad | Stops late: >0:bad; good | Minutes late: >0:bad; good')::THRESHOLDS;
SELECT 'Load: Stops, Cages | Route: Km, Back at the depot | Deadlines: Stops late, Minutes late'::COLGROUP;
SELECT 'Plan'::SUBTOTAL;
SELECT 'Van ' || v.van AS "Van" ::TABLE, dr_title(v.plan) AS "Plan",
       v.stops AS "Stops", v.cages AS "Cages", v.km AS "Km", dr_clock(v.back_min) AS "Back at the depot" ::PLAIN,
       v.late_stops AS "Stops late", round(d.late_min)::INTEGER AS "Minutes late",
       ('Van by van, ' || dr_n() || ' vans: today''s plan vs the ' || dr_name(dr_plan()))::TITLE
FROM dr_vans v
JOIN (SELECT plan, van, sum(late_min) AS late_min FROM dr_route WHERE vans = dr_n() GROUP BY ALL) d USING (plan, van)
WHERE v.vans = dr_n() AND v.plan IN (dr_today(), dr_plan()) ORDER BY v.plan = dr_today() DESC, v.van;

-- ── 3 · Stop list (one van) ──────────────────────────────────────────────────
SELECT 'Stop list'::TAB;

-- The van's route as a story: step k lights the leg to stop k, its arrow
-- and the stop (the ::XAXIS keys the features), the caption says when the
-- van gets there.
CREATE OR REPLACE MACRO dr_story_caption(pos, n, stop_id, name, kind, arrive_min, deliver_by_min, late_min, leg_km, total_km) AS
    dr_clock(arrive_min) || ' · ' ||
    CASE WHEN stop_id = 0 THEN 'back at the depot after ' || round(total_km)::INTEGER || ' km'
         ELSE 'stop ' || pos || ' of ' || n || ': ' || name || ' (' || kind || '), ' || round(leg_km, 1) || ' street km from the last stop — deadline ' ||
              dr_clock(deliver_by_min) || ', ' ||
              CASE WHEN late_min > 0 THEN round(late_min)::INTEGER || ' min LATE' ELSE round(deliver_by_min - arrive_min)::INTEGER || ' min to spare' END END;

SELECT 'Driver''s list'::GROUP;
SELECT ('Van ' || k) AS van ::DROPDOWN, 'Van'::LABEL FROM (SELECT DISTINCT van AS k FROM dr_vans WHERE vans = dr_n()) ORDER BY k;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT ('**Van ' || dr_van() || '**, ' || dr_name(dr_plan()) || ' with ' || dr_n() || ' vans: **' || v.stops || ' stops, ' || round(v.km)::INTEGER || ' km, back at the depot ' || dr_clock(v.back_min) || ', ' || dr_late(v.late_stops) ||
        '** (today''s plan for Van ' || dr_van() || ': ' || t.stops || ' stops, ' || round(t.km)::INTEGER || ' km, back ' || dr_clock(t.back_min) || ', ' || dr_late(t.late_stops) || '). ' ||
        'Press **▶ Play** on the map to follow the van stop by stop; the bars below show the minutes to spare at each stop — below zero is late.')::MARKDOWN
FROM dr_vans v JOIN dr_vans t ON t.plan = dr_today() AND t.vans = v.vans AND t.van = v.van
WHERE v.plan = dr_plan() AND v.vans = dr_n() AND v.van = dr_van();

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT wkt ::MAP, series ::CATEGORY, label ::LABEL, base ::BASEMAP, x ::XAXIS, caption ::STORY, 'Depot:neutral; Late stop:bad'::COLORS,
       ('Van ' || dr_van() || ', ' || dr_name(dr_plan()) || ': its route stop by stop')::TITLE
FROM (
    SELECT 0 AS layer, NULL AS x, NULL AS wkt, NULL AS series, NULL AS label, NULL AS caption, base
    FROM (SELECT min(x_km)::INTEGER - 2 AS x0, max(x_km)::INTEGER + 2 AS x1, min(y_km)::INTEGER - 2 AS y0, max(y_km)::INTEGER + 2 AS y1
          FROM dr_route WHERE plan = dr_plan() AND vans = dr_n() AND van = dr_van()) b,
         LATERAL (FROM dr_streets(b.x0, b.x1, b.y0, b.y1))
    UNION ALL
    SELECT 1, l.pos,
           dr_leg_wkt(l.ax, l.ay, l.bx, l.by_), 'Van ' || l.van,
           'Van ' || l.van || ' → ' || CASE WHEN l.stop_id = 0 THEN 'back to the depot' ELSE 'stop ' || l.pos || ' of ' || l.n || ': ' || l.to_name END ||
           chr(10) || round(l.leg_km, 1) || ' street km, arrives ' || dr_clock(l.arrive_min),
           dr_story_caption(l.pos, l.n, l.stop_id, l.to_name, s.kind, l.arrive_min, r.deliver_by_min, r.late_min, l.leg_km, v.km), NULL
    FROM dr_legs_of(dr_plan()) l JOIN dr_route r ON r.plan = dr_plan() AND r.vans = dr_n() AND r.van = l.van AND r.pos = l.pos
    LEFT JOIN dr_stops s ON s.stop_id = l.stop_id JOIN dr_vans v ON v.plan = dr_plan() AND v.vans = dr_n() AND v.van = l.van
    WHERE l.van = dr_van()
    UNION ALL
    SELECT 2, pos, dr_arrow(ax, ay, bx, by_, 0.35), 'Van ' || van,
           'Van ' || van || ' → ' || CASE WHEN stop_id = 0 THEN 'back to the depot' ELSE 'stop ' || pos || ' of ' || n || ': ' || to_name END, NULL, NULL
    FROM dr_legs_of(dr_plan()) WHERE van = dr_van() AND greatest(abs(bx - ax), abs(by_ - ay)) > 1.5
    UNION ALL
    SELECT 3, r.pos, dr_ring(r.x_km, r.y_km), 'Late stop',
           dr_stop_label(s.name, s.kind, r.van, r.pos, v.stops, r.arrive_min, r.deliver_by_min, r.late_min, s.cages), NULL, NULL
    FROM dr_route r JOIN dr_stops s USING (stop_id) JOIN dr_vans v USING (plan, vans, van)
    WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van() AND r.late_min > 0
    UNION ALL
    SELECT 4, r.pos, 'POINT(' || r.x_km || ' ' || r.y_km || ')', 'Van ' || r.van,
           dr_stop_label(s.name, s.kind, r.van, r.pos, v.stops, r.arrive_min, r.deliver_by_min, r.late_min, s.cages), NULL, NULL
    FROM dr_route r JOIN dr_stops s USING (stop_id) JOIN dr_vans v USING (plan, vans, van)
    WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van()
    UNION ALL
    SELECT 5, 0, 'POLYGON((-0.8 -0.8, 0.8 -0.8, 0.8 0.8, -0.8 0.8, -0.8 -0.8))', 'Depot',
           'Depot' || chr(10) || 'the van leaves at 06:00 and ends here', NULL, NULL
)
ORDER BY layer, x NULLS FIRST;

SELECT 12::COL;
SELECT 260::HEIGHT;
SELECT r.pos ::XAXIS, (r.deliver_by_min - r.arrive_min) ::BARCHART, 0 ::REFLINE, ' min'::YFORMAT,
       'Minutes to spare at each stop (x: stop number in driving order; below zero = late)'::TITLE
FROM dr_route r
WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van() AND r.stop_id > 0 ORDER BY r.pos;

SELECT 12::COL;
SELECT 'Minutes late: >0:bad'::THRESHOLDS;
SELECT 'Cages: sum, Street km from previous: sum, Minutes to spare: min, Minutes late: sum'::TOTALS;
SELECT r.pos AS "#" ::PLAIN, coalesce(s.name, 'Depot') AS "Stop" ::TABLE, s.kind AS "Kind" ::BADGE, s.district AS "District", s.cages AS "Cages",
       r.leg_km AS "Street km from previous", dr_clock(r.arrive_min) AS "Arrive" ::PLAIN, dr_clock(r.deliver_by_min) AS "Deadline" ::PLAIN,
       round(r.deliver_by_min - r.arrive_min)::INTEGER AS "Minutes to spare", r.late_min AS "Minutes late",
       ('Van ' || dr_van() || ': the driver''s list for tomorrow')::TITLE
FROM dr_route r LEFT JOIN dr_stops s USING (stop_id)
WHERE r.plan = dr_plan() AND r.vans = dr_n() AND r.van = dr_van() ORDER BY r.pos;

-- ── 4 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

-- The solvers don't optimise cost: opt_sequence_best_of minimises driving
-- minutes, opt_schedule_* minimises weighted lateness, and the number of vans
-- is the dispatcher's choice. Every stored plan is priced here with the cost
-- assumptions below; the recommendation is the cheapest one on time.
SELECT 'Cost assumptions'::GROUP;
SELECT 0.45 AS eur_per_km ::NUMBER, 'Cost per street km (€)'::LABEL;
SELECT 32 AS eur_per_driver_hour ::NUMBER, 'Driver cost per hour (€)'::LABEL;
SELECT 90 AS eur_per_van_day ::NUMBER, 'Day rate per van (€)'::LABEL;
SELECT 25 AS eur_per_late_stop ::NUMBER, 'Penalty per late delivery (€)'::LABEL;
SELECT 1::ENDGROUP;

-- Every plan's daily cost, by component.
CREATE OR REPLACE MACRO dr_parts() AS TABLE
    SELECT plan, vans, ord, km, driver_h, late_stops, last_back_min, max_cages,
           round(km * dr_eur_km()) AS eur_km, round(driver_h * dr_eur_h()) AS eur_h,
           round(vans * dr_eur_van()) AS eur_van, round(late_stops * dr_eur_late()) AS eur_late,
           dr_cost(km, driver_h, vans, late_stops) AS eur
    FROM dr_summary;
-- The cheapest stored plan that serves every stop on time, and today's plan
-- with the same number of vans.
CREATE OR REPLACE MACRO dr_best() AS TABLE
    SELECT * FROM dr_parts() WHERE late_stops = 0 ORDER BY eur, km LIMIT 1;
CREATE OR REPLACE MACRO dr_best_today() AS TABLE
    SELECT t.* FROM dr_parts() t, dr_best() b WHERE t.plan = dr_today() AND t.vans = b.vans;

SELECT 12::COL;
SELECT ('### Recommendation' || chr(10) || chr(10) ||
        '**Send ' || b.vans || ' vans on the ' || b.plan || ' plan.** Against today''s plan with ' || b.vans || ' vans it serves **all 40 stops on time instead of ' || (40 - t.late_stops) ||
        '**, drives **' || round(t.km - b.km)::INTEGER || ' street km less** (' || round(b.km)::INTEGER || ' instead of ' || round(t.km)::INTEGER || '), needs **' ||
        round(t.driver_h - b.driver_h, 1) || ' driver hours less** (' || b.driver_h || ' instead of ' || t.driver_h || ') and brings the last van back at **' ||
        dr_clock(b.last_back_min) || '** instead of ' || dr_clock(t.last_back_min) || ': **€' || (t.eur - b.eur)::INTEGER ||
        ' less per day** (€' || b.eur::INTEGER || ' instead of €' || t.eur::INTEGER || '), about **€' || round((t.eur - b.eur) * 250 / 1000)::INTEGER || 'k a year** at 250 delivery days.' ||
        chr(10) || chr(10) ||
        '**More vans don''t save money here.** A ' ||
        coalesce((SELECT string_agg(CASE n.vans WHEN 5 THEN 'fifth' ELSE 'sixth' END || ' van on the ' || b.plan || ' plan costs **€' || (n.eur - b.eur)::INTEGER || ' more** per day (+€' ||
                                    (n.eur_van - b.eur_van)::INTEGER || ' day rate, ' || CASE WHEN n.eur_h >= b.eur_h THEN '+' ELSE '−' END || '€' || abs(n.eur_h - b.eur_h)::INTEGER || ' driver time, ' ||
                                    CASE WHEN n.eur_km >= b.eur_km THEN '+' ELSE '−' END || '€' || abs(n.eur_km - b.eur_km)::INTEGER || ' km — each van drives out and back)',
                                    '; a ' ORDER BY n.vans)
                  FROM dr_parts() n WHERE n.plan = b.plan AND n.vans > b.vans), '') ||
        ' and avoids no late delivery, since ' || b.vans || ' vans are already on time. The shortest-route plan is cheaper on kilometres (' || round(s.km)::INTEGER ||
        ' km with ' || b.vans || ' vans) but leaves **' || s.late_stops || '** ' || CASE WHEN s.late_stops = 1 THEN 'customer' ELSE 'customers' END || ' waiting (€' || s.eur_late::INTEGER || ' in penalties at €' || printf('%g', dr_eur_late()) || ' each).')::MARKDOWN
FROM dr_best() b, dr_best_today() t
JOIN dr_parts() s ON s.plan = 'shortest route' AND s.vans = t.vans;

-- Cost per day by component, one panel per plan (today's plan first), the
-- bars by van count.
SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT vans || ' vans' ::XAXIS, part ::CATEGORY, eur ::BARCHART_STACKED, '€'::YFORMAT,
       dr_title(plan) ::FACET, 3 ::FACET_NCOL,
       'Cost per day by component (€), for each plan and number of vans — every extra van adds its day rate, while kilometres and driver time hardly shrink'::TITLE
FROM (SELECT plan, ord, vans, '1 · street km' AS part, eur_km AS eur FROM dr_parts()
      UNION ALL SELECT plan, ord, vans, '2 · driver time', eur_h FROM dr_parts()
      UNION ALL SELECT plan, ord, vans, '3 · van day rates', eur_van FROM dr_parts()
      UNION ALL SELECT plan, ord, vans, '4 · late penalties', eur_late FROM dr_parts())
ORDER BY ord, vans, part;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT vans || ' vans' ::XAXIS, dr_name(plan) ::CATEGORY, 'today''s plan:neutral'::COLORS, late_stops ::BARCHART, ''::DATALABELS,
       'Stops late, by plan and number of vans (0 = every customer on time)'::TITLE
FROM dr_summary ORDER BY vans, ord;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT what ::XAXIS, round(100 * b / nullif(t, 0)) ::BARCHART, '%'::YFORMAT, ''::DATALABELS, 100 ::REFLINE, 'today''s plan = 100%'::REFLINE_LABEL,
       'The recommendation as a share of today''s plan (lower is better)'::TITLE
FROM (SELECT 1 AS i, 'Cost per day' AS what, t.eur AS t, b.eur AS b FROM dr_best() b, dr_best_today() t
      UNION ALL SELECT 2, 'Driver hours', t.driver_h, b.driver_h FROM dr_best() b, dr_best_today() t
      UNION ALL SELECT 3, 'Street km', t.km, b.km FROM dr_best() b, dr_best_today() t
      UNION ALL SELECT 4, 'Stops late', t.late_stops, b.late_stops FROM dr_best() b, dr_best_today() t)
ORDER BY 2 DESC;

SELECT 12::COL;
SELECT 'Stops late: >0:bad; good'::THRESHOLDS;
SELECT 'none'::TOTALS;
SELECT 'Plan: Vans, Plan | Result: Stops late, Street km, Driver hours, Last van back, Fullest van (cages) | Cost per day (€): Km €, Drivers €, Van rates €, Penalties €, Total €'::COLGROUP;
SELECT vans || ' vans' AS "Vans" ::TABLE, dr_title(plan) AS "Plan",
       late_stops AS "Stops late", km AS "Street km", driver_h AS "Driver hours",
       dr_clock(last_back_min) AS "Last van back" ::PLAIN, max_cages AS "Fullest van (cages)",
       eur_km AS "Km €", eur_h AS "Drivers €", eur_van AS "Van rates €", eur_late AS "Penalties €", eur AS "Total €",
       CASE WHEN plan = (SELECT plan FROM dr_best()) AND vans = (SELECT vans FROM dr_best()) THEN 'ok: recommended'
            WHEN late_stops > 0 THEN 'bad: misses deadlines' ELSE 'on time, dearer' END AS "Verdict" ::BADGE,
       'Every plan, priced at the cost assumptions'::TITLE
FROM dr_parts() ORDER BY ord, vans;
