-- @title How do we plan a busy day at the distribution centre?
-- @group Logistics & production
-- @tags  optimize, warehouse
-- @description One Friday at a distribution centre: containers, carriers, picking waves and packing lines, each planned by a solver.
--
-- Optimize showcase — anofox_optimize on one Friday at a distribution centre.
--
-- STORED RESULTS, NOT LIVE. The solver outputs below were computed natively
-- with the anofox_optimize extension (examples/optimize-showcase/compute.sql,
-- written to web/data/optimize/*.csv by scripts/gen-showcase-data.sh). The
-- browser demo can't compute them live yet: the extension's DuckDB-Wasm build
-- doesn't load (see examples/optimize-showcase/README.md), so this dashboard
-- only reads and plots the stored tables. The planning inputs pick among
-- stored parameter sweeps (container sizes, bookings, wave sizes, schedule
-- rules, portfolio limits); nothing is re-solved in the browser.
--
-- A made-up regional distribution centre plans one Friday: which containers
-- to book, which shipments to give the cheap carrier, how to cut pick waves,
-- in what order to run the packing line and the paint line — and, in the
-- back office, where to park the reserve fund. Each question is one
-- anofox_optimize family; every panel puts "what we do today" (the family's
-- weak baseline) next to the optimiser.
--
-- Tabs: Overview · Containers · Carrier · Pick waves · Lines (packing, paint) ·
-- Reserve fund. Natively, run from web/:  dashboard --check ../examples/optimize-showcase/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_optimize'::BRAND;
SELECT 'One Friday at a distribution centre: six decisions, optimised in SQL'::DASHBOARD_TITLE;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data: the stored anofox_optimize results (CSV, a few KB each)
-- ─────────────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE os_pack_algos AS FROM read_csv('data/optimize/os_pack_algos.csv');
CREATE OR REPLACE TABLE os_pack_bins AS FROM read_csv('data/optimize/os_pack_bins.csv');
CREATE OR REPLACE TABLE os_knap_curve AS FROM read_csv('data/optimize/os_knap_curve.csv');
CREATE OR REPLACE TABLE os_knap_pick AS FROM read_csv('data/optimize/os_knap_pick.csv');
CREATE OR REPLACE TABLE os_wave_algos AS FROM read_csv('data/optimize/os_wave_algos.csv');
CREATE OR REPLACE TABLE os_wave_load AS FROM read_csv('data/optimize/os_wave_load.csv');
CREATE OR REPLACE TABLE os_sched_algos AS FROM read_csv('data/optimize/os_sched_algos.csv');
CREATE OR REPLACE TABLE os_sched_gantt AS FROM read_csv('data/optimize/os_sched_gantt.csv');
CREATE OR REPLACE TABLE os_seq_algos AS FROM read_csv('data/optimize/os_seq_algos.csv');
CREATE OR REPLACE TABLE os_seq_matrix AS FROM read_csv('data/optimize/os_seq_matrix.csv');
CREATE OR REPLACE TABLE os_seq_steps AS FROM read_csv('data/optimize/os_seq_steps.csv');
CREATE OR REPLACE TABLE os_portfolio AS FROM read_csv('data/optimize/os_portfolio.csv');
CREATE OR REPLACE TABLE os_portfolio_alloc AS FROM read_csv('data/optimize/os_portfolio_alloc.csv');
CREATE OR REPLACE TABLE os_provenance AS FROM read_csv('data/optimize/provenance.csv');

-- The planning inputs (strings from the controls → typed values; each snaps to
-- a stored sweep point).
CREATE OR REPLACE MACRO os_cap() AS coalesce(TRY_CAST(getvariable('container_size') AS INTEGER), 1000);
CREATE OR REPLACE MACRO os_book() AS
    least(5000, greatest(250, round(coalesce(TRY_CAST(getvariable('booking_kg') AS DOUBLE), 2000) / 250) * 250))::INTEGER;
CREATE OR REPLACE MACRO os_wave() AS coalesce(TRY_CAST(getvariable('wave_size') AS INTEGER), 120);
CREATE OR REPLACE MACRO os_rule() AS coalesce(getvariable('schedule_rule'), 'exact (proven optimum)');
CREATE OR REPLACE MACRO os_pcap() AS
    CASE coalesce(getvariable('position_cap'), '25 %') WHEN '40 %' THEN 0.4 WHEN 'no cap' THEN 1.0 ELSE 0.25 END;
CREATE OR REPLACE MACRO os_hold() AS
    greatest(coalesce(TRY_CAST(getvariable('holdings') AS INTEGER), 10), ceil(1 / os_pcap())::INTEGER);
CREATE OR REPLACE MACRO os_shift() AS 480;  -- paint-line minutes in an 8-hour shift

-- ── Header: planning inputs, KPIs ────────────────────────────────────────────

SELECT 'Planning inputs'::GROUP;
SELECT container_size ::BUTTONS, 'Container size (units)'::LABEL FROM (VALUES (1, '1000'), (2, '800'), (3, '1250'), (4, '1500')) t(k, container_size) ORDER BY k;
SELECT 2000 AS booking_kg ::NUMBER, 'Carrier booking (kg, 250–5,000)'::LABEL;
SELECT wave_size ::BUTTONS, 'Wave size (order lines)'::LABEL FROM (VALUES (1, '120'), (2, '80'), (3, '160'), (4, '200')) t(k, wave_size) ORDER BY k;
SELECT algorithm AS schedule_rule ::DROPDOWN, 'Packing-line rule'::LABEL FROM os_sched_algos WHERE algorithm NOT LIKE '%(today)' ORDER BY ord DESC;
SELECT holdings ::DROPDOWN, 'Max. holdings'::LABEL FROM (SELECT DISTINCT max_holdings::VARCHAR AS holdings, max_holdings FROM os_portfolio) ORDER BY max_holdings DESC;
SELECT position_cap ::BUTTONS, 'Cap per asset'::LABEL FROM (VALUES (1, '25 %'), (2, '40 %'), (3, 'no cap')) t(k, position_cap) ORDER BY k;
SELECT 1::ENDGROUP;

SELECT 'Optimiser vs. today''s rule'::GROUP;
SELECT (SELECT avg_fill_pct FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'best of')::PERCENT,
       (SELECT avg_fill_pct FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'next fit (today)')::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Container fill'::LABEL;
SELECT (SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE 'exact%')::METRIC,
       (SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE '%(today)')::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Carrier saving (€)'::LABEL;
SELECT round(100 * sum(lines) FILTER (WHERE algorithm = 'best of' AND wave <= 3) / sum(lines) FILTER (WHERE algorithm = 'best of'), 1)::PERCENT,
       round(100 * sum(lines) FILTER (WHERE algorithm LIKE '%(today)' AND wave <= 3) / sum(lines) FILTER (WHERE algorithm LIKE '%(today)'), 1)::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Express lines in waves 1–3'::LABEL
FROM os_wave_load WHERE capacity = os_wave() AND class = 'express';
SELECT count(*) FILTER (WHERE algorithm = os_rule() AND late_min = 0)::METRIC,
       count(*) FILTER (WHERE algorithm LIKE '%(today)' AND late_min = 0)::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Packing jobs on time (of 12)'::LABEL
FROM os_sched_gantt;
SELECT (os_shift() - (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of'))::METRIC,
       (os_shift() - (SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)'))::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Paint minutes left per shift'::LABEL;
SELECT (SELECT sharpe FROM os_portfolio WHERE max_holdings = os_hold() AND cap = os_pcap() AND algorithm = 'best of')::METRIC,
       (SELECT sharpe FROM os_portfolio WHERE max_holdings = os_hold() AND cap = os_pcap() AND algorithm LIKE 'top return%')::DELTA, 'vs today''s rule'::DELTA_LABEL,
       'Reserve-fund Sharpe ratio'::LABEL;
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 7::COL;
SELECT 380::HEIGHT;
WITH g AS (
    SELECT 'fewer containers' AS gain,
           100 * (1 - (SELECT bins_used FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'best of')
                    / (SELECT bins_used FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'next fit (today)')) AS pct
    UNION ALL SELECT 'more carrier saving',
           100 * ((SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE 'exact%')
                / (SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE '%(today)') - 1)
    UNION ALL SELECT 'earlier priority picking',
           100 * (1 - (SELECT avg_weighted_wave FROM os_wave_algos WHERE capacity = os_wave() AND algorithm = 'best of')
                    / (SELECT avg_weighted_wave FROM os_wave_algos WHERE capacity = os_wave() AND algorithm LIKE '%(today)'))
    UNION ALL SELECT 'less weighted lateness',
           100 * (1 - (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm = os_rule())
                    / (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE '%(today)'))
    UNION ALL SELECT 'less paint changeover',
           100 * (1 - (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of')
                    / (SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)'))
    UNION ALL SELECT 'higher Sharpe ratio',
           100 * ((SELECT sharpe FROM os_portfolio WHERE max_holdings = os_hold() AND cap = os_pcap() AND algorithm = 'best of')
                / (SELECT sharpe FROM os_portfolio WHERE max_holdings = os_hold() AND cap = os_pcap() AND algorithm LIKE 'top return%') - 1))
SELECT gain ::XAXIS, round(pct, 1) ::BARCHART, 1 ::FLIP, 'percent'::YFORMAT, 'tween'::TRANSITION,
       'Gain over today''s rule at the chosen inputs — paint changeovers and the carrier gain most'::TITLE
FROM g ORDER BY pct;

SELECT 5::COL;
SELECT 'Six decisions, six solver families'::TITLE,
       decision AS "Decision" ::TABLE, fn AS "Functions", today AS "Today's rule", guarantee AS "Optimiser" ::BADGE
FROM (VALUES
    (1, 'Containers', 'opt_pack_…', 'next fit', 'ok: meets the lower bound'),
    (2, 'Carrier', 'opt_knapsack_…', 'biggest saving first', 'ok: proven optimum'),
    (3, 'Pick waves', 'opt_wave_…', 'arrival order', 'heuristic (best of 4)'),
    (4, 'Packing line', 'opt_schedule_…', 'earliest due date', 'ok: proven optimum'),
    (5, 'Paint line', 'opt_sequence_…', 'order book', 'heuristic (best of 3)'),
    (6, 'Reserve fund', 'opt_portfolio_…', 'top expected return', 'heuristic')
) t(k, decision, fn, today, guarantee) ORDER BY k;

SELECT 12::COL;
SELECT '**What to look for:** change the planning inputs above — every KPI, chart and table re-reads the stored sweep for that setting. Each solver is one SQL call over a `list()` column, for example `opt_pack_best_of(list(size ORDER BY carton), 1000.0)`.'::MARKDOWN;

-- ── 2 · Containers (pack_*) ──────────────────────────────────────────────────
SELECT 'Containers'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** 150 cartons into **' || os_cap() || '-unit** containers. Today''s next-fit rule closes a container as soon as a carton doesn''t fit; every *decreasing* packer reaches the volume lower bound — so here the heuristics are provably optimal.')::MARKDOWN;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT (SELECT avg_fill_pct FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'best of')::GAUGE, '50,100'::RANGE,
       '<70:bad; <90:warn; good'::THRESHOLDS, 'Average fill, optimised'::TITLE;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT (SELECT avg_fill_pct FROM os_pack_algos WHERE capacity = os_cap() AND algorithm = 'next fit (today)')::GAUGE, '50,100'::RANGE,
       '<70:bad; <90:warn; good'::THRESHOLDS, 'Average fill, today (next fit)'::TITLE;

SELECT 4::COL;
SELECT 300::HEIGHT;
SELECT algorithm ::XAXIS, bins_used ::BARCHART, lower_bound ::REFLINE, 1 ::FLIP,
       'Containers used — dashed: the volume lower bound'::TITLE
FROM os_pack_algos WHERE capacity = os_cap() ORDER BY 2 DESC, 1 DESC;

SELECT 8::COL;
SELECT 340::HEIGHT;
SELECT rank ::XAXIS, rule ::CATEGORY, fill_pct ::LINECHART, 'percent'::YFORMAT,
       'Fill profile, fullest container first — next fit leaves a long tail of half-empty containers'::TITLE
FROM (SELECT row_number() OVER (PARTITION BY algorithm ORDER BY fill_pct DESC) AS rank,
             CASE WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'today (next fit)' END AS rule, fill_pct
      FROM os_pack_bins WHERE capacity = os_cap())
ORDER BY rule, rank;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT capacity ::XAXIS, rule ::CATEGORY, n ::LINECHART, os_cap() ::XLINE,
       'Containers needed by size — optimised = the lower bound at every size'::TITLE
FROM (SELECT capacity, CASE WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'today (next fit)' END AS rule, bins_used AS n
      FROM os_pack_algos WHERE algorithm IN ('best of', 'next fit (today)'))
ORDER BY rule, capacity;

-- ── 3 · Carrier (knapsack_*) ─────────────────────────────────────────────────
SELECT 'Carrier'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** at a **' || os_book() || ' kg** booking, today''s "biggest saver first" rule captures **' ||
        (SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE '%(today)')::INTEGER || ' €** of the **' ||
        (SELECT saving_eur FROM os_knap_curve WHERE capacity = os_book() AND algorithm LIKE 'exact%')::INTEGER ||
        ' €** possible (`opt_knapsack_exact`) — the big savers are also the heaviest. Ranking by saving per kg comes close, without a guarantee.')::MARKDOWN;

SELECT 8::COL;
SELECT 360::HEIGHT;
SELECT capacity ::XAXIS, algorithm ::CATEGORY, saving_eur ::LINECHART, os_book() ::XLINE,
       'Saving (€) by booked capacity (kg) — the gap to the optimum never closes'::TITLE
FROM os_knap_curve ORDER BY algorithm, capacity;

SELECT 4::COL;
SELECT 360::HEIGHT;
SELECT algorithm ::XAXIS, saving_eur ::BARCHART, 1 ::FLIP, 'tween'::TRANSITION,
       ('Saving (€) at ' || os_book() || ' kg')::TITLE
FROM os_knap_curve WHERE capacity = os_book() ORDER BY 2;

SELECT 7::COL;
SELECT 380::HEIGHT;
SELECT weight_kg ::XAXIS, saving_eur ::SCATTER,
       CASE WHEN exact_loads AND greedy_loads THEN 'both rules'
            WHEN exact_loads THEN 'optimum only'
            WHEN greedy_loads THEN 'today only'
            ELSE 'stays with the regular carrier' END ::CATEGORY,
       ('Which shipments ride at ' || os_book() || ' kg (x: weight in kg, y: saving in €) — the optimum swaps heavy savers for light ones')::TITLE
FROM os_knap_pick WHERE capacity = os_book() ORDER BY 3, shipment;

SELECT 5::COL;
SELECT 'Where the two plans disagree'::TITLE,
       'S' || lpad(shipment::VARCHAR, 2, '0') AS "Shipment" ::TABLE, weight_kg AS "kg" ::PLAIN, saving_eur AS "Saving €" ::PLAIN, eur_per_kg AS "€ / kg" ::PLAIN,
       CASE WHEN exact_loads THEN 'ok: loads' ELSE 'leaves' END AS "Optimum" ::BADGE,
       CASE WHEN greedy_loads THEN 'ok: loads' ELSE 'leaves' END AS "Today" ::BADGE
FROM os_knap_pick WHERE capacity = os_book() AND exact_loads <> greedy_loads
ORDER BY exact_loads DESC, eur_per_kg DESC;

-- ── 4 · Pick waves (wave_*) ──────────────────────────────────────────────────
SELECT 'Pick waves'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** 48 orders in waves of **' || os_wave() || '** order lines. Lower priority-weighted mean wave = urgent orders leave earlier. The optimiser front-loads express orders at the price of one extra wave at the end; "fewest waves" alone is the worst rule for urgency.')::MARKDOWN;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT algorithm ::XAXIS, avg_weighted_wave ::BARCHART, 1 ::FLIP,
       'Priority-weighted mean wave — lower is better'::TITLE
FROM os_wave_algos WHERE capacity = os_wave() ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT 'W' || lpad(wave::VARCHAR, 2, '0') ::XAXIS, class ::CATEGORY, lines ::BARCHART_STACKED,
       'Today (arrival order) — express lines in every wave'::TITLE
FROM os_wave_load WHERE capacity = os_wave() AND algorithm = 'arrival order (today)' ORDER BY wave, class;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT 'W' || lpad(wave::VARCHAR, 2, '0') ::XAXIS, class ::CATEGORY, lines ::BARCHART_STACKED,
       'Optimised (best of) — express first, standard last'::TITLE
FROM os_wave_load WHERE capacity = os_wave() AND algorithm = 'best of' ORDER BY wave, class;

SELECT 7::COL;
SELECT 320::HEIGHT;
SELECT (capacity || ' lines') ::XAXIS, algorithm ::YAXIS, avg_weighted_wave ::HEATMAP,
       'Mean wave by rule and wave size (darker = later, worse) — the optimiser wins at every size'::TITLE
FROM os_wave_algos ORDER BY capacity, ord DESC;

SELECT 5::COL;
SELECT 320::HEIGHT;
SELECT capacity ::XAXIS, CASE WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'today (arrival order)' END ::CATEGORY,
       waves ::LINECHART, os_wave() ::XLINE, 'Waves needed by wave size — the price is one extra wave'::TITLE
FROM os_wave_algos WHERE algorithm IN ('best of', 'arrival order (today)') ORDER BY 2, 1;

-- ── 5 · Lines (schedule_* and sequence_*) ────────────────────────────────────
SELECT 'Lines'::TAB;

SELECT 'Packing line'::SUBTAB;

SELECT 12::COL;
SELECT ('**What to look for:** 12 jobs, four product families, a 15-minute changeover between families and a weight of 3 for key accounts. Today''s earliest-due-date rule ignores both; **' || os_rule() || '** cuts weighted lateness from **' ||
        (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE '%(today)')::INTEGER || '** to **' ||
        (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm = os_rule())::INTEGER || '** job-minutes. Pick another rule in the inputs.')::MARKDOWN;

SELECT 12::COL;
SELECT 440::HEIGHT;
SELECT ((start_min + end_min) / 2) ::XAXIS, pos ::SCATTER, start_min ::XMIN, end_min ::XMAX,
       CASE WHEN late_min > 0 THEN 'late job' ELSE 'job on time' END ::CATEGORY,
       algorithm ::FACET, 1 ::FACET_NCOL, 'reverse'::YSCALE,
       'Run plan (bar = job, minutes after 06:00; row = position in the sequence)'::TITLE
FROM os_sched_gantt WHERE algorithm LIKE '%(today)' OR algorithm = os_rule() ORDER BY algorithm, pos;

SELECT 5::COL;
SELECT 360::HEIGHT;
SELECT algorithm ::XAXIS, weighted_tardiness ::BARCHART, 1 ::FLIP,
       'Weighted lateness by rule (job-minutes) — exact = proven optimum'::TITLE
FROM os_sched_algos ORDER BY 2 DESC, 1;

SELECT 7::COL;
SELECT 'Job by job: today vs the selected rule'::TITLE,
       t.job_name AS "Job" ::TABLE, t.family AS "Family",
       CASE WHEN t.weight > 1 THEN 'key account' ELSE 'standard' END AS "Customer",
       strftime(TIMESTAMP '2026-10-16 06:00' + INTERVAL (t.due_min::INTEGER) MINUTE, '%H:%M') AS "Due" ::PLAIN,
       t.late_min AS "Late today (min)" ::COLORSCALE,
       s.late_min AS "Late, selected (min)" ::COLORSCALE,
       CASE WHEN s.late_min = 0 THEN 'ok: on time' WHEN s.late_min < t.late_min THEN 'late, but earlier' ELSE 'late' END AS "Selected rule" ::BADGE
FROM os_sched_gantt t JOIN os_sched_gantt s ON s.job_name = t.job_name AND s.algorithm = os_rule()
WHERE t.algorithm LIKE '%(today)'
ORDER BY t.weight DESC, t.due_min;

SELECT 'Paint line'::SUBTAB;

SELECT 12::COL;
SELECT ('**What to look for:** ten colours; light → dark is a quick rinse, dark → light a full clean. The order book needs **' ||
        (SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)')::INTEGER || '** minutes of changeovers, `opt_sequence_best_of` **' ||
        (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of')::INTEGER || '** — it simply paints light to dark.')::MARKDOWN;

SELECT 7::COL;
SELECT 360::HEIGHT;
SELECT pos ::XAXIS, CASE WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'today (order book)' END ::CATEGORY,
       cum_min ::STEP, 'Changeover minutes, cumulative over the ten colours — every dark → light step costs half an hour'::TITLE
FROM os_seq_steps WHERE algorithm IN ('best of', 'order book (today)') ORDER BY 2, 1;

SELECT 5::COL;
SELECT 360::HEIGHT;
SELECT to_colour ::XAXIS, from_colour ::YAXIS, minutes ::HEATMAP,
       'Changeover matrix (minutes, from row to column) — colours from light to dark'::TITLE
FROM os_seq_matrix ORDER BY to_dark, from_dark;

SELECT 12::COL;
SELECT 'The four sequences'::TITLE,
       algorithm AS "Rule" ::TABLE, changeover_min AS "Changeover (min)" ::COLORSCALE, sequence AS "Colour order" ::PLAIN
FROM os_seq_algos ORDER BY ord;

-- ── 6 · Reserve fund (portfolio_*) ───────────────────────────────────────────
SELECT 'Reserve fund'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** at most **' || os_hold() || '** of ten asset classes, at most **' || CASE WHEN os_pcap() = 1 THEN 'no cap' ELSE (100 * os_pcap())::INTEGER || ' %' END ||
        '** in any one. The naive rule fills the cap with the highest-return assets; the optimiser mixes bonds, gold and private credit whose risks don''t move together, and earns more per unit of risk. Heuristics: no optimum is guaranteed.')::MARKDOWN;

SELECT 7::COL;
SELECT 400::HEIGHT;
SELECT volatility_pct ::XAXIS, return_pct ::SCATTER,
       CASE WHEN max_holdings = os_hold() AND cap = os_pcap() THEN '▶ selected setting'
            WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'top return (naive)' END ::CATEGORY,
       'percent'::XFORMAT, 'percent'::YFORMAT,
       'Return vs risk for every (holdings, cap) setting — optimised portfolios sit up and to the left'::TITLE
FROM os_portfolio ORDER BY 3, max_holdings, cap;

SELECT 5::COL;
SELECT 400::HEIGHT;
SELECT asset ::XAXIS, CASE WHEN algorithm = 'best of' THEN 'optimised (best of)' ELSE 'top return (naive)' END ::CATEGORY,
       weight_pct ::BARCHART, 1 ::FLIP, 'percent'::YFORMAT, 'tween'::TRANSITION,
       'Weights at the selected setting'::TITLE
FROM os_portfolio_alloc WHERE max_holdings = os_hold() AND cap = os_pcap() ORDER BY 2, asset DESC;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT max_holdings ::XAXIS, CASE cap WHEN 1 THEN 'no cap' ELSE (100 * cap)::INTEGER || ' % cap' END ::YAXIS, sharpe ::HEATMAP,
       'Sharpe ratio of the optimised portfolio by holdings limit and cap'::TITLE
FROM os_portfolio WHERE algorithm = 'best of' ORDER BY max_holdings, cap;

SELECT 6::COL;
SELECT 'Settings ranked by Sharpe ratio'::TITLE,
       max_holdings || ' holdings' AS "Setting" ::TABLE, CASE cap WHEN 1 THEN 'no cap' ELSE (100 * cap)::INTEGER || ' %' END AS "Cap" ::PLAIN,
       return_pct AS "Return %" ::PLAIN, volatility_pct AS "Volatility %" ::PLAIN, sharpe AS "Sharpe" ::PLAIN,
       round(100 * (sharpe / naive - 1), 1) AS "vs naive %" ::TREND
FROM (SELECT b.*, n.sharpe AS naive FROM os_portfolio b
      JOIN os_portfolio n ON n.max_holdings = b.max_holdings AND n.cap = b.cap AND n.algorithm LIKE 'top return%'
      WHERE b.algorithm = 'best of')
ORDER BY sharpe DESC, max_holdings LIMIT 10;
