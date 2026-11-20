-- @title Should we drill, and is the seismic test worth paying for?
-- @group How it works
-- @tags  diagram, bayesian network, influence diagram, decision, policy, expected utility
-- @description The oil wildcatter as an influence diagram: the best decision for each test result, what the test is worth, and what you expect to earn.
--
-- Should we drill, and is the seismic test worth paying for? The oil
-- wildcatter (Raiffa, 1968), the textbook influence diagram: a wildcatter
-- can drill a site or walk away, and can first pay 10 for a seismic test
-- whose result (closed, open or diffuse structure) hints at how much oil
-- there is. The tables are plain SQL (the beliefs about the oil, the test's
-- reliability, the payoff), and the policy is solved from them below by
-- backward induction: for every test result, the expected payoff of
-- drilling and of walking away; for the test itself, the expected payoff of
-- testing (then acting on its result) and of not testing.
--
-- anofox_plot_bayes_net draws the influence diagram (#1279): the decisions
-- as square boxes, each with its policy as a table (a row per thing known
-- when deciding, a column per option with its expected payoff, the best one
-- marked); the payoffs as diamonds with their expected value; the dotted
-- arcs are what is known when deciding. Click an option to decide it, click
-- a test result to observe it: getvariable('oil_ev') holds your choices,
-- and the beliefs and expected payoffs below follow them (a decision you
-- leave open follows the policy).

SELECT 'bayes-net'::REQUIRES;

-- The diagram. Two decisions (test? drill?), two chance nodes (the oil, the
-- test's result), two payoffs (the oil sold, the test's cost).
CREATE OR REPLACE TEMP TABLE oil_nodes AS
FROM (VALUES
  ('test', 'Test?', 'decision', ['test', 'no test']),
  ('oil', 'Oil', 'chance', ['dry', 'wet', 'soaking']),
  ('seismic', 'Seismic', 'chance', ['closed', 'open', 'diffuse', 'none']),
  ('drill', 'Drill?', 'decision', ['drill', 'walk away']),
  ('payoff', 'Payoff', 'utility', []::VARCHAR[]),
  ('test_cost', 'Test cost', 'utility', []::VARCHAR[])
) t(id, label, kind, states);

-- The arcs in each child's parent order. Into a decision: what is known
-- when deciding (an information arc, drawn dotted).
CREATE OR REPLACE TEMP TABLE oil_arcs AS
FROM (VALUES
  (1, 'oil', 'seismic'), (2, 'test', 'seismic'),
  (3, 'test', 'drill'), (4, 'seismic', 'drill'),
  (5, 'oil', 'payoff'), (6, 'drill', 'payoff'),
  (7, 'test', 'test_cost')
) t(k, "from", "to");

-- What we believe about the oil, and how the test reads each case
-- (a closed structure is likelier the more oil there is).
CREATE OR REPLACE TEMP TABLE oil_prior AS
FROM (VALUES ('dry', 0.5), ('wet', 0.3), ('soaking', 0.2)) t(oil, p);
CREATE OR REPLACE TEMP TABLE oil_seismic AS
FROM (VALUES
  ('dry', 'closed', 0.1), ('dry', 'open', 0.3), ('dry', 'diffuse', 0.6),
  ('wet', 'closed', 0.3), ('wet', 'open', 0.4), ('wet', 'diffuse', 0.3),
  ('soaking', 'closed', 0.5), ('soaking', 'open', 0.4), ('soaking', 'diffuse', 0.1)
) t(oil, seismic, p);
-- The payoff of each decision in each case; the test costs 10.
CREATE OR REPLACE TEMP TABLE oil_payoff AS
FROM (VALUES
  ('drill', 'dry', -70), ('drill', 'wet', 50), ('drill', 'soaking', 200),
  ('walk away', 'dry', 0), ('walk away', 'wet', 0), ('walk away', 'soaking', 0)
) t(drill, oil, value);
SET VARIABLE oil_test_cost = 10;

-- The conditional probability tables, as the plugin's card shows them
-- (parent_states in the order of the node's arcs). Without a test the
-- result is 'none'.
CREATE OR REPLACE TEMP TABLE oil_tables AS
SELECT 'oil' AS node, []::VARCHAR[] AS parent_states, oil AS state, p AS probability FROM oil_prior
UNION ALL
SELECT 'seismic', [o.oil, t.test], s.state,
       CASE WHEN t.test = 'no test' THEN (s.state = 'none')::INT
            ELSE coalesce((SELECT p FROM oil_seismic x WHERE x.oil = o.oil AND x.seismic = s.state), 0) END
FROM oil_prior o, (VALUES ('test'), ('no test')) t(test),
     (VALUES ('closed'), ('open'), ('diffuse'), ('none')) s(state);

-- Every case: the test decision, the oil, the result, the drill decision,
-- its probability given both decisions and its payoff.
CREATE OR REPLACE TEMP TABLE oil_cases AS
SELECT t.test, o.oil, s.state AS seismic, d.drill, o.p * tb.probability AS p,
       pay.value AS payoff,
       CASE WHEN t.test = 'test' THEN -getvariable('oil_test_cost') ELSE 0 END AS test_cost
FROM (VALUES ('test'), ('no test')) t(test), oil_prior o,
     (VALUES ('closed'), ('open'), ('diffuse'), ('none')) s(state),
     (VALUES ('drill'), ('walk away')) d(drill),
     oil_tables tb, oil_payoff pay
WHERE tb.node = 'seismic' AND tb.parent_states = [o.oil, t.test] AND tb.state = s.state
  AND tb.probability > 0 AND pay.drill = d.drill AND pay.oil = o.oil;

-- The policy, by backward induction. Drill?: for each thing known when
-- deciding (the test decision and its result), the expected payoff of each
-- option given it; the best is chosen.
CREATE OR REPLACE TEMP TABLE oil_drill_policy AS
WITH eu AS (
  SELECT test, seismic, drill, sum(p * (payoff + test_cost)) / sum(p) AS eu,
         sum(p) AS p_known
  FROM oil_cases GROUP BY ALL
)
SELECT *, row_number() OVER (PARTITION BY test, seismic ORDER BY eu DESC, drill) = 1 AS chosen
FROM eu;
-- Test first?: the expected payoff of each option, acting on the result
-- with the best drill decision.
CREATE OR REPLACE TEMP TABLE oil_test_policy AS
WITH eu AS (
  SELECT test, sum(p_known * eu) AS eu
  FROM oil_drill_policy WHERE chosen GROUP BY ALL
)
SELECT *, row_number() OVER (ORDER BY eu DESC, test) = 1 AS chosen FROM eu;

-- The policy as the plugin reads it: decision, parent_states (in the order
-- of the arcs into the decision), option, expected_utility, chosen.
CREATE OR REPLACE TEMP TABLE oil_policy AS
SELECT 'test' AS decision, []::VARCHAR[] AS parent_states, test AS option,
       round(eu, 1) AS expected_utility, chosen
FROM oil_test_policy
UNION ALL
SELECT 'drill', [test, seismic], drill, round(eu, 1), chosen
FROM oil_drill_policy
ORDER BY decision DESC, parent_states, option;

-- Your choices and observations, as the clicks left them ('now') and
-- before the last click ('before').
CREATE OR REPLACE TEMP VIEW oil_evidence AS
WITH v AS (
  SELECT 'now' AS which, getvariable('oil_ev') AS j
  UNION ALL SELECT 'before', getvariable('oil_ev_prev')
)
SELECT which, e.node, e.state, e.likelihood, coalesce(lower(e.mode), 'observe') AS mode
FROM (
  SELECT which, unnest(from_json(coalesce(nullif(j, ''), '[]'),
    '[{"node": "VARCHAR", "state": "VARCHAR", "likelihood": "DOUBLE", "mode": "VARCHAR"}]')) AS e
  FROM v
);

-- Each case's weight given the evidence: a decision follows your choice,
-- else the policy; an observed chance node keeps the cases with its state
-- (soft evidence: weighs them by the likelihood).
CREATE OR REPLACE TEMP VIEW oil_weights AS
WITH k AS (SELECT * FROM (VALUES ('now'), ('before'), ('prior')) v(which)),
ev AS (SELECT * FROM oil_evidence),
lik AS (
  SELECT which, node, state, sum(coalesce(likelihood, 1.0)) AS lik FROM ev GROUP BY ALL
),
has AS (SELECT DISTINCT which, node FROM ev)
SELECT k.which, c.*,
       c.p
       * CASE WHEN EXISTS (SELECT 1 FROM has h WHERE h.which = k.which AND h.node = 'test')
              THEN coalesce((SELECT lik FROM lik l WHERE l.which = k.which AND l.node = 'test' AND l.state = c.test), 0)
              ELSE (SELECT chosen::INT FROM oil_test_policy tp WHERE tp.test = c.test) END
       * CASE WHEN EXISTS (SELECT 1 FROM has h WHERE h.which = k.which AND h.node = 'drill')
              THEN coalesce((SELECT lik FROM lik l WHERE l.which = k.which AND l.node = 'drill' AND l.state = c.drill), 0)
              ELSE (SELECT chosen::INT FROM oil_drill_policy dp
                    WHERE dp.test = c.test AND dp.seismic = c.seismic AND dp.drill = c.drill) END
       * CASE WHEN EXISTS (SELECT 1 FROM has h WHERE h.which = k.which AND h.node = 'oil')
              THEN coalesce((SELECT lik FROM lik l WHERE l.which = k.which AND l.node = 'oil' AND l.state = c.oil), 0)
              ELSE 1 END
       * CASE WHEN EXISTS (SELECT 1 FROM has h WHERE h.which = k.which AND h.node = 'seismic')
              THEN coalesce((SELECT lik FROM lik l WHERE l.which = k.which AND l.node = 'seismic' AND l.state = c.seismic), 0)
              ELSE 1 END AS w
FROM k, oil_cases c;

-- The beliefs about the chance nodes (the prior: no evidence, the policy
-- followed), and the payoffs' expected values given the evidence.
CREATE OR REPLACE TEMP VIEW oil_beliefs AS
WITH m AS (
  SELECT which, node, state, sum(w) / nullif(sum(sum(w)) OVER (PARTITION BY which, node), 0) AS p
  FROM (SELECT which, 'oil' AS node, oil AS state, w FROM oil_weights
        UNION ALL SELECT which, 'seismic', seismic, w FROM oil_weights)
  GROUP BY which, node, state
),
states AS (
  SELECT id AS node, unnest(states) AS state FROM oil_nodes WHERE kind = 'chance'
)
SELECT s.node, s.state, coalesce(now.p, 0) AS probability, coalesce(pr.p, 0) AS prior,
       coalesce(was.p, 0) AS previous, NULL::DOUBLE AS expected_utility
FROM states s
LEFT JOIN m now ON now.which = 'now' AND now.node = s.node AND now.state = s.state
LEFT JOIN m was ON was.which = 'before' AND was.node = s.node AND was.state = s.state
LEFT JOIN m pr ON pr.which = 'prior' AND pr.node = s.node AND pr.state = s.state
UNION ALL
SELECT u.node, NULL, NULL, NULL, NULL,
       round(sum(w * CASE u.node WHEN 'payoff' THEN payoff ELSE test_cost END) / nullif(sum(w), 0), 2)
FROM oil_weights, (VALUES ('payoff'), ('test_cost')) u(node)
WHERE which = 'now'
GROUP BY u.node;

-- -------------------------------------------------------------------------

SELECT 'What the decision is worth'::GROUP;
SELECT round(max(eu), 1) ::METRIC,
       'Best expected payoff (' || arg_max(test, eu) || ', then act on the result)' ::LABEL
FROM oil_test_policy;

SELECT round((SELECT eu FROM oil_test_policy WHERE test = 'test')
             + getvariable('oil_test_cost')
             - (SELECT eu FROM oil_test_policy WHERE test = 'no test'), 1) ::METRIC,
       'What the test is worth (it costs ' || getvariable('oil_test_cost') || ')' ::LABEL;

SELECT round(sum(expected_utility), 1) ::METRIC,
       'Expected payoff, given ' || coalesce((SELECT string_agg(node || ' = ' || state, ', ')
                                              FROM oil_evidence WHERE which = 'now'),
                                             'the policy') ::LABEL
FROM oil_beliefs WHERE expected_utility IS NOT NULL;
SELECT 1::ENDGROUP;

SELECT 4::SPAN;
SELECT '**The question.** Drill, or walk away? A seismic test costs 10 and reads the structure as *closed*, *open* or *diffuse*; the more oil, the likelier closed.

**The policy** is in each decision''s box: a row per thing known when deciding, a column per option with its expected payoff, the best one marked. Drill unless the test reads diffuse; test first, it lifts the expected payoff from 20 to 22.5.

**Try it.** Click **test** in *Test?*, then **diffuse** in *Seismic*: that row lights up, the oil is likely dry (73 %), walk away. **Closed** instead: soaking is 42 % likely, drill.

The dotted arcs are what is known when deciding. The *Payoff* diamond leaves out the test''s cost (its own node, *Test cost*), which is why it differs from the best expected payoff above.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 520::HEIGHT;
SELECT 'oil_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM oil_arcs a),
         beliefs := (SELECT list(b) FROM oil_beliefs b),
         evidence := (SELECT list(e) FROM oil_evidence e WHERE e.which = 'now'),
         tables := (SELECT list(t) FROM oil_tables t),
         policy := (SELECT list(p) FROM oil_policy p))
       AS "The wildcatter's decisions and their policies"
FROM oil_nodes n;
