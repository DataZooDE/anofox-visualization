-- @title How does the model reason from our inspections?
-- @group How it works
-- @tags  diagram, factor graph, bayesian
-- @description Three Bayesian models as factor graphs, each with the problem it solves: a defect share, a cycle time, when a machine's condition changed.
--
-- How does the model reason from our inspections? Three models as an
-- inference library writes them (nodes, edges and posterior marginals),
-- drawn with anofox_plot_factor_graph (#1151):
--   1. the defect rate θ: a Beta prior and four inspections (defective or
--      not). θ meets every inspection at its equality node; hover it for
--      what the model believes now (mean ± sd);
--   2. a machine's cycle time, its mean m and precision τ learned together
--      (mean-field): the estimates settle over six iterations — hover m or
--      τ for its posterior mean per iteration, click it for its prior and
--      posterior beside the graph and its mean per iteration under it;
--   3. a machine's hidden state over five shifts: the states form one row,
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
CREATE OR REPLACE TABLE fg_cycle_nodes AS FROM read_csv('data/factor-graphs/cycle-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_cycle_edges AS FROM read_csv('data/factor-graphs/cycle-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_cycle_marginals AS FROM read_csv('data/factor-graphs/cycle-marginals.csv');
CREATE OR REPLACE TABLE fg_cycle_schedule AS FROM read_csv('data/factor-graphs/cycle-schedule.csv', all_varchar = true);

SELECT 'The defect rate after four inspections'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** Customers sent back units of a new product. Before we claim against the supplier we need to know what share of returned units has a real defect (not "no fault found") — and how sure we can be. **What goes in:** what earlier products taught us, about 40 % but loosely (the Beta(2, 3) prior), and four returns checked in the lab, three of them defective. **What comes out:** the defect share θ, 56 % ± 16 %. Four checks moved it but left it wide: check more returns before claiming.'::MARKDOWN,
       'What this graph solves'::TITLE;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_coin_edges e),
         marginals := (SELECT list(m) FROM fg_coin_marginals m),
         schedule := (SELECT list(s) FROM fg_coin_schedule s))
       AS "What is the defect rate?"
FROM fg_coin_nodes n;

SELECT 'A machine''s cycle time: mean and spread, learned together'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** To plan a shift we need to know how long one cycle on a machine takes and how much it varies: the mean sets the output per shift, the spread the buffer to plan in and the limit at which a slow cycle should raise an alarm. **What goes in:** six timed cycles (minutes) and next to no prior knowledge (m ~ N(0, 100), τ ~ Γ(1, 1)). **What comes out:** a cycle takes 4.03 ± 0.24 min on average; τ is the precision, 2.85 means one cycle varies by about ± 0.6 min (1 / √τ). The mean and the spread depend on each other, so the messages go round six times until both settle.'::MARKDOWN,
       'What this graph solves'::TITLE;

-- The priors as the model states them: m ~ N(0, variance 100), τ ~ Γ(shape 1, rate 1).
CREATE OR REPLACE TABLE fg_cycle_priors AS
  FROM (VALUES ('v6', 'Normal', 0.0, 10.0), ('v9', 'Gamma', 1.0, 1.0)) p(variable, family, mean, sd);
-- The variable a click on the graph picks (its equality node e6 / e9), else m.
CREATE OR REPLACE MACRO fg_pick() AS coalesce(
  (SELECT e.variable FROM fg_cycle_edges e JOIN fg_cycle_nodes n ON n.id = e.variable
   WHERE e.factor = getvariable('fg_node') AND n.kind = 'variable' LIMIT 1),
  (SELECT id FROM fg_cycle_nodes WHERE id = getvariable('fg_node') AND kind = 'variable'),
  'v6');
-- A density at x for a family, from its mean and sd (Normal, Gamma).
CREATE OR REPLACE MACRO fg_density(family, x, mu, sd) AS CASE family
  WHEN 'Gamma' THEN CASE WHEN x > 0 THEN exp(((mu / sd) ^ 2 - 1) * ln(x) - x * mu / sd ^ 2
                                         + (mu / sd) ^ 2 * ln(mu / sd ^ 2) - lgamma((mu / sd) ^ 2)) ELSE 0 END
  ELSE exp(-0.5 * ((x - mu) / sd) ^ 2) / (sd * sqrt(2 * pi())) END;

