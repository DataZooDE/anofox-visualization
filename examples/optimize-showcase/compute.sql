-- anofox-optimize showcase — the native computation behind the stored results.
--
-- Runs natively only: the anofox_optimize wasm build doesn't load in DuckDB-Wasm
-- yet (it links the telemetry HTTP client, see README.md), so the browser demo
-- reads the tables this script produces from web/data/optimize/*.csv.
-- Regenerate them with scripts/gen-showcase-data.sh optimize.
--
-- A made-up regional distribution centre on one Friday. Every input is
-- synthetic and deterministic (hash-based noise, no files); every output is one
-- call of an anofox_optimize function, usually several algorithms of one family
-- side by side so the baseline ("what we do today") is always visible.
LOAD anofox_optimize;

CREATE OR REPLACE MACRO os_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

-- ── A. Containers: 150 cartons into 1,000-unit containers (pack_*) ──────────
-- Sizes in volume units: a mix of small parcels, mid cartons and bulky ones.
CREATE OR REPLACE TABLE os_cartons AS
SELECT i AS carton,
       round(CASE WHEN os_unif('c', i, 'k') < 0.45 THEN 60 + 140 * os_unif('c', i, 's')
                  WHEN os_unif('c', i, 'k') < 0.85 THEN 200 + 200 * os_unif('c', i, 's')
                  ELSE 420 + 160 * os_unif('c', i, 's') END) AS size
FROM range(1, 151) t(i);

CREATE OR REPLACE TABLE os_pack_raw AS
WITH l AS (SELECT list(size ORDER BY carton) AS s FROM os_cartons)
SELECT a.algorithm, a.ord, r.bins_used, r.assignment
FROM l, LATERAL (VALUES
    ('next fit (today)', 1, opt_pack_next_fit(l.s, 1000.0)),
    ('first fit decreasing', 2, opt_pack_first_fit_decreasing(l.s, 1000.0)),
    ('best fit decreasing', 3, opt_pack_best_fit_decreasing(l.s, 1000.0)),
    ('worst fit decreasing', 4, opt_pack_worst_fit_decreasing(l.s, 1000.0)),
    ('bin completion', 5, opt_pack_bin_completion(l.s, 1000.0)),
    ('local search', 6, opt_pack_local_search(l.s, 1000.0)),
    ('best of', 7, opt_pack_best_of(l.s, 1000.0))
) a(algorithm, ord, res), LATERAL (SELECT a.res.bins_used::INTEGER AS bins_used, a.res.assignment AS assignment) r;

-- One row per algorithm: containers used vs the volume lower bound.
CREATE OR REPLACE TABLE os_pack_algos AS
SELECT algorithm, ord, bins_used,
       (SELECT ceil(sum(size) / 1000.0)::INTEGER FROM os_cartons) AS lower_bound,
       round(100.0 * (SELECT sum(size) FROM os_cartons) / (bins_used * 1000.0), 1) AS avg_fill_pct
FROM os_pack_raw;

-- One row per container: its fill, for today's rule and the best packer.
CREATE OR REPLACE TABLE os_pack_bins AS
SELECT p.algorithm, (u.bin + 1)::INTEGER AS container, round(sum(c.size) / 10.0, 1) AS fill_pct
FROM os_pack_raw p, LATERAL unnest(p.assignment) WITH ORDINALITY u(bin, k)
JOIN os_cartons c ON c.carton = u.k
WHERE p.algorithm IN ('next fit (today)', 'best of')
GROUP BY ALL;

-- ── B. Carrier choice: which shipments ride the cheap carrier (knapsack_*) ──
-- 60 shipments; the cheap carrier saves money per shipment but takes only a
-- limited weight. The curve sweeps its capacity.
CREATE OR REPLACE TABLE os_shipments AS
SELECT i AS shipment,
       round(40 + 260 * os_unif('s', i, 'w')) AS weight_kg,
       round(15 + 110 * os_unif('s', i, 'v') + 0.15 * (40 + 260 * os_unif('s', i, 'w'))) AS saving_eur
FROM range(1, 61) t(i);

CREATE OR REPLACE TABLE os_knap_curve AS
WITH l AS (SELECT list(saving_eur ORDER BY shipment) AS v, list(weight_kg ORDER BY shipment) AS w FROM os_shipments),
     caps AS (SELECT (c * 250)::DOUBLE AS capacity FROM range(1, 21) t(c))
SELECT capacity, a.algorithm, a.res.total_value AS saving_eur, a.res.total_weight AS loaded_kg
FROM l, caps, LATERAL (VALUES
    ('exact (proven optimum)', opt_knapsack_exact(l.v, l.w, capacity)),
    ('greedy by saving/kg', opt_knapsack_greedy_ratio(l.v, l.w, capacity)),
    ('greedy by saving (today)', opt_knapsack_greedy_value(l.v, l.w, capacity))
) a(algorithm, res);

-- The selection at today's 2,000 kg booking: which shipments each rule loads.
CREATE OR REPLACE TABLE os_knap_pick AS
WITH l AS (SELECT list(saving_eur ORDER BY shipment) AS v, list(weight_kg ORDER BY shipment) AS w FROM os_shipments),
     r AS (SELECT opt_knapsack_exact(v, w, 2000.0).selected AS ex,
                  opt_knapsack_greedy_value(v, w, 2000.0).selected AS gv FROM l)
SELECT s.shipment, s.weight_kg, s.saving_eur,
       round(s.saving_eur / s.weight_kg, 2) AS eur_per_kg,
       r.ex[s.shipment] AS exact_loads, r.gv[s.shipment] AS greedy_loads
FROM os_shipments s, r;

-- ── C. Pick waves: 48 orders into waves of 120 order lines (wave_*) ─────────
-- Priority 5 = same-day express … 1 = standard. Lower weighted mean wave =
-- high-priority orders are picked earlier.
CREATE OR REPLACE TABLE os_orders AS
SELECT i AS order_no,
       round(8 + 40 * os_unif('o', i, 'l')) AS lines,
       CASE WHEN os_unif('o', i, 'p') < 0.12 THEN 5 WHEN os_unif('o', i, 'p') < 0.30 THEN 3 ELSE 1 END AS priority
FROM range(1, 49) t(i);

CREATE OR REPLACE TABLE os_wave_raw AS
WITH l AS (SELECT list(lines ORDER BY order_no) AS s, list(priority::DOUBLE ORDER BY order_no) AS p FROM os_orders)
SELECT a.algorithm, a.ord, a.res.wave AS wave, a.res.avg_weighted_wave AS avg_weighted_wave
FROM l, LATERAL (VALUES
    ('arrival order (today)', 1, opt_wave_as_given(l.s, l.p, 120.0)),
    ('fewest waves', 2, opt_wave_fewest_waves(l.s, l.p, 120.0)),
    ('priority first', 3, opt_wave_priority_first(l.s, l.p, 120.0)),
    ('priority density', 4, opt_wave_priority_density(l.s, l.p, 120.0)),
    ('best of', 5, opt_wave_best_of(l.s, l.p, 120.0))
) a(algorithm, ord, res);

CREATE OR REPLACE TABLE os_wave_algos AS
SELECT algorithm, ord, round(avg_weighted_wave, 3) AS avg_weighted_wave, list_max(wave)::INTEGER AS waves
FROM os_wave_raw;

-- Order lines per wave, split by priority class.
CREATE OR REPLACE TABLE os_wave_load AS
SELECT w.algorithm, u.wave::INTEGER AS wave,
       CASE o.priority WHEN 5 THEN 'express' WHEN 3 THEN 'priority' ELSE 'standard' END AS class,
       sum(o.lines)::INTEGER AS lines
FROM os_wave_raw w, LATERAL unnest(w.wave) WITH ORDINALITY u(wave, k)
JOIN os_orders o ON o.order_no = u.k
WHERE w.algorithm IN ('arrival order (today)', 'best of')
GROUP BY ALL;

-- ── D. The packing line: 12 jobs with due dates and changeovers (schedule_*) ─
-- Jobs belong to 4 product families; a changeover inside a family takes 2 min,
-- across families 15 min (from the start: 5 min). Times in minutes from 06:00.
CREATE OR REPLACE TABLE os_jobs AS
SELECT i AS job, 'J' || lpad(i::VARCHAR, 2, '0') AS job_name,
       ['A','B','C','D'][1 + (i * 7) % 4] AS family,
       round(20 + 50 * os_unif('j', i, 'p')) AS proc_min,
       round(60 + 420 * os_unif('j', i, 'd')) AS due_min,
       CASE WHEN os_unif('j', i, 'w') < 0.25 THEN 3.0 ELSE 1.0 END AS weight
FROM range(1, 13) t(i);

CREATE OR REPLACE TABLE os_setup AS  -- (n+1)×(n+1), row/col 0 = virtual start
SELECT list(CASE WHEN a.job = 0 THEN 5.0 WHEN b.job = 0 THEN 0.0
                 WHEN a.family = b.family THEN 2.0 ELSE 15.0 END ORDER BY a.job, b.job) AS m
FROM (SELECT 0 AS job, NULL AS family UNION ALL SELECT job, family FROM os_jobs) a,
     (SELECT 0 AS job, NULL AS family UNION ALL SELECT job, family FROM os_jobs) b;

CREATE OR REPLACE TABLE os_sched_raw AS
WITH l AS (SELECT list(proc_min ORDER BY job) AS p, list(due_min ORDER BY job) AS d,
                  list(weight ORDER BY job) AS w, (SELECT m FROM os_setup) AS m, count(*) AS n FROM os_jobs)
SELECT a.algorithm, a.ord, a.res."order" AS job_order,
       a.res.total_weighted_tardiness AS weighted_tardiness, a.res.makespan AS makespan
FROM l, LATERAL (VALUES
    ('earliest due date (today)', 1, opt_schedule_edd(l.p, l.d, l.w, l.m, l.n)),
    ('WSPT', 2, opt_schedule_wspt(l.p, l.d, l.w, l.m, l.n)),
    ('ATCS', 3, opt_schedule_atcs(l.p, l.d, l.w, l.m, l.n)),
    ('local search', 4, opt_schedule_local_search(l.p, l.d, l.w, l.m, l.n)),
    ('exact (proven optimum)', 5, opt_schedule_exact(l.p, l.d, l.w, l.m, l.n))
) a(algorithm, ord, res);

CREATE OR REPLACE TABLE os_sched_algos AS
SELECT algorithm, ord, round(weighted_tardiness, 1) AS weighted_tardiness, round(makespan, 1) AS makespan
FROM os_sched_raw;

-- Gantt rows: start/end of every job in each order (setup before it included).
CREATE OR REPLACE TABLE os_sched_gantt AS
WITH seq AS (
    SELECT s.algorithm, u.pos::INTEGER AS pos, (u.j + 1)::INTEGER AS job,
           lag((u.j + 1)::INTEGER) OVER (PARTITION BY s.algorithm ORDER BY u.pos) AS prev_job
    FROM os_sched_raw s, LATERAL unnest(s.job_order) WITH ORDINALITY u(j, pos)
    WHERE s.algorithm IN ('earliest due date (today)', 'exact (proven optimum)')
), t AS (
    SELECT q.*, j.job_name, j.family, j.proc_min, j.due_min, j.weight,
           CASE WHEN q.prev_job IS NULL THEN 5.0
                WHEN (SELECT family FROM os_jobs WHERE job = q.prev_job) = j.family THEN 2.0 ELSE 15.0 END AS setup_min
    FROM seq q JOIN os_jobs j USING (job)
)
SELECT algorithm, pos, job_name, family, weight, due_min,
       sum(setup_min + proc_min) OVER w - proc_min AS start_min,
       sum(setup_min + proc_min) OVER w AS end_min,
       greatest(0, sum(setup_min + proc_min) OVER w - due_min) AS late_min
FROM t WINDOW w AS (PARTITION BY algorithm ORDER BY pos ROWS UNBOUNDED PRECEDING);

-- ── E. Paint line changeovers: 10 colours, light→dark is cheap (sequence_*) ─
CREATE OR REPLACE TABLE os_colours AS
SELECT * FROM (VALUES (0,'white',0),(1,'navy',8),(2,'ivory',1),(3,'red',6),(4,'sand',2),
                      (5,'black',9),(6,'yellow',3),(7,'green',5),(8,'grey',4),(9,'brown',7)) t(k, colour, darkness);
CREATE OR REPLACE TABLE os_seq_algos AS
WITH m AS (SELECT list(CASE WHEN a.k = b.k THEN 0.0 WHEN b.darkness >= a.darkness THEN 5.0 + 2 * (b.darkness - a.darkness)
                            ELSE 30.0 + 6 * (a.darkness - b.darkness) END ORDER BY a.k, b.k) AS m
           FROM os_colours a, os_colours b)
SELECT a.algorithm, a.ord, a.res.total_setup AS changeover_min,
       (SELECT string_agg(c.colour, ' → ' ORDER BY u.pos) FROM unnest(a.res."order") WITH ORDINALITY u(k, pos)
          JOIN os_colours c ON c.k = u.k) AS sequence
FROM m, LATERAL (VALUES
    ('order book (today)', 1, opt_sequence_as_given(m.m, 10)),
    ('nearest neighbour', 2, opt_sequence_nearest_neighbour(m.m, 10)),
    ('2-opt', 3, opt_sequence_two_opt(m.m, 10)),
    ('best of', 4, opt_sequence_best_of(m.m, 10))
) a(algorithm, ord, res);

-- ── F. Treasury: where to park the DC's reserve fund (portfolio_*) ───────────
-- 10 risky assets (excess return over cash / volatility) and a one-factor
-- covariance; Sharpe = excess return / volatility.
CREATE OR REPLACE TABLE os_assets AS
SELECT i AS k, name, ret, vol, beta FROM (VALUES
    (0,'Gov bonds',0.012,0.050,0.1),(1,'Corp bonds',0.022,0.075,0.3),(2,'High yield',0.035,0.110,0.6),
    (3,'Real estate',0.040,0.150,0.6),(4,'Infrastructure',0.038,0.120,0.5),(5,'Global equity',0.055,0.160,1.0),
    (6,'EM equity',0.065,0.230,1.2),(7,'Small caps',0.060,0.210,1.1),(8,'Gold',0.020,0.150,-0.3),
    (9,'Private credit',0.045,0.130,0.4)) t(i, name, ret, vol, beta);
CREATE OR REPLACE TABLE os_cov AS
SELECT list(CASE WHEN a.k = b.k THEN a.vol * a.vol
                 ELSE a.beta * b.beta * 0.16 * 0.16 * 0.85 END
            ORDER BY a.k, b.k) AS m
FROM os_assets a, os_assets b;

-- The frontier: sweep the holding limit (1–10) and the per-asset cap.
CREATE OR REPLACE TABLE os_portfolio AS
WITH l AS (SELECT list(ret ORDER BY k) AS r, (SELECT m FROM os_cov) AS m FROM os_assets)
SELECT h AS max_holdings, cap, a.algorithm,
       round(100 * a.res.expected_return, 2) AS return_pct, round(100 * a.res.volatility, 2) AS volatility_pct,
       round(a.res.sharpe, 3) AS sharpe, a.res.weights AS weights
FROM l, range(1, 11) t(h), (VALUES (0.25), (0.4), (1.0)) c(cap),
     LATERAL (VALUES ('best of', opt_portfolio_best_of(l.r, l.m, h, cap)),
                     ('top return (naive)', opt_portfolio_top_return(l.r, l.m, h, cap))) a(algorithm, res)
WHERE h * cap >= 1.0;

-- Weights of the best-Sharpe portfolio found.
CREATE OR REPLACE TABLE os_portfolio_weights AS
WITH best AS (SELECT weights FROM os_portfolio WHERE algorithm = 'best of' ORDER BY sharpe DESC, max_holdings LIMIT 1)
SELECT a.name AS asset, round(100 * best.weights[a.k + 1], 1) AS weight_pct
FROM os_assets a, best WHERE best.weights[a.k + 1] > 0.0005;
