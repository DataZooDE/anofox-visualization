-- Delivery routing — the native computation behind the stored results of
-- web/examples/optimize-delivery.sql.
--
-- Runs natively only: the anofox_optimize wasm build doesn't load in DuckDB-Wasm
-- yet (see ../optimize-showcase/README.md), so the browser demo reads the
-- tables this script produces from web/data/optimize-delivery/*.csv.
-- Regenerate them with scripts/gen-optimize-cases.sh delivery.
--
-- A made-up food wholesaler delivers tomorrow (Tue 20 Oct 2026) to 40
-- bakeries, cafés, restaurants, shops and canteens around its depot. Every stop
-- has a "deliver by" deadline (bakeries before they open, restaurants before
-- lunch) and a number of roll cages; a van takes 32 cages, leaves at 06:00 and
-- drives at 40 km/h. The town is a street grid, a street every kilometre each
-- way: every stop and the depot sit on a crossing, a van drives along the
-- streets, so the distance between two places is the street distance
-- |dx| + |dy| (km) — the matrix the solvers get, the km on the dashboard, and
-- the routes drawn along the streets (dr_legs) all use it.
--
-- The plan is "cluster first, route second":
--   1. assign stops to vans — today by fixed districts (one per driver);
--      optimised by a capacity-balanced sweep around the depot, tried from 24
--      start angles (plain SQL);
--   2. order each van's stops with anofox_optimize:
--      * opt_sequence_* — the shortest route (setup matrix = driving minutes),
--      * opt_schedule_* — the route with the least weighted lateness against
--        the deadlines (setup = driving, processing = unloading, due = deadline);
--   3. keep, per van count and plan, the start angle with the best result.
-- Every input is synthetic and deterministic (hash-based noise, no files).
LOAD anofox_optimize;

CREATE OR REPLACE MACRO dr_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO dr_km(x1, y1, x2, y2) AS abs(x1 - x2) + abs(y1 - y2);   -- street km on the grid
CREATE OR REPLACE MACRO dr_drive(km) AS km * 60.0 / 40.0;   -- minutes at 40 km/h

-- ── Stops: 40 customers, depot at (0, 0), coordinates in km ─────────────────
CREATE OR REPLACE TABLE dr_stops AS
WITH s AS (
    SELECT i,
           dr_unif('k', i, 0) AS u_k,
           2 * pi() * dr_unif('a', i, 0) AS ang,
           3 + 15 * sqrt(dr_unif('r', i, 0)) AS rad
    FROM range(1, 41) t(i)
), g AS (
    -- on the street grid: the nearest crossing; a crossing taken by an earlier
    -- stop pushes the next one a block further east
    SELECT *, round(rad * cos(ang)) + row_number() OVER (PARTITION BY round(rad * cos(ang)), round(rad * sin(ang)) ORDER BY i) - 1 AS gx,
           round(rad * sin(ang)) AS gy
    FROM s
), k AS (
    SELECT *, CASE WHEN u_k < 0.25 THEN 'bakery' WHEN u_k < 0.45 THEN 'café' WHEN u_k < 0.65 THEN 'restaurant'
                   WHEN u_k < 0.90 THEN 'shop' ELSE 'canteen' END AS kind
    FROM g
)
SELECT i AS stop_id,
       (CASE kind WHEN 'bakery' THEN ['Backstube', 'Bäckerei Korn', 'Brotzeit', 'Bäckerei Hahn', 'Mühlenbäcker']
                  WHEN 'café' THEN ['Café Linde', 'Café am Markt', 'Kaffeehaus', 'Café Rosa', 'Espressobar']
                  WHEN 'restaurant' THEN ['Trattoria Sole', 'Gasthaus Krone', 'Bistro 12', 'Zum Anker', 'Thai Garden']
                  WHEN 'shop' THEN ['Feinkost Weber', 'Bio-Laden', 'Dorfladen', 'Markthalle', 'Kiosk Nord']
                  ELSE ['Klinikum', 'Schulmensa', 'Werkskantine', 'Rathauskantine', 'Uni-Mensa'] END)[1 + floor(5 * dr_unif('n', i, 0))::INTEGER]
           || ' ' || i AS name,
       kind,
       CASE WHEN ang < pi() / 2 THEN 'North-East' WHEN ang < pi() THEN 'North-West'
            WHEN ang < 3 * pi() / 2 THEN 'South-West' ELSE 'South-East' END AS district,
       gx::DOUBLE AS x_km, gy::DOUBLE AS y_km,
       1 + floor(3 * dr_unif('c', i, 0))::INTEGER AS cages,
       round(8 + 12 * dr_unif('s', i, 0)) AS service_min,
       -- deadline in minutes after 06:00
       CASE kind WHEN 'bakery' THEN 90 WHEN 'café' THEN 120 WHEN 'canteen' THEN 180
                 WHEN 'shop' THEN 210 ELSE 240 END
           + 30 * floor(3 * dr_unif('d', i, 0))::INTEGER AS deliver_by_min,
       CASE WHEN kind = 'canteen' OR dr_unif('w', i, 0) < 0.15 THEN 3.0 ELSE 1.0 END AS weight
FROM k;

-- ── Scenarios: 4, 5 or 6 vans; 24 sweep start angles ────────────────────────
CREATE OR REPLACE TABLE dr_assign AS
WITH tot AS (SELECT sum(cages) AS c FROM dr_stops),
     polar AS (SELECT *, atan2(y_km, x_km) AS a FROM dr_stops),
     -- today: the four districts, split further when there are more vans
     today AS (
        SELECT v.vans, 0 AS rot, p.stop_id,
               least(v.vans, 1 + floor(v.vans * ((p.a + pi()) / (2 * pi())))::INTEGER) AS van   -- a = π (due west) stays in the last district
        FROM polar p, (VALUES (4), (5), (6)) v(vans)
     ),
     sweep AS (
        SELECT v.vans, r.rot, p.stop_id, p.cages,
               ((p.a + pi()) / (2 * pi()) + r.rot / 24.0) % 1.0 AS pos
        FROM polar p, (VALUES (4), (5), (6)) v(vans), range(24) r(rot)
     ),
     cum AS (
        SELECT *, sum(cages) OVER (PARTITION BY vans, rot ORDER BY pos, stop_id) - cages / 2.0 AS mid FROM sweep
     )
SELECT 'today' AS method, vans, rot, stop_id, van FROM today
UNION ALL
SELECT 'sweep', vans, rot, stop_id, least(vans, 1 + floor(mid / ((SELECT c FROM tot) / vans)))::INTEGER FROM cum;

CREATE OR REPLACE MACRO dr_seq_cell(i, j, n, xs, ys) AS
    CASE WHEN i = j THEN 0.0
         WHEN j = 0 OR i = n + 1 THEN 1e6                 -- nothing returns to the start, nothing leaves the end
         WHEN i = 0 AND j = n + 1 THEN 1e6
         WHEN i = 0 THEN dr_drive(dr_km(0, 0, xs[j], ys[j]))
         WHEN j = n + 1 THEN 1e4 + dr_drive(dr_km(xs[i], ys[i], 0, 0))
         ELSE dr_drive(dr_km(xs[i], ys[i], xs[j], ys[j])) END;
CREATE OR REPLACE MACRO dr_sched_cell(i, j, xs, ys) AS
    CASE WHEN j = 0 THEN 0.0
         WHEN i = 0 THEN dr_drive(dr_km(0, 0, xs[j], ys[j]))
         ELSE dr_drive(dr_km(xs[i], ys[i], xs[j], ys[j])) END;

-- One row per (method, vans, rot, van) with the matrices the solvers need.
-- Sequence matrix: node 0 = depot (start), 1..n = stops, n+1 = depot (end);
-- the big constants keep the depot first and last (subtracted again below).
CREATE OR REPLACE TABLE dr_groups AS
WITH m AS (SELECT a.method, a.vans, a.rot, a.van,
                  list(s.stop_id ORDER BY s.stop_id) AS ids,
                  list(s.x_km ORDER BY s.stop_id) AS xs, list(s.y_km ORDER BY s.stop_id) AS ys,
                  list(s.service_min ORDER BY s.stop_id) AS svc,
                  list(s.deliver_by_min::DOUBLE ORDER BY s.stop_id) AS due,
                  list(s.weight ORDER BY s.stop_id) AS w,
                  count(*) AS n
           FROM dr_assign a JOIN dr_stops s USING (stop_id) GROUP BY ALL)
SELECT *,
       -- (n+2)×(n+2) driving minutes for opt_sequence_*
       [dr_seq_cell(x // (n + 2), x % (n + 2), n, xs, ys) FOR x IN range((n + 2) * (n + 2))] AS seq_m,
       -- (n+1)×(n+1) for opt_schedule_*: row 0 = the depot (virtual start)
       [dr_sched_cell(x // (n + 1), x % (n + 1), xs, ys) FOR x IN range((n + 1) * (n + 1))] AS sched_m
FROM m;

-- ── Solve every van of every candidate ───────────────────────────────────────
-- Orders are 0-based over the matrix nodes; *_order lists stop positions 1..n
-- within the van's group.
-- (opt_schedule_exact refuses more than 14 stops; larger vans take best_of.)
CREATE OR REPLACE TABLE dr_solved AS
SELECT g.*,
       [k::INTEGER FOR k IN opt_sequence_as_given(seq_m, n + 2)."order" IF k BETWEEN 1 AND n] AS list_order,
       [k::INTEGER FOR k IN opt_sequence_best_of(seq_m, n + 2)."order" IF k BETWEEN 1 AND n] AS short_order,
       [(k + 1)::INTEGER FOR k IN opt_schedule_exact(svc, due, w, sched_m, n)."order"] AS deadline_order
FROM dr_groups g WHERE n <= 14
UNION ALL
SELECT g.*,
       [k::INTEGER FOR k IN opt_sequence_as_given(seq_m, n + 2)."order" IF k BETWEEN 1 AND n],
       [k::INTEGER FOR k IN opt_sequence_best_of(seq_m, n + 2)."order" IF k BETWEEN 1 AND n],
       [(k + 1)::INTEGER FOR k IN opt_schedule_best_of(svc, due, w, sched_m, n)."order"]
FROM dr_groups g WHERE n > 14;

-- Every plan we compare: (method, sequencing) pairs.
CREATE OR REPLACE TABLE dr_candidates AS
SELECT method, vans, rot, van, ids, xs, ys, svc, due, w, n, cand.plan, cand.ord
FROM dr_solved, LATERAL (VALUES
    ('today: districts, order list', list_order),
    ('shortest route', short_order),
    ('deadline schedule', deadline_order)) cand(plan, ord)
WHERE (method = 'today') = (cand.plan LIKE 'today%');

-- Stop-level simulation of every candidate route (depot → stops → depot).
CREATE OR REPLACE TABLE dr_sim AS
WITH u AS (
    SELECT c.plan, c.vans, c.rot, c.van, p.pos::INTEGER AS pos, c.ids[p.k] AS stop_id,
           c.xs[p.k] AS x_km, c.ys[p.k] AS y_km, c.svc[p.k] AS service_min, c.due[p.k] AS deliver_by_min, c.w[p.k] AS weight,
           lag(c.xs[p.k], 1, 0) OVER v AS px, lag(c.ys[p.k], 1, 0) OVER v AS py
    FROM dr_candidates c, LATERAL unnest(c.ord) WITH ORDINALITY p(k, pos)
    WINDOW v AS (PARTITION BY c.plan, c.vans, c.rot, c.van ORDER BY p.pos)
), l AS (
    SELECT *, dr_km(px, py, x_km, y_km) AS leg_km FROM u
), t AS (
    SELECT *, sum(dr_drive(leg_km)) OVER v + coalesce(sum(service_min) OVER (v ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS arrive_min
    FROM l WINDOW v AS (PARTITION BY plan, vans, rot, van ORDER BY pos)
)
SELECT *, greatest(0, arrive_min - deliver_by_min) AS late_min FROM t;

-- The deadline-aware plan takes, van by van, the opt_schedule_* order — or the
-- shortest route when that is just as punctual (the schedule objective only
-- counts lateness, so among punctual orders it doesn't prefer short ones).
CREATE OR REPLACE TABLE dr_van_score AS
SELECT plan, vans, rot, van, sum(leg_km) + dr_km(arg_max(x_km, pos), arg_max(y_km, pos), 0, 0) AS km,
       sum(weight * late_min) AS wlate
FROM dr_sim GROUP BY ALL;

CREATE OR REPLACE TABLE dr_sim AS
WITH pick AS (
    SELECT vans, rot, van, arg_min(plan, (round(wlate, 3), CASE WHEN plan = 'shortest route' THEN 0 ELSE 1 END, round(km, 3))) AS src
    FROM dr_van_score WHERE plan IN ('shortest route', 'deadline schedule') GROUP BY ALL
)
SELECT * FROM dr_sim WHERE plan <> 'deadline schedule'
UNION ALL BY NAME
SELECT d.* REPLACE ('deadline-aware' AS plan) FROM dr_sim d JOIN pick p ON p.vans = d.vans AND p.rot = d.rot AND p.van = d.van AND p.src = d.plan;

-- Per candidate: total km (with the drive back), lateness; choose the start
-- angle per (plan, vans): shortest route → fewest km; deadline-aware → least
-- weighted lateness, then km.
CREATE OR REPLACE TABLE dr_cand_score AS
WITH per_van AS (
    SELECT plan, vans, rot, van, sum(leg_km) + dr_km(arg_max(x_km, pos), arg_max(y_km, pos), 0, 0) AS km,
           sum(weight * late_min) AS wlate
    FROM dr_sim GROUP BY ALL
)
SELECT *, row_number() OVER (PARTITION BY plan, vans ORDER BY
           CASE WHEN plan = 'deadline-aware' THEN wlate ELSE 0 END, km, rot) AS rk
FROM (SELECT plan, vans, rot, round(sum(km), 3) AS km, round(sum(wlate), 3) AS wlate FROM per_van GROUP BY ALL);  -- rounded: ties break by rot, deterministically

-- ── The stored results ───────────────────────────────────────────────────────
-- Route rows: stops in visiting order, plus the depot at both ends.
CREATE OR REPLACE TABLE dr_route AS
WITH best AS (SELECT plan, vans, rot FROM dr_cand_score WHERE rk = 1),
     s AS (SELECT d.* FROM dr_sim d JOIN best USING (plan, vans, rot))
SELECT plan, vans, van, pos, stop_id, x_km, y_km, round(leg_km, 2) AS leg_km,
       round(arrive_min) AS arrive_min, deliver_by_min, round(late_min) AS late_min, weight
FROM s
UNION ALL
SELECT plan, vans, van, 0, 0, 0, 0, 0, 0, NULL, 0, NULL FROM s GROUP BY ALL
UNION ALL
SELECT plan, vans, van, max(pos) + 1, 0, 0, 0,
       round(dr_km(arg_max(x_km, pos), arg_max(y_km, pos), 0, 0), 2),
       round(max(arrive_min + service_min) + dr_drive(dr_km(arg_max(x_km, pos), arg_max(y_km, pos), 0, 0))), NULL, 0, NULL
FROM s GROUP BY ALL;

-- Per van.
CREATE OR REPLACE TABLE dr_vans AS
SELECT r.plan, r.vans, r.van, count(*) FILTER (WHERE r.stop_id > 0) AS stops,
       sum(s.cages) AS cages, round(sum(r.leg_km), 1) AS km,
       max(r.arrive_min) AS back_min,
       count(*) FILTER (WHERE r.late_min > 0) AS late_stops,
       sum(r.weight * r.late_min) AS weighted_late_min
FROM dr_route r LEFT JOIN dr_stops s ON s.stop_id = r.stop_id AND r.stop_id > 0
GROUP BY ALL;

-- Per plan and van count.
CREATE OR REPLACE TABLE dr_summary AS
SELECT plan, vans, CASE WHEN plan LIKE 'today%' THEN 1 WHEN plan = 'shortest route' THEN 2 ELSE 3 END AS ord,
       round(sum(km), 1) AS km, sum(late_stops) AS late_stops, sum(weighted_late_min) AS weighted_late_min,
       max(back_min) AS last_back_min, round(sum(back_min) / 60, 1) AS driver_h,
       max(cages) AS max_cages, max(stops) AS max_stops
FROM dr_vans GROUP BY ALL;

-- The routes as WKT lines (km coordinates) along the streets, one per van:
-- each leg runs east-west first, then north-south (one of the shortest
-- street paths).
CREATE OR REPLACE TABLE dr_legs AS
WITH c AS (
    SELECT *, lag(y_km, 1, 0) OVER (PARTITION BY plan, vans, van ORDER BY pos) AS py FROM dr_route
)
SELECT plan, vans, van,
       'LINESTRING(0.0 0.0, ' || string_agg(x_km || ' ' || py || ', ' || x_km || ' ' || y_km, ', ' ORDER BY pos) FILTER (WHERE pos > 0) || ')' AS wkt
FROM c GROUP BY ALL;