SELECT 8::SPAN;
SELECT 420::HEIGHT;
SELECT 'fg_node'::SELECT_AS;
SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_cycle_edges e),
         marginals := (SELECT list(m) FROM fg_cycle_marginals m),
         schedule := (SELECT list(s) FROM fg_cycle_schedule s))
       AS "How long does a cycle take, and how steady is it? (click m or τ)"
FROM fg_cycle_nodes n;

-- Beside it, the clicked variable: what the model believed before the data
-- (its prior) and after (its posterior, the last iteration).
SELECT 4::SPAN;
SELECT 420::HEIGHT;
WITH post AS (SELECT * FROM fg_cycle_marginals WHERE variable = fg_pick() ORDER BY iteration DESC LIMIT 1),
pri AS (SELECT * FROM fg_cycle_priors WHERE variable = fg_pick()),
lab AS (SELECT replace(replace(label, '\tau', 'τ'), '\', '') AS v FROM fg_cycle_nodes WHERE id = fg_pick()),
rng AS (SELECT greatest(CASE WHEN post.family = 'Gamma' THEN 0 ELSE -1e300 END,
                        least(pri.mean - 3 * pri.sd, post.mean - 4 * post.sd), post.mean - 12 * post.sd) AS lo,
               least(greatest(pri.mean + 3 * pri.sd, post.mean + 4 * post.sd), post.mean + 12 * post.sd) AS hi
        FROM post, pri),
xs AS (SELECT lo + (hi - lo) * i / 120.0 AS x FROM rng, range(0, 121) r(i))
SELECT round(x, 4)::XAXIS, k::CATEGORY, round(d, 5)::LINECHART,
       ('What we believe about ' || (SELECT v FROM lab) || ': before and after the data')::TITLE,
       'The prior is what the model assumed before any cycle was measured; the posterior what it believes after the six. The narrower and taller the curve, the surer the model.'::INFO
FROM (SELECT x, 'before (prior)' AS k, fg_density(pri.family, x, pri.mean, pri.sd) AS d FROM xs, pri
      UNION ALL
      SELECT x, 'after six cycles (posterior)', fg_density(post.family, x, post.mean, post.sd) FROM xs, post)
ORDER BY k, x;

-- Under them, how it settled: the mean ± sd per iteration.
SELECT 12::SPAN;
SELECT 220::HEIGHT;
SELECT iteration::XAXIS, round(mean, 4)::LINECHART,
       round(mean - sd, 4)::BAND_LOWER, round(mean + sd, 4)::BAND_UPPER,
       ((SELECT replace(replace(label, '\tau', 'τ'), '\', '') FROM fg_cycle_nodes WHERE id = fg_pick())
        || ' by iteration: its mean and ± 1 sd as the messages go round')::TITLE
FROM fg_cycle_marginals WHERE variable = fg_pick() ORDER BY iteration;

SELECT 'A machine''s hidden state over five shifts'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** A machine wears without telling us: per shift we only see a coarse reading (one of three levels), not its condition. Maintenance wants to know in which shift the condition changed, to service the machine before it fails. **What goes in:** five shift readings; the model learns how conditions follow one another (the transition matrix A) and what each condition tends to read (B). **What comes out:** the condition per shift — here the model is at least 94 % sure of state 0 in shifts 1–2, state 1 in shift 3 and state 2 in shifts 4–5: a change step by step, the cue to plan the service. The model only numbers the states; calling them good, worn and faulty is our reading.'::MARKDOWN,
       'What this graph solves'::TITLE;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_hmm_edges e),
         schedule := (SELECT list(s) FROM fg_hmm_schedule s),
         width := 1400, height := 380)
       AS "Which state was the machine in?"
FROM fg_hmm_nodes n;
