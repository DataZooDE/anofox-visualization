-- @title The printer won't print: what do we try first?
-- @group How it works
-- @tags  diagram, bayesian network, troubleshooting, decision, expected cost, evidence
-- @description Decision-theoretic troubleshooting: the cheapest order of tests and repairs, re-planned after every answer, and which answers point to a fault.
--
-- The printer won't print: what do we try first? Decision-theoretic
-- troubleshooting (Heckerman, Breese & Rommelse, 1995) on a small Bayesian
-- network: one fault (toner, driver, cable or a jammed paper path), three
-- cheap tests that hint at it, and a repair for each fault. A repair fixes
-- its own fault; a test costs a little and moves the beliefs.
--
-- The plan is the order of steps with the lowest expected cost: repairs by
-- probability per euro (the most likely fault per euro first), unless a test
-- first is cheaper on average, because its answer lets the repairs after it
-- come in a better order. The waterfall shows where a test's saving comes
-- from. Every answer is a dashboard input: click a test's answer, or "still
-- broken" after a repair, in the network (getvariable('fix_ev')), and the
-- SQL below re-plans from it. The second tab searches the answers that
-- would point most to a fault, or away from it.

SELECT 'bayes-net'::REQUIRES;

-- The faults, how likely each is before any test, and the repair that
-- fixes it (cost in euros).
CREATE OR REPLACE TEMP TABLE fx_faults AS
FROM (VALUES
  ('toner', 0.40, 'Replace the toner', 30),
  ('driver', 0.25, 'Reinstall the driver', 10),
  ('cable', 0.15, 'Swap the cable', 5),
  ('paper', 0.20, 'Clear the paper path', 15)
) t(fault, prior, repair, cost);

-- The tests, their cost, and how each answer reads given the fault.
CREATE OR REPLACE TEMP TABLE fx_tests AS
FROM (VALUES
  ('page', 'Test page', 2, ['prints', 'fails']),
  ('light', 'Toner light', 1, ['on', 'off']),
  ('other', 'Other PC', 5, ['prints', 'fails'])
) t(test, label, cost, answers);
CREATE OR REPLACE TEMP TABLE fx_test_given AS
FROM (VALUES
  ('page', 'toner', 'prints', 0.10), ('page', 'driver', 'prints', 0.95),
  ('page', 'cable', 'prints', 0.95), ('page', 'paper', 'prints', 0.05),
  ('light', 'toner', 'on', 0.85), ('light', 'driver', 'on', 0.05),
  ('light', 'cable', 'on', 0.05), ('light', 'paper', 'on', 0.10),
  ('other', 'toner', 'prints', 0.05), ('other', 'driver', 'prints', 0.90),
  ('other', 'cable', 'prints', 0.05), ('other', 'paper', 'prints', 0.05)
) t(test, fault, answer, p)
UNION ALL
-- the other answer of each test
SELECT g.test, g.fault, t.answers[2], 1 - g.p
FROM (VALUES
  ('page', 'toner', 0.10), ('page', 'driver', 0.95), ('page', 'cable', 0.95), ('page', 'paper', 0.05),
  ('light', 'toner', 0.85), ('light', 'driver', 0.05), ('light', 'cable', 0.05), ('light', 'paper', 0.10),
  ('other', 'toner', 0.05), ('other', 'driver', 0.90), ('other', 'cable', 0.05), ('other', 'paper', 0.05)
) g(test, fault, p)
JOIN fx_tests t USING (test);

-- The network: the fault, a node per test (its answer) and a node per
-- repair (did it fix the printer?), each a child of the fault.
CREATE OR REPLACE TEMP TABLE fx_nodes AS
SELECT 'fault' AS id, 'What is broken' AS label, 'chance' AS kind,
       ['toner', 'driver', 'cable', 'paper'] AS states, 'fault' AS "group"
