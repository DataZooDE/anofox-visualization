-- @title How does the model reason from our inspections?
-- @group How it works
-- @tags  diagram, factor graph, bayesian
-- @description Two Bayesian models drawn as factor graphs: what the defect rate is after four inspections, and which state a machine was in.
--
-- How does the model reason from our inspections? Two models as an
-- inference library writes them (nodes, edges and posterior marginals),
-- drawn with anofox_plot_factor_graph (#1151):
--   1. the defect rate θ: a Beta prior and four inspections (defective or
--      not). θ meets every inspection at its equality node; hover it for
--      what the model believes now (mean ± sd);
--   2. a machine's hidden state over five shifts: the states form one row,
--      each shift's reading drops below its state, and the transition and
--      reading matrices (learned) sit in the parameter band above.
-- Step / Play walk through the messages the inference passed, in order:
-- each lights its edge, moving the message's way (schedule :=, #1152).
-- Factors are boxes, variables circles (filled when observed), equality
-- nodes small squares; edges carry the factor's interface names.
--
-- The tables are stored in data/factor-graphs/ (CSV, as the library wrote
-- them).

CREATE OR REPLACE TABLE fg_coin_nodes AS FROM read_csv('data/factor-graphs/coin-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_edges AS FROM read_csv('data/factor-graphs/coin-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_marginals AS FROM read_csv('data/factor-graphs/coin-marginals.csv');
CREATE OR REPLACE TABLE fg_hmm_nodes AS FROM read_csv('data/factor-graphs/hmm-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_hmm_edges AS FROM read_csv('data/factor-graphs/hmm-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_schedule AS FROM read_csv('data/factor-graphs/coin-schedule.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_hmm_schedule AS FROM read_csv('data/factor-graphs/hmm-schedule.csv', all_varchar = true);

SELECT 'The defect rate after four inspections'::SECTION;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_coin_edges e),
         marginals := (SELECT list(m) FROM fg_coin_marginals m),
         schedule := (SELECT list(s) FROM fg_coin_schedule s))
       AS "What is the defect rate?"
FROM fg_coin_nodes n;

SELECT 'A machine''s hidden state over five shifts'::SECTION;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_hmm_edges e),
         schedule := (SELECT list(s) FROM fg_hmm_schedule s),
         width := 1400, height := 380)
       AS "Which state was the machine in?"
FROM fg_hmm_nodes n;
