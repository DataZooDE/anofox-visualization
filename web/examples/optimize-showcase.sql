-- Optimize showcase — anofox_optimize on one Friday at a distribution centre.
--
-- STORED RESULTS, NOT LIVE. The solver outputs below were computed natively
-- with the anofox_optimize extension (examples/optimize-showcase/compute.sql,
-- written to web/data/optimize/*.csv by scripts/gen-showcase-data.sh). The
-- browser demo can't compute them live yet: the extension's DuckDB-Wasm build
-- doesn't load (see examples/optimize-showcase/README.md), so this dashboard
-- only reads and plots the stored tables.
--
-- A made-up regional distribution centre plans one Friday: which containers
-- to book, which shipments to give the cheap carrier, how to cut pick waves,
-- in what order to run the packing line and the paint line — and, in the
-- back office, where to park the reserve fund. Each question is one
-- anofox_optimize family; every panel puts "what we do today" (the family's
-- weak baseline) next to the optimiser.
--
-- Tabs: 1 Overview · 2 Containers · 3 Carrier · 4 Pick waves · 5 Lines ·
-- 6 Reserve fund. Natively, run from web/:  dashboard --check ../examples/optimize-showcase/showcase.sql

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
CREATE OR REPLACE TABLE os_portfolio AS FROM read_csv('data/optimize/os_portfolio.csv');
CREATE OR REPLACE TABLE os_portfolio_weights AS FROM read_csv('data/optimize/os_portfolio_weights.csv');
CREATE OR REPLACE TABLE os_provenance AS FROM read_csv('data/optimize/provenance.csv');

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'One Friday at a distribution centre: six decisions, optimised in SQL with anofox_optimize (stored results)'::LABEL;

SELECT 12::COL;
SELECT ('**Stored results, computed natively.** Every number on this page was computed with the **anofox_optimize** DuckDB extension (version `' || (SELECT extension_version FROM os_provenance) || '`) on a native DuckDB ' || (SELECT duckdb_version FROM os_provenance) || ' by `examples/optimize-showcase/compute.sql`; the demo reads the stored tables from `web/data/optimize/`. They are **not** recomputed in your browser: the extension''s DuckDB-Wasm build does not load in DuckDB-Wasm yet. The inputs are synthetic and deterministic. Each solver is one SQL call over a column aggregated with `list()`, e.g. `SELECT opt_pack_best_of(list(size ORDER BY carton), 1000.0) FROM cartons`.')::MARKDOWN, 'About this dashboard'::TITLE;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT '1 Overview'::TAB;

SELECT 12::COL;
SELECT '**Question:** how much does a DC leave on the table when it plans with today''s rules of thumb? Six decisions, six anofox_optimize families: **pack** cartons into containers, **knapsack** shipments onto a cheap carrier, batch orders into pick **waves**, **schedule** the packing line against due dates, **sequence** the paint line to cut changeovers and pick a **portfolio** for the reserve fund. **What to look for:** every bar pair compares the baseline (the family''s deliberately weak member — "what we do today") with the optimiser; lower is better except for the carrier saving. Two functions return a **proven optimum** (`knapsack_exact`, `schedule_exact`); the rest are heuristics, which is why the `opt_…_best_of` members run every algorithm of the family and keep the winner.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 'Today''s rule vs the optimiser'::GROUP;
SELECT ((SELECT bins_used FROM os_pack_algos WHERE algorithm = 'next fit (today)') - (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'best of'))::METRIC,
       ('containers saved (of ' || (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'next fit (today)') || ')')::LABEL;
SELECT ((SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE 'exact%')
     - (SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE '%(today)'))::METRIC,
       '€ more carrier savings at a 2 t booking'::LABEL;
SELECT round(100 * (1 - (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm = 'best of')
                      / (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm LIKE '%(today)')))::PERCENT,
       'earlier pick of priority orders'::LABEL;
SELECT round(100 * (1 - (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE 'exact%')
                      / (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE '%(today)')))::PERCENT,
       'less weighted lateness on the packing line'::LABEL;
SELECT ((SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)')
     - (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of'))::METRIC,
       'paint-line changeover minutes saved'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT decision ::XAXIS, rule ::CATEGORY, value ::BARCHART, 'percent'::YFORMAT,
       'Optimiser vs today''s rule, today = 100 % — every family beats its baseline; paint changeovers drop by four fifths'::TITLE
FROM (
    SELECT 'containers' AS decision, 'today' AS rule, 100.0 AS value
    UNION ALL SELECT 'containers', 'optimised', round(100.0 * (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'best of') / (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'next fit (today)'), 1)
    UNION ALL SELECT 'carrier saving', 'today', 100.0
    UNION ALL SELECT 'carrier saving', 'optimised', round(100.0 * (SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE 'exact%') / (SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE '%(today)'), 1)
    UNION ALL SELECT 'mean pick wave', 'today', 100.0
    UNION ALL SELECT 'mean pick wave', 'optimised', round(100.0 * (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm = 'best of') / (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm LIKE '%(today)'), 1)
    UNION ALL SELECT 'weighted lateness', 'today', 100.0
    UNION ALL SELECT 'weighted lateness', 'optimised', round(100.0 * (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE 'exact%') / (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE '%(today)'), 1)
    UNION ALL SELECT 'paint changeovers', 'today', 100.0
    UNION ALL SELECT 'paint changeovers', 'optimised', round(100.0 * (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of') / (SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)'), 1)
) ORDER BY decision, rule DESC;

-- ── 2 · Containers (pack_*) ──────────────────────────────────────────────────
SELECT '2 Containers'::TAB;

SELECT 12::COL;
SELECT ('**Decision:** 150 cartons (60–580 volume units) go out in 1,000-unit containers — how few containers will do? `opt_pack_…(sizes, capacity)` returns the number of bins and each carton''s bin. **What to look for:** **next fit** — close the container as soon as a carton doesn''t fit, today''s practice at the dock — needs **' || (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'next fit (today)') || '** containers. Every *decreasing* packer needs **' || (SELECT bins_used FROM os_pack_algos WHERE algorithm = 'best of') || '**, which equals the volume lower bound ⌈total volume / 1,000⌉ — so here the heuristics are **provably optimal**. The fill chart shows why: next fit leaves a ragged tail of half-empty containers, the optimised plan fills 34 of its 39 containers to 95 % or more.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 5::COL;
SELECT 380::HEIGHT;
SELECT algorithm ::XAXIS, bins_used ::BARCHART, lower_bound ::REFLINE, 1 ::FLIP,
       'Containers used — next fit needs 7 more than the bound (dashed); every decreasing packer hits it'::TITLE
FROM os_pack_algos ORDER BY 2 DESC;

SELECT 7::COL;
SELECT 380::HEIGHT;
SELECT container ::XAXIS, fill_pct ::BARCHART, algorithm ::FACET, 1 ::FACET_NCOL, 'percent'::YFORMAT,
       'Fill per container — the optimised plan is nearly full; next fit leaves gaps everywhere'::TITLE
FROM os_pack_bins ORDER BY algorithm, container;

-- ── 3 · Carrier (knapsack_*) ─────────────────────────────────────────────────
SELECT '3 Carrier'::TAB;

SELECT 12::COL;
SELECT ('**Decision:** a cheaper carrier takes a limited weight; each of 60 shipments saves a different amount if it rides with them. Which shipments to hand over? That is a 0/1 knapsack: `opt_knapsack_…(values, weights, capacity)`. **What to look for:** today''s rule ("give them the biggest savers first") falls far short of the **proven optimum** (`opt_knapsack_exact`) at every booking size — at the **2,000 kg** booking it captures **' || (SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE '%(today)')::INTEGER || ' €** of **' || (SELECT saving_eur FROM os_knap_curve WHERE capacity = 2000 AND algorithm LIKE 'exact%')::INTEGER || ' €** possible, because the big savers are also the heaviest. Ranking by **saving per kg** gets within a few euros of the optimum here, but carries no guarantee. The scatter shows the 2,000 kg plan: the optimum takes many light, mid-value shipments.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 7::COL;
SELECT 400::HEIGHT;
SELECT capacity ::XAXIS, algorithm ::CATEGORY, saving_eur ::LINECHART, '€'::YFORMAT, 2000 ::XLINE,
       'Saving vs booked capacity — the exact optimum dominates; today''s rule loses 35 % at 2 t'::TITLE
FROM os_knap_curve ORDER BY capacity, algorithm;

SELECT 5::COL;
SELECT 400::HEIGHT;
SELECT weight_kg ::XAXIS, saving_eur ::SCATTER,
       CASE WHEN exact_loads AND greedy_loads THEN 'both'
            WHEN exact_loads THEN 'optimum only'
            WHEN greedy_loads THEN 'today only'
            ELSE 'neither' END ::CATEGORY, '€'::YFORMAT,
       'The 2 t plan — the optimum swaps heavy savers for many light ones'::TITLE
FROM os_knap_pick ORDER BY shipment;

-- ── 4 · Pick waves (wave_*) ──────────────────────────────────────────────────
SELECT '4 Pick waves'::TAB;

SELECT 12::COL;
SELECT ('**Decision:** 48 orders (8–48 order lines each; **express**, **priority** or **standard**) are picked in waves of at most 120 lines. Which order goes into which wave? `opt_wave_…(lines, priorities, capacity)` returns a 1-based wave per order and the **priority-weighted mean wave** (lower = urgent orders leave earlier). **What to look for:** in **arrival order** express orders are scattered from the first to the last of twelve waves (weighted mean wave **' || (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm LIKE '%(today)') || '**). The optimiser front-loads them into the first waves (**' || (SELECT avg_weighted_wave FROM os_wave_algos WHERE algorithm = 'best of') || '**) at the price of one extra wave at the end — and "fewest waves" alone is the *worst* rule for urgency.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 4::COL;
SELECT 400::HEIGHT;
SELECT algorithm ::XAXIS, avg_weighted_wave ::BARCHART, 1 ::FLIP,
       'Priority-weighted mean wave — lower is better'::TITLE
FROM os_wave_algos ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 400::HEIGHT;
SELECT wave ::XAXIS, class ::CATEGORY, lines ::BARCHART_STACKED,
       'Arrival order (today) — express orders spread from wave 1 to 12'::TITLE
FROM os_wave_load WHERE algorithm = 'arrival order (today)' ORDER BY wave, class;

SELECT 4::COL;
SELECT 400::HEIGHT;
SELECT wave ::XAXIS, class ::CATEGORY, lines ::BARCHART_STACKED,
       'Optimised (best of) — express and priority orders in waves 1–6'::TITLE
FROM os_wave_load WHERE algorithm = 'best of' ORDER BY wave, class;

-- ── 5 · Lines (schedule_* and sequence_*) ────────────────────────────────────
SELECT '5 Lines'::TAB;

SELECT 12::COL;
SELECT ('**Decisions:** (1) the packing line runs 12 jobs from four product families; switching family costs a 15-minute changeover, and every job has a due time and a weight (3 = key account). `opt_schedule_…(processing, due, weights, setup, n)` orders the jobs. (2) The paint line runs ten colours; going light → dark is a quick rinse, dark → light a full clean. `opt_sequence_…(setup, n)` orders them. **What to look for:** **earliest due date** — today''s rule — ignores changeovers and weights; the **proven optimum** (`opt_schedule_exact`, ≤ 14 jobs) cuts weighted lateness from **' || (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE '%(today)')::INTEGER || '** to **' || (SELECT weighted_tardiness FROM os_sched_algos WHERE algorithm LIKE 'exact%')::INTEGER || '** job-minutes and finishes 13 minutes earlier with one 15-minute family changeover fewer. On the paint line the order book needs **' || (SELECT changeover_min FROM os_seq_algos WHERE algorithm LIKE '%(today)')::INTEGER || '** minutes of changeovers, the light-to-dark sequence **' || (SELECT changeover_min FROM os_seq_algos WHERE algorithm = 'best of')::INTEGER || '**.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT ((start_min + end_min) / 2) ::XAXIS, pos ::SCATTER, start_min ::XMIN, end_min ::XMAX,
       CASE WHEN late_min > 0 THEN 'late job' ELSE 'job on time' END ::CATEGORY,
       algorithm ::FACET, 1 ::FACET_NCOL, 'reverse'::YSCALE,
       'Packing-line plan (bar = job run, minutes after 06:00; row = position) — 7 late jobs instead of 9 under the optimum'::TITLE
FROM os_sched_gantt ORDER BY algorithm, pos;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT algorithm ::XAXIS, weighted_tardiness ::BARCHART, 1 ::FLIP,
       'Weighted lateness by rule (job-minutes) — exact = proven optimum'::TITLE
FROM os_sched_algos ORDER BY 2 DESC;

SELECT 8::COL;
SELECT 'Paint line — changeover minutes per sequence'::TITLE,
       algorithm AS "Rule" ::TABLE, changeover_min AS "Changeover (min)" ::COLORSCALE, sequence AS "Colour order" ::PLAIN
FROM os_seq_algos ORDER BY ord;

-- ── 6 · Reserve fund (portfolio_*) ───────────────────────────────────────────
SELECT '6 Reserve fund'::TAB;

SELECT 12::COL;
SELECT ('**Decision:** the back office parks a reserve fund in at most *k* of ten asset classes, with at most *cap* of the fund in any one. `opt_portfolio_…(expected_returns, covariance, max_holdings, cap)` returns the weights, expected (excess) return, volatility and Sharpe ratio. Each dot is one *(k, cap)* setting. **What to look for:** the naive rule — fill the cap with the highest-return assets — is riskier than the optimiser''s portfolio for every setting (dots further **right**). The optimiser''s portfolios sit **up and to the left**: per unit of risk they earn more (best Sharpe **' || (SELECT max(sharpe) FROM os_portfolio WHERE algorithm = 'best of') || '** vs **' || (SELECT max(sharpe) FROM os_portfolio WHERE algorithm LIKE 'top return%') || '**), because they mix bonds, gold and private credit whose risks don''t move together. These are heuristics: no optimum is guaranteed.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 8::COL;
SELECT 420::HEIGHT;
SELECT volatility_pct ::XAXIS, return_pct ::SCATTER, algorithm ::CATEGORY,
       'percent'::XFORMAT, 'percent'::YFORMAT,
       'Return vs risk per (holdings, cap) setting — the optimiser earns more per unit of risk'::TITLE
FROM os_portfolio ORDER BY algorithm, max_holdings, cap;

SELECT 4::COL;
SELECT 420::HEIGHT;
SELECT asset ::XAXIS, weight_pct ::BARCHART, 1 ::FLIP, 'percent'::YFORMAT,
       'The best-Sharpe portfolio — eight asset classes, none above 25 %'::TITLE
FROM os_portfolio_weights ORDER BY 2 DESC;
