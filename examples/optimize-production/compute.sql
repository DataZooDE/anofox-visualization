-- Production scheduling with changeovers — the native computation behind the
-- stored results of web/examples/optimize-production.sql.
--
-- Runs natively only: the anofox_optimize wasm build doesn't load in DuckDB-Wasm
-- yet (see ../optimize-showcase/README.md), so the browser demo reads the
-- tables this script produces from web/data/optimize-production/*.csv.
-- Regenerate them with scripts/gen-optimize-cases.sh production.
--
-- A made-up soft-drinks bottler plans week 43 (Mon 19 Oct 2026 06:00 to
-- Sat 24 Oct 06:00, three shifts a day) on three filling lines. Every line has
-- 14 orders with a truck slot (due date); key-account promotion orders weigh 3.
-- Changeovers are sequence-dependent: a lot change is quick, a heavier flavour
-- after a lighter one needs a rinse, the way back (cola → water) a full clean,
-- a new bottle format a parts change. Every input is synthetic and
-- deterministic (hash-based noise, no files).
LOAD anofox_optimize;

CREATE OR REPLACE MACRO pp_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

-- ── Master data ──────────────────────────────────────────────────────────────
-- Lines: format family and filling speed (bottles per hour).
CREATE OR REPLACE TABLE pp_lines AS
SELECT * FROM (VALUES
    ('L1 glass', 'returnable glass', 22000),
    ('L2 PET',   'PET bottles',      30000),
    ('L3 cans',  'cans',             48000)) t(line, format_family, bottles_per_h);

-- Flavours from light to heavy: the rinse after a lighter flavour is short,
-- the way back needs a full clean (CIP). Sugar-free needs its own rinse.
CREATE OR REPLACE TABLE pp_flavours AS
SELECT * FROM (VALUES
    (0, 'still water', false), (1, 'sparkling water', false), (2, 'apple spritzer', false),
    (3, 'lemon', false), (4, 'orange', false), (5, 'ginger ale', false),
    (6, 'cola', false), (7, 'cola zero', true)) t(rank, flavour, sugar_free);

-- Products = flavour × format; each line fills two formats.
CREATE OR REPLACE TABLE pp_products AS
SELECT l.line, f.rank, f.flavour, f.sugar_free, fm.format,
       f.flavour || ' ' || fm.format AS product
FROM pp_lines l
JOIN (VALUES ('L1 glass', '0.33 l'), ('L1 glass', '0.7 l'), ('L2 PET', '0.5 l'), ('L2 PET', '1.5 l'),
             ('L3 cans', '0.33 l'), ('L3 cans', '0.5 l')) fm(line, format) ON fm.line = l.line
CROSS JOIN pp_flavours f
WHERE NOT (l.line = 'L1 glass' AND f.rank IN (5, 7))      -- no ginger ale / cola zero in glass
  AND NOT (l.line = 'L3 cans' AND f.rank IN (0, 2));      -- no still water / spritzer in cans

-- The changeover rule (minutes) between two products of one line.
CREATE OR REPLACE MACRO pp_changeover(r_from, sf_from, fmt_from, r_to, sf_to, fmt_to) AS
    CASE WHEN r_from = r_to AND fmt_from = fmt_to THEN 10.0                -- lot change
         ELSE (CASE WHEN r_from = r_to THEN 0.0
                    WHEN r_to > r_from THEN 20.0 + 5 * (r_to - r_from)     -- rinse, light → heavy
                    ELSE 80.0 END)                                          -- full clean, heavy → light
              + CASE WHEN fmt_from <> fmt_to THEN 45.0 ELSE 0.0 END         -- format parts
              + CASE WHEN sf_from <> sf_to THEN 25.0 ELSE 0.0 END END;      -- sugar ↔ sugar-free

-- ── Week 43's order book: 14 orders per line ─────────────────────────────────
-- Volume in thousand bottles. Sales promised each order the truck slot after
-- its share of the line's week (cumulative filling time plus an hour per
-- changeover, ± a random slack), snapped to the slots at 14:00 and 22:00;
-- due = minutes after Mon 06:00. Orders are listed as booked (by order
-- number), which is how the planner receives them.
CREATE OR REPLACE MACRO pp_slot(t) AS
    least(4 * 1440 + 960,
          CASE WHEN t - floor(t / 1440) * 1440 <= 480 THEN floor(t / 1440) * 1440 + 480
               WHEN t - floor(t / 1440) * 1440 <= 960 THEN floor(t / 1440) * 1440 + 960
               ELSE floor(t / 1440) * 1440 + 1440 + 480 END)::INTEGER;

CREATE OR REPLACE TABLE pp_orders AS
WITH o AS (
    SELECT l.line, l.bottles_per_h, i,
           (SELECT list(product ORDER BY rank, format) FROM pp_products p WHERE p.line = l.line) AS prods,
           pp_unif(l.line, i, 'p') AS u_p, pp_unif(l.line, i, 'v') AS u_v,
           pp_unif(l.line, i, 'd') AS u_d, pp_unif(l.line, i, 'w') AS u_w, pp_unif(l.line, i, 'c') AS u_c,
           round(60 * (3 + 7 * pp_unif(l.line, i, 'v'))) AS proc_min                -- 3–10 h at line speed
    FROM pp_lines l, range(1, 15) t(i)
), promised AS (
    SELECT *, sum(proc_min + 60) OVER (PARTITION BY line ORDER BY u_d) * (0.9 + 0.35 * pp_unif(line, i, 's')) AS promise_min
    FROM o
)
SELECT o.line, 'O' || substr(o.line, 2, 1) || lpad(o.i::VARCHAR, 2, '0') AS order_id, o.i AS seq_in_book,
       p.product, p.flavour, p.format, p.rank, p.sugar_free,
       ['Nordmarkt', 'Frischkauf', 'GetränkeWelt', 'Hotel & Gastro Service', 'Export NL', 'Kiosk-Großhandel'][1 + floor(6 * o.u_c)::INTEGER] AS customer,
       CASE WHEN o.u_w < 0.25 THEN 3.0 ELSE 1.0 END AS weight,
       round(o.bottles_per_h * o.proc_min / 60 / 1000) AS volume_kbtl,
       o.proc_min, pp_slot(o.promise_min) AS due_min
FROM promised o JOIN pp_products p ON p.line = o.line
     AND p.product = o.prods[1 + floor(len(o.prods) * o.u_p)::INTEGER];

-- ── Scenarios: demand × changeover speed ─────────────────────────────────────
-- volume_factor scales the filling time; changeover_factor the cleaning and
-- format times (0.6 = the SMED / fast-CIP project the plant is discussing).
CREATE OR REPLACE TABLE pp_scenarios AS
SELECT v.volume, v.volume_factor, c.changeovers, c.changeover_factor
FROM (VALUES ('as booked', 1.0), ('-10 % (cool week)', 0.9), ('+10 % (heatwave)', 1.1), ('+20 % (heatwave + promo)', 1.2)) v(volume, volume_factor),
     (VALUES ('today''s changeovers', 1.0), ('SMED project (-40 %)', 0.6)) c(changeovers, changeover_factor);

-- Setup matrix per line and scenario: (n+1)×(n+1), row/col 0 = the virtual
-- start (the line was last cleaned on Friday: a water-like start, any format).
CREATE OR REPLACE TABLE pp_setup AS
WITH n AS (
    SELECT line, 0 AS k, -1 AS rank, false AS sugar_free, '' AS format FROM pp_lines
    UNION ALL SELECT line, seq_in_book, rank, sugar_free, format FROM pp_orders
)
SELECT s.volume, s.changeovers, a.line,
       list(CASE WHEN b.k = 0 THEN 0.0
                 WHEN a.k = 0 THEN 30.0 * s.changeover_factor
                 ELSE s.changeover_factor * pp_changeover(a.rank, a.sugar_free, a.format, b.rank, b.sugar_free, b.format) END
            ORDER BY a.k, b.k) AS m
FROM pp_scenarios s, n a JOIN n b ON a.line = b.line
GROUP BY ALL;

-- ── The solver runs: 6 rules × 3 lines × 8 scenarios ─────────────────────────
-- Five opt_schedule_* rules minimise priority-weighted lateness; the sixth,
-- "fewest changeovers", runs opt_sequence_best_of on the same matrix and
-- ignores due dates — the trade-off the planner has to see.
CREATE OR REPLACE TABLE pp_runs AS
WITH l AS (
    SELECT s.volume, s.changeovers, s.volume_factor, o.line,
           list(o.proc_min * s.volume_factor ORDER BY o.seq_in_book) AS p,
           list(o.due_min::DOUBLE ORDER BY o.seq_in_book) AS d,
           list(o.weight ORDER BY o.seq_in_book) AS w, count(*) AS n, any_value(m.m) AS m
    FROM pp_orders o, pp_scenarios s
    JOIN pp_setup m ON m.volume = s.volume AND m.changeovers = s.changeovers
    WHERE m.line = o.line
    GROUP BY ALL
), seqm AS (  -- the n×n block of the setup matrix (without the virtual start)
    SELECT *, [m[(x // n + 1) * (n + 1) + x % n + 2] FOR x IN range(n * n)] AS mm FROM l
)
SELECT volume, changeovers, line, a.rule, a.ord, a.job_order
FROM seqm, LATERAL (VALUES
    ('due date order (today)', 1, opt_schedule_edd(p, d, w, m, n)."order"),
    ('WSPT',                    2, opt_schedule_wspt(p, d, w, m, n)."order"),
    ('ATCS',                    3, opt_schedule_atcs(p, d, w, m, n)."order"),
    ('local search',            4, opt_schedule_local_search(p, d, w, m, n)."order"),
    ('exact (proven optimum)',  5, opt_schedule_exact(p, d, w, m, n)."order"),
    ('fewest changeovers',      6, opt_sequence_best_of(mm, n)."order")
) a(rule, ord, job_order);

-- Gantt rows: every order of every run with its changeover before it.
CREATE OR REPLACE TABLE pp_gantt AS
WITH seq AS (
    SELECT r.volume, r.changeovers, r.line, r.rule, r.ord, u.pos::INTEGER AS pos, (u.j + 1)::INTEGER AS k,
           coalesce(lag((u.j + 1)::INTEGER) OVER (PARTITION BY r.volume, r.changeovers, r.line, r.rule ORDER BY u.pos), 0) AS prev_k
    FROM pp_runs r, LATERAL unnest(r.job_order) WITH ORDINALITY u(j, pos)
), t AS (
    SELECT q.*, o.order_id, o.product, o.flavour, o.format, o.customer, o.weight, o.due_min,
           round(o.volume_kbtl * s.volume_factor) AS volume_kbtl,
           o.proc_min * s.volume_factor AS proc_min,
           m.m[q.prev_k * (n.n + 1) + q.k + 1] AS setup_min,
           CASE WHEN q.prev_k = 0 THEN 'start-up'
                WHEN po.product = o.product THEN 'lot change'
                WHEN po.rank > o.rank THEN 'full clean'
                WHEN po.rank < o.rank THEN 'rinse'
                ELSE 'format parts' END AS changeover_type
    FROM seq q
    JOIN pp_orders o ON o.line = q.line AND o.seq_in_book = q.k
    LEFT JOIN pp_orders po ON po.line = q.line AND po.seq_in_book = q.prev_k
    JOIN pp_scenarios s ON s.volume = q.volume AND s.changeovers = q.changeovers
    JOIN pp_setup m ON m.volume = q.volume AND m.changeovers = q.changeovers AND m.line = q.line
    JOIN (SELECT line, count(*) AS n FROM pp_orders GROUP BY line) n ON n.line = q.line
)
SELECT volume, changeovers, line, rule, ord, pos, order_id, product, flavour, format, customer, weight,
       volume_kbtl, due_min, round(setup_min) AS changeover_min, changeover_type,
       round(sum(setup_min + proc_min) OVER w - proc_min) AS start_min,
       round(sum(setup_min + proc_min) OVER w) AS end_min,
       round(greatest(0, sum(setup_min + proc_min) OVER w - due_min)) AS late_min
FROM t WINDOW w AS (PARTITION BY volume, changeovers, line, rule ORDER BY pos ROWS UNBOUNDED PRECEDING);

-- One row per (scenario, line, rule).
CREATE OR REPLACE TABLE pp_algos AS
SELECT volume, changeovers, line, rule, ord,
       sum(weight * late_min) AS weighted_late_min,
       count(*) FILTER (WHERE late_min > 0) AS late_orders,
       max(late_min) AS max_late_min,
       sum(changeover_min) AS changeover_min,
       max(end_min) AS makespan_min
FROM pp_gantt GROUP BY ALL;

-- The changeover matrix of one line's products (as-booked, today's times) for
-- the heatmap.
CREATE OR REPLACE TABLE pp_matrix AS
SELECT a.line, a.product AS from_product, a.rank * 10 + (a.format > '0.4') ::INTEGER AS from_ord,
       b.product AS to_product, b.rank * 10 + (b.format > '0.4')::INTEGER AS to_ord,
       pp_changeover(a.rank, a.sugar_free, a.format, b.rank, b.sugar_free, b.format) AS minutes
FROM (SELECT DISTINCT line, product, rank, sugar_free, format FROM pp_orders) a
JOIN (SELECT DISTINCT line, product, rank, sugar_free, format FROM pp_orders) b ON a.line = b.line;