UNION ALL SELECT test, label, 'chance', answers, 'test' FROM fx_tests
UNION ALL SELECT 'after_' || fault, repair, 'chance', ['fixed', 'still broken'], 'repair' FROM fx_faults;
CREATE OR REPLACE TEMP TABLE fx_arcs AS
SELECT row_number() OVER () AS k, 'fault' AS "from", id AS "to" FROM fx_nodes WHERE id <> 'fault';

-- Each node's table given the fault: a test's answer, whether a repair
-- fixes it (only its own fault's), the fault itself.
CREATE OR REPLACE TEMP TABLE fx_cpt AS
SELECT test AS node, fault, answer AS state, p FROM fx_test_given
UNION ALL
SELECT 'after_' || r.fault, f.fault, s.state,
       CASE WHEN (f.fault = r.fault) = (s.state = 'fixed') THEN 1.0 ELSE 0.0 END
FROM fx_faults r, fx_faults f, (VALUES ('fixed'), ('still broken')) s(state)
UNION ALL
SELECT 'fault', f.fault, s.fault, (f.fault = s.fault)::INT FROM fx_faults f, fx_faults s;
CREATE OR REPLACE TEMP TABLE fx_tables AS
SELECT 'fault' AS node, []::VARCHAR[] AS parent_states, fault AS state, prior AS probability FROM fx_faults
UNION ALL
SELECT node, [fault], state, p FROM fx_cpt WHERE node <> 'fault';

-- The answers so far, as the clicks left them ('now') and before the last
-- click ('before'); 'prior' has none.
CREATE OR REPLACE TEMP VIEW fx_evidence AS
WITH v AS (
  SELECT 'now' AS which, getvariable('fix_ev') AS j
  UNION ALL SELECT 'before', getvariable('fix_ev_prev')
)
SELECT which, e.node, e.state, e.likelihood, coalesce(lower(e.mode), 'observe') AS mode
FROM (
  SELECT which, unnest(from_json(coalesce(nullif(j, ''), '[]'),
    '[{"node": "VARCHAR", "state": "VARCHAR", "likelihood": "DOUBLE", "mode": "VARCHAR"}]')) AS e
  FROM v
);

-- The fault given the answers: its prior times each answer's likelihood
-- (soft evidence: the likelihood-weighted sum over its states).
CREATE OR REPLACE TEMP VIEW fx_posterior AS
WITH k AS (SELECT * FROM (VALUES ('now'), ('before'), ('prior')) v(which)),
lik AS (
  SELECT e.which, e.node, c.fault, sum(coalesce(e.likelihood, 1.0) * c.p) AS l
  FROM fx_evidence e JOIN fx_cpt c ON c.node = e.node AND c.state = e.state
  GROUP BY ALL
),
w AS (
  SELECT k.which, f.fault,
         f.prior * coalesce((SELECT product(l.l) FROM lik l WHERE l.which = k.which AND l.fault = f.fault), 1) AS w
  FROM k, fx_faults f
)
SELECT which, fault, w / nullif(sum(w) OVER (PARTITION BY which), 0) AS p FROM w;

-- Every node's belief: the fault's posterior through the node's table; a
-- node with an answer keeps it.
CREATE OR REPLACE TEMP VIEW fx_beliefs AS
WITH m AS (
  SELECT p.which, c.node, c.state, sum(p.p * c.p) AS p
  FROM fx_posterior p JOIN fx_cpt c USING (fault)
  GROUP BY ALL
),
seen AS (
  SELECT which, node, any_value(state) AS state FROM fx_evidence
  WHERE likelihood IS NULL GROUP BY ALL HAVING count(*) = 1
),
b AS (
  SELECT m.which, m.node, m.state,
         CASE WHEN s.node IS NULL THEN m.p ELSE (m.state = s.state)::INT END AS p
  FROM m LEFT JOIN seen s ON s.which = m.which AND s.node = m.node
)
SELECT now.node, now.state, round(now.p, 6) AS probability, round(pr.p, 6) AS prior, round(was.p, 6) AS previous
FROM b now
JOIN b pr ON pr.which = 'prior' AND pr.node = now.node AND pr.state = now.state
JOIN b was ON was.which = 'before' AND was.node = now.node AND was.state = now.state
WHERE now.which = 'now';

-- -------------------------------------------------------------------------
-- The plan. The repairs not yet tried and the tests not yet answered.
CREATE OR REPLACE TEMP VIEW fx_open_repairs AS
SELECT f.* FROM fx_faults f
WHERE NOT EXISTS (SELECT 1 FROM fx_evidence e WHERE e.which = 'now' AND e.node = 'after_' || f.fault);
CREATE OR REPLACE TEMP VIEW fx_open_tests AS
SELECT t.* FROM fx_tests t
WHERE NOT EXISTS (SELECT 1 FROM fx_evidence e WHERE e.which = 'now' AND e.node = t.test);
CREATE OR REPLACE TEMP VIEW fx_fixed AS
SELECT count(*) > 0 AS fixed FROM fx_evidence WHERE which = 'now' AND node LIKE 'after_%' AND state = 'fixed';

-- The branches: repairing directly (the beliefs now), and each open test's
-- answers (the beliefs given that answer too), each with its probability.
CREATE OR REPLACE TEMP VIEW fx_branches AS
SELECT 'repair directly' AS scenario, NULL AS answer, 1.0 AS p_answer, p.fault, p.p
FROM fx_posterior p WHERE p.which = 'now'
UNION ALL
SELECT t.test, g.answer, sum(p.p * g.p) OVER (PARTITION BY t.test, g.answer), p.fault,
       p.p * g.p / nullif(sum(p.p * g.p) OVER (PARTITION BY t.test, g.answer), 0)
FROM fx_open_tests t
JOIN fx_test_given g ON g.test = t.test
JOIN fx_posterior p ON p.which = 'now' AND p.fault = g.fault;

-- In each branch, the open repairs in the best order (probability per euro)
-- and in the order of repairing directly; each step's chance of being
-- reached (no earlier repair fixed it) and of fixing it then.
CREATE OR REPLACE TEMP VIEW fx_steps AS
WITH direct AS (
  SELECT r.fault, row_number() OVER (ORDER BY b.p / r.cost DESC, r.cost, r.fault) AS k_direct
  FROM fx_open_repairs r JOIN fx_branches b ON b.scenario = 'repair directly' AND b.fault = r.fault
),
s AS (
  SELECT b.scenario, b.answer, b.p_answer, r.fault, r.repair, r.cost, coalesce(b.p, 0) AS p,
         row_number() OVER (PARTITION BY b.scenario, b.answer ORDER BY coalesce(b.p, 0) / r.cost DESC, r.cost, r.fault) AS k,
         d.k_direct
  FROM fx_open_repairs r
  JOIN fx_branches b ON b.fault = r.fault
  JOIN direct d ON d.fault = r.fault
)
SELECT *,
       1 - coalesce(sum(p) OVER (PARTITION BY scenario, answer ORDER BY k ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS reach,
       1 - coalesce(sum(p) OVER (PARTITION BY scenario, answer ORDER BY k_direct ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 0) AS reach_direct
FROM s;

-- The expected cost of each plan: repairing directly, or each test first
-- (its cost, then the best order given its answer); and, per answer, how
-- much the better order saves against repairing directly.
CREATE OR REPLACE TEMP VIEW fx_scenarios AS
WITH per_answer AS (
  SELECT scenario, answer, any_value(p_answer) AS p_answer,
         sum(cost * greatest(reach, 0)) AS cost_best,
         sum(cost * greatest(reach_direct, 0)) AS cost_direct
  FROM fx_steps GROUP BY ALL
)
SELECT a.scenario, coalesce(t.label, 'Repair directly') AS label, coalesce(t.cost, 0) AS test_cost,
       coalesce(t.cost, 0) + sum(a.p_answer * a.cost_best) AS expected_cost,
       list({'answer': a.answer, 'p': a.p_answer, 'saving': a.p_answer * (a.cost_direct - a.cost_best)} ORDER BY a.answer) AS savings
FROM per_answer a LEFT JOIN fx_tests t ON t.test = a.scenario
GROUP BY a.scenario, t.label, t.cost;
CREATE OR REPLACE TEMP VIEW fx_best AS
SELECT arg_min(scenario, expected_cost) AS scenario, min(expected_cost) AS expected_cost FROM fx_scenarios;

-- The plan as the troubleshooting contract has it: step, kind, name, cost,
-- p_fix (the chance it fixes the printer if reached), expected_cost_after
-- (the plan's expected cost up to and including the step). After a test
-- first, step k is the k-th repair of the answer's order ("if prints: …").
CREATE OR REPLACE TEMP VIEW fx_plan AS
WITH best AS (SELECT * FROM fx_best),
t AS (
  SELECT 1 AS step, 'test' AS kind, s.label AS name, s.test_cost AS cost, 0.0 AS p_fix, s.test_cost AS expected_cost_after
  FROM fx_scenarios s, best WHERE s.scenario = best.scenario AND best.scenario <> 'repair directly'
),
r AS (
  SELECT k + (SELECT count(*) FROM t) AS step, 'repair' AS kind,
         CASE WHEN count(DISTINCT repair) = 1 THEN any_value(repair)
              ELSE string_agg(DISTINCT 'if ' || answer || ': ' || lower(repair), '; ') END AS name,
         sum(p_answer * cost * reach) / nullif(sum(p_answer * reach), 0) AS cost,
         sum(p_answer * p) / nullif(sum(p_answer * reach), 0) AS p_fix,
         sum(sum(p_answer * cost * greatest(reach, 0))) OVER (ORDER BY k) + coalesce((SELECT cost FROM t), 0) AS expected_cost_after,
         sum(p_answer * reach) AS reach
  FROM fx_steps, best WHERE fx_steps.scenario = best.scenario
  GROUP BY k
)
SELECT step, kind, name, round(cost, 2) AS cost, round(p_fix, 4) AS p_fix, round(expected_cost_after, 2) AS expected_cost_after
FROM (SELECT step, kind, name, cost, p_fix, expected_cost_after FROM t
      UNION ALL SELECT step, kind, name, cost, p_fix, expected_cost_after FROM r WHERE reach > 1e-9)
WHERE NOT (SELECT fixed FROM fx_fixed)
ORDER BY step;

-- -------------------------------------------------------------------------

SELECT 'What do we try first?'::TAB;

SELECT 'The plan'::GROUP;
SELECT CASE WHEN (SELECT fixed FROM fx_fixed) THEN 0 ELSE (SELECT round(expected_cost, 2) FROM fx_best) END ::MONEY,
       'EUR'::CURRENCY_CODE,
       CASE WHEN (SELECT fixed FROM fx_fixed) THEN 'Fixed: nothing left to try'
            ELSE 'Expected cost of the plan' END ::LABEL;
SELECT round(expected_cost, 2) ::MONEY, 'EUR'::CURRENCY_CODE, 'Expected cost of repairing directly' ::LABEL
FROM fx_scenarios WHERE scenario = 'repair directly';
SELECT '**Next step:** ' || coalesce((SELECT name FROM fx_plan WHERE step = 1), 'none, it is fixed') ::MARKDOWN,
       'plain'::MARKDOWN_STYLE;
SELECT 1::ENDGROUP;

SELECT 4::SPAN;
SELECT '**The plan** is the order with the lowest expected cost. Without a test, the repairs go by probability per euro: the cable first (cheap), then the driver, the paper path and the toner, 34.50 on average. Looking at the toner light first costs 1 and brings it to 26.49: if it is on, the toner goes first.

**Answer as you go.** Click **on** or **off** in *Toner light*, or **still broken** after a repair, and the plan is made again from what you now know. The waterfall shows where a test''s saving comes from: what it costs, and how much the better order saves after each answer, weighted by how likely that answer is.

After a test, a step names the repair for each answer, its cost is the expected cost if it comes to it.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 560::HEIGHT;
SELECT 'fix_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM fx_arcs a),
         beliefs := (SELECT list(b) FROM fx_beliefs b),
         evidence := (SELECT list(e) FROM fx_evidence e WHERE e.which = 'now'),
         tables := (SELECT list(t) FROM fx_tables t),
         direction := 'LR')
       AS "Click an answer: a test's result, or a repair that left it still broken"
FROM fx_nodes n;

SELECT 12::SPAN;
SELECT 'none'::TOTALS;
SELECT step AS "#" ::TABLE, kind, name AS "what to do", cost ::MONEY, 100 * p_fix AS "fixes it" ::PERCENT,
       expected_cost_after AS "expected so far" ::MONEY, 'EUR'::CURRENCY_CODE,
       'The plan, cheapest on average' ::TITLE
FROM fx_plan ORDER BY "#";

SELECT 6::SPAN;
SELECT step ::XAXIS, expected_cost_after AS "expected cost so far" ::LINECHART, '€'::YFORMAT,
       'The expected cost adds up step by step' ::TITLE
FROM fx_plan ORDER BY step;

-- the test the waterfall takes apart: the plan's, else the best one
CREATE OR REPLACE TEMP VIEW fx_shown_test AS
SELECT s.* FROM fx_scenarios s
WHERE s.scenario <> 'repair directly'
ORDER BY s.scenario = (SELECT scenario FROM fx_best) DESC, s.expected_cost
LIMIT 1;

SELECT 6::SPAN;
SELECT label ::XAXIS, amount ::WATERFALL, kind ::CATEGORY, '€'::YFORMAT,
       (SELECT label FROM fx_shown_test) || ' first or repair directly: where the saving comes from' ::TITLE
FROM (
  SELECT 1 AS i, 'Repair directly' AS label, expected_cost AS amount, NULL AS kind
  FROM fx_scenarios WHERE scenario = 'repair directly'
  UNION ALL
  SELECT 2, 'the test', test_cost, NULL FROM fx_shown_test
  UNION ALL
  SELECT 3, 'if ' || x.answer, -x.saving, NULL
  FROM (SELECT unnest(savings) AS x FROM fx_shown_test)
  UNION ALL
  SELECT 4, 'Test first', expected_cost, 'total' FROM fx_shown_test
)
ORDER BY i, label;

-- -------------------------------------------------------------------------
-- Evidence optimization: which answers of the open tests would point most
-- to a fault, or away from it, on top of what is known.
SELECT 'Which answers point to a fault?'::TAB;

SELECT fault AS fx_target ::BUTTONS, fault = 'driver' ::DEFAULT, 'Fault' ::LABEL
FROM fx_faults ORDER BY prior DESC;

-- Every way the open tests could answer: each test unanswered or one of
-- its answers (3 × 3 × 3 for three tests), with the fault's probability
-- given those answers and what is known.
CREATE OR REPLACE TEMP VIEW fx_configs AS
WITH RECURSIVE t AS (
  SELECT row_number() OVER (ORDER BY test) AS i, test, label, answers FROM fx_open_tests
),
c(i, cfg) AS (
  SELECT 0, []::STRUCT(test VARCHAR, answer VARCHAR)[]
  UNION ALL
  SELECT c.i + 1, CASE WHEN u.a IS NULL THEN c.cfg ELSE list_append(c.cfg, {'test': t.test, 'answer': u.a}) END
  FROM c JOIN t ON t.i = c.i + 1, LATERAL (SELECT unnest(list_prepend(NULL::VARCHAR, t.answers)) AS a) u
),
full_cfg AS (SELECT row_number() OVER (ORDER BY cfg::VARCHAR) AS id, cfg FROM c WHERE i = (SELECT count(*) FROM t)),
w AS (
  SELECT c.id, c.cfg, p.fault,
         p.p * coalesce((SELECT product(g.p) FROM unnest(c.cfg) x(e)
                         JOIN fx_test_given g ON g.test = x.e.test AND g.answer = x.e.answer AND g.fault = p.fault), 1) AS w
  FROM full_cfg c, fx_posterior p WHERE p.which = 'now'
)
SELECT id, cfg, len(cfg) AS answers, fault, w / nullif(sum(w) OVER (PARTITION BY id), 0) AS p,
       sum(w) OVER (PARTITION BY id) AS p_answers
FROM w;

-- Exhaustive: every combination of answers to all open tests, ranked.
-- Greedy: one answer at a time, the one that moves the fault most.
CREATE OR REPLACE TEMP VIEW fx_search AS
WITH target AS (
  SELECT id, cfg, answers, p, p_answers FROM fx_configs
  WHERE fault = getvariable('fx_target') AND p_answers > 0
),
goals AS (SELECT * FROM (VALUES ('points to it', 1), ('points away', -1)) g(goal, sign)),
open_tests AS (SELECT count(*) AS n FROM fx_open_tests),
exhaustive AS (
  SELECT g.goal, 'exhaustive' AS method,
         row_number() OVER (PARTITION BY g.goal ORDER BY g.sign * t.p DESC, t.p_answers DESC) AS rank, t.cfg, t.p, t.p_answers
  FROM target t, goals g, open_tests o
  WHERE t.answers = o.n
),
-- greedy, three steps (one per test): extend the last pick by the answer
-- that moves the fault most
g1 AS (
  SELECT g.goal, g.sign, arg_max(t.id, g.sign * t.p) AS id
  FROM goals g, target t WHERE t.answers = 1 GROUP BY ALL
),
g2 AS (
  SELECT g.goal, g.sign, arg_max(t.id, g.sign * t.p) AS id
  FROM g1 g JOIN target prev ON prev.id = g.id, target t
  WHERE t.answers = 2 AND list_has_all(t.cfg, prev.cfg) GROUP BY ALL
),
g3 AS (
  SELECT g.goal, g.sign, arg_max(t.id, g.sign * t.p) AS id
  FROM g2 g JOIN target prev ON prev.id = g.id, target t
  WHERE t.answers = 3 AND list_has_all(t.cfg, prev.cfg) GROUP BY ALL
),
greedy AS (
  SELECT goal, id FROM g1, open_tests o WHERE o.n = 1
  UNION ALL SELECT goal, id FROM g2, open_tests o WHERE o.n = 2
  UNION ALL SELECT goal, id FROM g3, open_tests o WHERE o.n = 3
)
SELECT goal, method, rank, cfg, p, p_answers FROM exhaustive WHERE rank <= 3
UNION ALL
SELECT g.goal, 'greedy', 1, t.cfg, t.p, t.p_answers
FROM greedy g JOIN target t ON t.id = g.id;

-- a configuration as text: "Toner light on, Test page fails"
CREATE OR REPLACE TEMP VIEW fx_search_text AS
SELECT s.*, (SELECT string_agg(t.label || ' ' || x.e.answer, ', ' ORDER BY t.label)
             FROM unnest(s.cfg) x(e) JOIN fx_tests t ON t.test = x.e.test) AS answers_text
FROM fx_search s;

SELECT 12::SPAN;
SELECT CASE goal WHEN 'points to it' THEN '↑ ' ELSE '↓ ' END
       || CASE method WHEN 'greedy' THEN 'greedy' ELSE '#' || rank END || ': ' || answers_text ::XAXIS,
       100 * p AS "probability of the fault" ::BARCHART, goal ::CATEGORY, 1::FLIP, 'percent'::YFORMAT,
       'The answers that point most to the ' || getvariable('fx_target') || ', and away from it' ::TITLE
FROM fx_search_text ORDER BY goal DESC, method, rank;

SELECT 12::SPAN;
SELECT 'Every test is answered: nothing left to search.'::EMPTY;
SELECT 'none'::TOTALS;
SELECT goal ::TABLE, method, rank, answers_text AS answers, 100 * p AS "probability of the fault" ::PERCENT,
       100 * p_answers AS "chance of these answers" ::PERCENT,
       'Exhaustive: every combination of answers; greedy: one answer at a time' ::TITLE
FROM fx_search_text ORDER BY goal DESC, method, rank;
