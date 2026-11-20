-- @title Does the discount keep customers?
-- @group Sales & customers
-- @tags  diagram, bayesian network, causal, intervention, counterfactual, structure learning
-- @description Seeing or doing: what a renewal discount really does, how big its effect is, what would have happened without it, and whether a learner found the network.
--
-- Does the discount keep customers? Customers who got a renewal discount
-- renew more often — but sales gives the discount mostly to engaged
-- customers, who renew more often anyway. A Bayesian network separates
-- seeing from doing (#1278): the drawing marks the treatment (X), the outcome
-- (Y) and the adjustment set (ringed), the back-door paths dashed — red when
-- open, with a bar where the set blocks them. The charts compare
-- P(renewed | got the discount) with P(renewed | do(give the discount)),
-- estimate the effect four ways with intervals, split it into a direct and
-- an indirect part, put a customer's factual and counterfactual worlds side
-- by side, and compare a learned network with the true one.
--
-- Every table is plain SQL, as a producer (pgmpy, bnlearn, a DuckDB
-- extension) would write it:
--   * cz_nodes, cz_arcs, cz_cpt — the network (P(yes) per parent
--     configuration; cz_tables has both states);
--   * cz_adjust: treatment, outcome, set_id, node, kind (minimal | optimal |
--     canonical, front-door for a front-door mediator) — the adjustment
--     sets, drawn with adjust := ;
--   * cz_seeing_doing: node, value, mode (observe | do), target_state,
--     probability — the paired chart;
--   * cz_effects: effect, estimate, lo, hi, truth — the effect cards;
--   * cz_twin: node, state, probability, prior, world (factual |
--     counterfactual) — the twin network;
--   * cz_learned: from, to, strength — a structure learner's arcs, drawn
--     against the true ones with reference := .
-- The beliefs are exact (every case of the network enumerated); the effects
-- are estimated from 8,000 customers simulated from the network, with 95 %
-- intervals from 100 bootstrap resamples, next to the true value. The
-- learned arcs are written by hand, as a learner would export them.

-- Bayesian networks are a plugin (#1275): a build without it says so.
SELECT 'bayes-net'::REQUIRES;

-- ─────────────────────────────────────────────────────────────────────────────
-- The network
-- ─────────────────────────────────────────────────────────────────────────────
-- 'discount': large accounts are more often engaged; sales gives the
-- discount mostly to engaged customers; the discount makes weekly use more
-- likely; renewal depends on engagement, the discount (a little), weekly use
-- and a price rise. 'hidden': the same story with engagement not measured.
CREATE OR REPLACE TEMP TABLE cz_nodes AS
FROM (VALUES
  ('discount', 'size', 'Large account', 'chance', ['yes', 'no']),
  ('discount', 'engaged', 'Engaged customer', 'chance', ['yes', 'no']),
  ('discount', 'price', 'Price rose', 'chance', ['yes', 'no']),
  ('discount', 'discount', 'Got the discount', 'chance', ['yes', 'no']),
  ('discount', 'usage', 'Uses it weekly', 'chance', ['yes', 'no']),
  ('discount', 'stay', 'Renewed', 'chance', ['yes', 'no']),
  ('hidden', 'engaged', 'Engaged (not measured)', 'latent', ['yes', 'no']),
  ('hidden', 'discount', 'Got the discount', 'chance', ['yes', 'no']),
  ('hidden', 'usage', 'Uses it weekly', 'chance', ['yes', 'no']),
  ('hidden', 'stay', 'Renewed', 'chance', ['yes', 'no'])
) t(net, id, label, kind, states);

-- the arcs, a node's parents in the order of its table's parent_states
CREATE OR REPLACE TEMP TABLE cz_arcs AS
FROM (VALUES
  ('discount', 1, 'size', 'engaged'),
  ('discount', 2, 'engaged', 'discount'),
  ('discount', 3, 'discount', 'usage'),
  ('discount', 4, 'engaged', 'usage'),
  ('discount', 5, 'engaged', 'stay'),
  ('discount', 6, 'discount', 'stay'),
  ('discount', 7, 'usage', 'stay'),
  ('discount', 8, 'price', 'stay'),
  ('hidden', 1, 'engaged', 'discount'),
  ('hidden', 2, 'discount', 'usage'),
  ('hidden', 3, 'usage', 'stay'),
  ('hidden', 4, 'engaged', 'stay')
) t(net, k, "from", "to");

-- P(yes) per parent configuration. Renewal: 35 % plus 25 points when
-- engaged, 10 for the discount, 20 for weekly use, minus 15 after a price
-- rise.
CREATE OR REPLACE TEMP TABLE cz_cpt AS
FROM (VALUES
  ('discount', 'size', []::VARCHAR[], 0.3),
  ('discount', 'engaged', ['yes'], 0.7),
  ('discount', 'engaged', ['no'], 0.35),
  ('discount', 'price', []::VARCHAR[], 0.4),
  ('discount', 'discount', ['yes'], 0.7),
  ('discount', 'discount', ['no'], 0.2),
  ('discount', 'usage', ['yes', 'yes'], 0.8),
  ('discount', 'usage', ['yes', 'no'], 0.55),
  ('discount', 'usage', ['no', 'yes'], 0.6),
  ('discount', 'usage', ['no', 'no'], 0.25),
  ('hidden', 'engaged', []::VARCHAR[], 0.45),
  ('hidden', 'discount', ['yes'], 0.7),
  ('hidden', 'discount', ['no'], 0.2),
  ('hidden', 'usage', ['yes'], 0.75),
  ('hidden', 'usage', ['no'], 0.3),
  ('hidden', 'stay', ['yes', 'yes'], 0.85),
  ('hidden', 'stay', ['yes', 'no'], 0.6),
  ('hidden', 'stay', ['no', 'yes'], 0.65),
  ('hidden', 'stay', ['no', 'no'], 0.3)
) t(net, node, parent_states, p_yes)
UNION ALL
SELECT 'discount', 'stay', [e, d, u, p],
       round(0.35 + 0.25 * (e = 'yes')::INT + 0.10 * (d = 'yes')::INT
                  + 0.20 * (u = 'yes')::INT - 0.15 * (p = 'yes')::INT, 2)
FROM (VALUES ('yes'), ('no')) a(e), (VALUES ('yes'), ('no')) b(d),
     (VALUES ('yes'), ('no')) c(u), (VALUES ('yes'), ('no')) q(p);

CREATE OR REPLACE TEMP TABLE cz_tables AS
SELECT net, node, parent_states, 'yes' AS state, p_yes AS probability FROM cz_cpt
UNION ALL
SELECT net, node, parent_states, 'no', round(1 - p_yes, 6) FROM cz_cpt;

-- Every case of a network, one row per node: case c gives node k (with m
-- states) the state (c / stride) % m, stride the product of the state counts
-- before it.
CREATE OR REPLACE TEMP TABLE cz_cases AS
WITH n AS (
  SELECT net, id, states, len(states) AS m,
         coalesce(product(len(states)) OVER (PARTITION BY net ORDER BY id
           ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 1)::BIGINT AS stride,
         product(len(states)) OVER (PARTITION BY net)::BIGINT AS total
  FROM cz_nodes
)
SELECT n.net, c.range AS "case", n.id AS node, n.states[(c.range // n.stride) % n.m + 1] AS state
FROM (SELECT DISTINCT net, total FROM n) t, range(t.total) c, n
WHERE n.net = t.net;

-- Each node's factor in each case: its table's probability of its state
-- given its parents' states.
CREATE OR REPLACE TEMP TABLE cz_factors AS
WITH parents AS (
  SELECT c.net, c."case", a."to" AS node, list(c.state ORDER BY a.k) AS parent_states
  FROM cz_arcs a JOIN cz_cases c ON c.net = a.net AND c.node = a."from"
  GROUP BY ALL
)
SELECT c.net, c."case", c.node, c.state, t.probability AS f
FROM cz_cases c
LEFT JOIN parents p USING (net, "case", node)
JOIN cz_tables t ON t.net = c.net AND t.node = c.node AND t.state = c.state
  AND t.parent_states = coalesce(p.parent_states, []::VARCHAR[]);

-- ─────────────────────────────────────────────────────────────────────────────
-- Queries: the beliefs under a scenario (evidence: observe or do)
-- ─────────────────────────────────────────────────────────────────────────────
-- Scenarios: the clicks on the drawing ('now', and 'before' the last click),
-- the prior, and for the paired chart each node and state seen (observe) or
-- set (do).
CREATE OR REPLACE TEMP VIEW cz_scenarios AS
WITH clicks AS (
  SELECT 'now' AS scen, getvariable('cz_ev') AS j
  UNION ALL SELECT 'before', getvariable('cz_ev_prev')
)
SELECT 'discount' AS net, scen, e.node, e.state, e.likelihood, coalesce(lower(e.mode), 'observe') AS mode
FROM (
  SELECT scen, unnest(from_json(coalesce(nullif(j, ''), '[]'),
    '[{"node": "VARCHAR", "state": "VARCHAR", "likelihood": "DOUBLE", "mode": "VARCHAR"}]')) AS e
  FROM clicks
)
UNION ALL
SELECT n.net, m.mode || ':' || n.id || '=' || s.state, n.id, s.state, NULL, m.mode
FROM cz_nodes n, unnest(n.states) s(state), (VALUES ('observe'), ('do')) m(mode)
WHERE n.id IN ('discount', 'usage');

-- The beliefs per scenario. A case weighs the product of its factors; an
-- observed node keeps the cases with its state (soft evidence weighs each by
-- its likelihood); do() sets the node: its own factor drops out and only
-- the cases with the set state are kept.
CREATE OR REPLACE TEMP VIEW cz_posterior AS
WITH k AS (
  SELECT DISTINCT net, scen FROM cz_scenarios
  UNION SELECT DISTINCT net, 'prior' FROM cz_nodes
  UNION SELECT 'discount', 'now' UNION SELECT 'discount', 'before'
),
on_node AS (SELECT net, scen, node, bool_or(mode = 'do') AS is_do FROM cz_scenarios GROUP BY ALL),
lik AS (SELECT net, scen, node, state, sum(coalesce(likelihood, 1.0)) AS lik FROM cz_scenarios GROUP BY ALL),
w AS (
  SELECT k.net, k.scen, f."case",
         product(CASE WHEN o.is_do THEN 1.0 ELSE f.f END
                 * CASE WHEN o.node IS NULL THEN 1.0 ELSE coalesce(l.lik, 0.0) END) AS w
  FROM cz_factors f
  JOIN k ON k.net = f.net
  LEFT JOIN on_node o ON o.net = k.net AND o.scen = k.scen AND o.node = f.node
  LEFT JOIN lik l ON l.net = k.net AND l.scen = k.scen AND l.node = f.node AND l.state = f.state
  GROUP BY ALL
)
SELECT w.net, w.scen, c.node, c.state,
       sum(w.w) / nullif(sum(sum(w.w)) OVER (PARTITION BY w.net, w.scen, c.node), 0) AS p
FROM w JOIN cz_cases c ON c.net = w.net AND c."case" = w."case"
GROUP BY w.net, w.scen, c.node, c.state;

-- The drawing's beliefs: given the clicks, the prior, before the last click.
CREATE OR REPLACE TEMP VIEW cz_beliefs AS
SELECT now.net, now.node, now.state, now.p AS probability, pr.p AS prior, was.p AS previous
FROM cz_posterior now
JOIN cz_posterior pr ON pr.scen = 'prior' AND pr.net = now.net AND pr.node = now.node AND pr.state = now.state
LEFT JOIN cz_posterior was ON was.scen = 'before' AND was.net = now.net AND was.node = now.node AND was.state = now.state
WHERE now.scen = CASE WHEN now.net = 'discount' THEN 'now' ELSE 'prior' END;

CREATE OR REPLACE TEMP VIEW cz_evidence AS
SELECT net, node, state, likelihood, mode FROM cz_scenarios WHERE scen = 'now';

-- Seeing or doing: P(renewed = yes) with a node seen at a value and with it
-- set to that value, one row each.
CREATE OR REPLACE TEMP VIEW cz_seeing_doing AS
SELECT s.net, split_part(split_part(s.scen, ':', 2), '=', 1) AS node,
       split_part(s.scen, '=', 2) AS value, split_part(s.scen, ':', 1) AS mode,
       'yes' AS target_state, p.p AS probability
FROM (SELECT DISTINCT net, scen FROM cz_scenarios WHERE scen LIKE '%:%') s
JOIN cz_posterior p ON p.net = s.net AND p.scen = s.scen AND p.node = 'stay' AND p.state = 'yes';

-- ─────────────────────────────────────────────────────────────────────────────
-- The adjustment sets (a producer's table)
-- ─────────────────────────────────────────────────────────────────────────────
-- From Got the discount to Renewed. Minimal: the one confounder. Optimal (the
-- O-set): the outcome's other causes too, for the tightest interval.
-- Canonical: every cause of the treatment or the outcome that is not
-- downstream of the treatment. 'none' compares without adjusting. In the
-- 'hidden' network no set exists (engagement is not measured), but weekly
-- use is a front-door mediator.
CREATE OR REPLACE TEMP TABLE cz_adjust AS
FROM (VALUES
  ('discount', 'discount', 'stay', 'minimal', 'engaged', 'minimal'),
  ('discount', 'discount', 'stay', 'optimal', 'engaged', 'optimal'),
  ('discount', 'discount', 'stay', 'optimal', 'price', 'optimal'),
  ('discount', 'discount', 'stay', 'canonical', 'size', 'canonical'),
  ('discount', 'discount', 'stay', 'canonical', 'engaged', 'canonical'),
  ('discount', 'discount', 'stay', 'canonical', 'price', 'canonical'),
  ('discount', 'discount', 'stay', 'none', NULL, NULL),
  ('hidden', 'discount', 'stay', 'front-door', 'usage', 'front-door')
) t(net, treatment, outcome, set_id, node, kind);

-- ─────────────────────────────────────────────────────────────────────────────
-- The effects, estimated from 8,000 simulated customers
-- ─────────────────────────────────────────────────────────────────────────────
-- Each customer is drawn from the network, node by node: a node is yes when
-- its uniform draw (a hash of the customer and the node) is below its
-- table's P(yes) given the parents drawn before.
CREATE OR REPLACE TEMP TABLE cz_sample AS
WITH u AS (
  SELECT i, n.id AS node, (hash(i::VARCHAR || '/' || n.id) % 1000000) / 1e6 AS u
  FROM range(8000) r(i), cz_nodes n WHERE n.net = 'discount'
),
p AS (
  SELECT i, any_value(u) FILTER (node = 'size') AS size, any_value(u) FILTER (node = 'engaged') AS engaged,
         any_value(u) FILTER (node = 'price') AS price, any_value(u) FILTER (node = 'discount') AS discount,
         any_value(u) FILTER (node = 'usage') AS usage, any_value(u) FILTER (node = 'stay') AS stay
  FROM u GROUP BY i
),
p_of AS (SELECT node, parent_states, p_yes FROM cz_cpt WHERE net = 'discount'),
s1 AS (SELECT i, if(size < 0.3, 'yes', 'no') AS size, engaged AS u_engaged, price AS u_price,
              discount AS u_discount, usage AS u_usage, stay AS u_stay FROM p),
s2 AS (SELECT s1.*, if(u_engaged < c.p_yes, 'yes', 'no') AS engaged
       FROM s1 JOIN p_of c ON c.node = 'engaged' AND c.parent_states = [size]),
s3 AS (SELECT *, if(u_price < 0.4, 'yes', 'no') AS price FROM s2),
s4 AS (SELECT s3.*, if(u_discount < c.p_yes, 'yes', 'no') AS discount
       FROM s3 JOIN p_of c ON c.node = 'discount' AND c.parent_states = [engaged]),
s5 AS (SELECT s4.*, if(u_usage < c.p_yes, 'yes', 'no') AS usage
       FROM s4 JOIN p_of c ON c.node = 'usage' AND c.parent_states = [discount, engaged])
SELECT s5.i, size, engaged, price, discount, usage, if(u_stay < c.p_yes, 'yes', 'no') AS stay
FROM s5 JOIN p_of c ON c.node = 'stay' AND c.parent_states = [engaged, discount, usage, price];

-- Weights per strata: b = 0 the sample, b = 1…100 Poisson bootstrap
-- resamples (each customer counted 0, 1, 2, … times), b = -1 the truth (the
-- network's exact joint probabilities).
CREATE OR REPLACE TEMP TABLE cz_strata AS
WITH boot AS (
  SELECT b.range AS b, s.*, (hash(s.i::VARCHAR || '#' || b.range) % 1000000) / 1e6 AS u
  FROM cz_sample s, range(1, 101) b
),
truth AS (
  SELECT "case", product(f) AS w FROM cz_factors WHERE net = 'discount' GROUP BY ALL
),
truth_cases AS (
  SELECT "case", any_value(state) FILTER (node = 'engaged') AS engaged, any_value(state) FILTER (node = 'price') AS price,
         any_value(state) FILTER (node = 'discount') AS discount, any_value(state) FILTER (node = 'usage') AS usage,
         any_value(state) FILTER (node = 'stay') AS stay
  FROM cz_cases WHERE net = 'discount' GROUP BY "case"
)
SELECT 0 AS b, engaged, price, discount, usage, stay, count(*)::DOUBLE AS w FROM cz_sample GROUP BY ALL
UNION ALL
SELECT b, engaged, price, discount, usage, stay,
       sum(CASE WHEN u < 0.3679 THEN 0 WHEN u < 0.7358 THEN 1 WHEN u < 0.9197 THEN 2
                WHEN u < 0.9810 THEN 3 ELSE 4 END)::DOUBLE
FROM boot GROUP BY ALL
UNION ALL
SELECT -1, t.engaged, t.price, t.discount, t.usage, t.stay, sum(w.w)
FROM truth w JOIN truth_cases t USING ("case") GROUP BY ALL;

-- Per stratum z = (engaged, price) — the optimal adjustment set — the
-- renewal rate with and without the discount (y1, y0), the weekly-use rate
-- (m1, m0) and the renewal rate per discount and weekly use (y11 … y00).
CREATE OR REPLACE TEMP VIEW cz_by_z AS
SELECT b, engaged, price,
       sum(w) / sum(sum(w)) OVER (PARTITION BY b) AS pz,
       sum(w) FILTER (discount = 'yes') / sum(sum(w) FILTER (discount = 'yes')) OVER (PARTITION BY b) AS pz_treated,
       sum(w) FILTER (discount = 'yes' AND stay = 'yes') / sum(w) FILTER (discount = 'yes') AS y1,
       sum(w) FILTER (discount = 'no' AND stay = 'yes') / sum(w) FILTER (discount = 'no') AS y0,
       sum(w) FILTER (discount = 'yes' AND usage = 'yes') / sum(w) FILTER (discount = 'yes') AS m1,
       sum(w) FILTER (discount = 'no' AND usage = 'yes') / sum(w) FILTER (discount = 'no') AS m0,
       sum(w) FILTER (discount = 'yes' AND usage = 'yes' AND stay = 'yes') / sum(w) FILTER (discount = 'yes' AND usage = 'yes') AS y11,
       sum(w) FILTER (discount = 'yes' AND usage = 'no' AND stay = 'yes') / sum(w) FILTER (discount = 'yes' AND usage = 'no') AS y10,
       sum(w) FILTER (discount = 'no' AND usage = 'yes' AND stay = 'yes') / sum(w) FILTER (discount = 'no' AND usage = 'yes') AS y01,
       sum(w) FILTER (discount = 'no' AND usage = 'no' AND stay = 'yes') / sum(w) FILTER (discount = 'no' AND usage = 'no') AS y00
FROM cz_strata GROUP BY b, engaged, price;

-- The effects per weight set, in percentage points of renewal:
--   ATE  average effect: Σz P(z) (y1 − y0)
--   CATE the average effect among the customers matching the evidence
--   ETT  the effect on those who got the discount: Σz P(z | treated) (y1 − y0)
--   NDE  natural direct: Σz P(z) Σm (y1m − y0m) P(m | no discount, z)
--   NIE  natural indirect (through weekly use): Σz P(z) (y11 − y10)(m1 − m0)
--   NDE + NIE = ATE (total = direct + indirect)
CREATE OR REPLACE TEMP VIEW cz_effect_draws AS
WITH e AS (
  SELECT b,
         100 * sum(pz * (y1 - y0)) AS ate,
         100 * sum(pz * (y1 - y0)) FILTER (engaged = 'yes') / sum(pz) FILTER (engaged = 'yes') AS cate_engaged_yes,
         100 * sum(pz * (y1 - y0)) FILTER (engaged = 'no') / sum(pz) FILTER (engaged = 'no') AS cate_engaged_no,
         100 * sum(pz * (y1 - y0)) FILTER (price = 'yes') / sum(pz) FILTER (price = 'yes') AS cate_price_yes,
         100 * sum(pz * (y1 - y0)) FILTER (price = 'no') / sum(pz) FILTER (price = 'no') AS cate_price_no,
         100 * sum(pz_treated * (y1 - y0)) AS ett,
         100 * sum(pz * ((y11 - y01) * m0 + (y10 - y00) * (1 - m0))) AS nde,
         100 * sum(pz * (y11 - y10) * (m1 - m0)) AS nie
  FROM cz_by_z GROUP BY b
)
SELECT b, unnest(['ate', 'cate_engaged_yes', 'cate_engaged_no', 'cate_price_yes', 'cate_price_no', 'ett', 'nde', 'nie']) AS effect,
       unnest([ate, cate_engaged_yes, cate_engaged_no, cate_price_yes, cate_price_no, ett, nde, nie]) AS pt
FROM e;

CREATE OR REPLACE TEMP TABLE cz_effects AS
SELECT effect,
       any_value(pt) FILTER (b = 0) AS estimate,
       quantile_cont(pt, 0.025) FILTER (b > 0) AS lo,
       quantile_cont(pt, 0.975) FILTER (b > 0) AS hi,
       any_value(pt) FILTER (b = -1) AS truth
FROM cz_effect_draws GROUP BY effect;

-- ─────────────────────────────────────────────────────────────────────────────
-- What if: one customer's factual and counterfactual worlds
-- ─────────────────────────────────────────────────────────────────────────────
-- A counterfactual needs more than the tables: how each node responds to
-- its parents, customer by customer. The network is read as a threshold
-- model: a node's hidden draw u (uniform) makes it yes when u < P(yes |
-- parents), the same u in both worlds. Only where u falls between the
-- table's distinct P(yes) values matters, so u is enumerated by interval
-- (its probability the interval's width). Abduction keeps the background
-- that fits what was seen; action sets the changed node; prediction runs the
-- network again from there.
CREATE OR REPLACE TEMP TABLE cz_u AS
WITH cuts AS (
  SELECT DISTINCT node, p_yes AS t FROM cz_cpt WHERE net = 'discount'
  UNION SELECT DISTINCT node, 1.0 FROM cz_cpt WHERE net = 'discount'
),
iv AS (SELECT node, coalesce(lag(t) OVER (PARTITION BY node ORDER BY t), 0) AS lo, t AS hi FROM cuts)
SELECT node, (lo + hi) / 2 AS mid, hi - lo AS pu FROM iv WHERE hi > lo;

-- the case (what was seen) and the change, as picked above the drawing
CREATE OR REPLACE TEMP VIEW cz_what_if AS
SELECT coalesce(nullif(getvariable('cz_case'), ''), 'discount=yes, stay=yes') AS seen,
       coalesce(nullif(getvariable('cz_change'), ''), 'discount=no') AS change;

CREATE OR REPLACE TEMP VIEW cz_worlds AS
WITH p_of AS (SELECT node, parent_states, p_yes FROM cz_cpt WHERE net = 'discount'),
bg AS (
  SELECT s.mid AS u_size, e.mid AS u_engaged, r.mid AS u_price, d.mid AS u_discount,
         m.mid AS u_usage, y.mid AS u_stay, s.pu * e.pu * r.pu * d.pu * m.pu * y.pu AS w
  FROM cz_u s, cz_u e, cz_u r, cz_u d, cz_u m, cz_u y
  WHERE s.node = 'size' AND e.node = 'engaged' AND r.node = 'price' AND d.node = 'discount'
    AND m.node = 'usage' AND y.node = 'stay'
),
ch AS (SELECT split_part(change, '=', 1) AS cnode, split_part(change, '=', 2) AS cstate FROM cz_what_if),
-- the factual world: the network as it ran
f1 AS (SELECT bg.*, if(u_size < 0.3, 'yes', 'no') AS size, if(u_price < 0.4, 'yes', 'no') AS price FROM bg),
f2 AS (SELECT f1.*, if(u_engaged < c.p_yes, 'yes', 'no') AS engaged FROM f1 JOIN p_of c ON c.node = 'engaged' AND c.parent_states = [size]),
f3 AS (SELECT f2.*, if(u_discount < c.p_yes, 'yes', 'no') AS discount FROM f2 JOIN p_of c ON c.node = 'discount' AND c.parent_states = [engaged]),
f4 AS (SELECT f3.*, if(u_usage < c.p_yes, 'yes', 'no') AS usage FROM f3 JOIN p_of c ON c.node = 'usage' AND c.parent_states = [discount, engaged]),
f5 AS (SELECT f4.*, if(u_stay < c.p_yes, 'yes', 'no') AS stay FROM f4 JOIN p_of c ON c.node = 'stay' AND c.parent_states = [engaged, discount, usage, price]),
-- the counterfactual world: the same background, the changed node set, the
-- nodes after it run again
c1 AS (SELECT f5.*, if(ch.cnode = 'discount', ch.cstate, discount) AS cf_discount FROM f5, ch),
c2 AS (SELECT c1.*, if(ch.cnode = 'usage', ch.cstate, if(u_usage < c.p_yes, 'yes', 'no')) AS cf_usage
       FROM c1, ch JOIN p_of c ON c.node = 'usage' AND c.parent_states = [cf_discount, engaged]),
c3 AS (SELECT c2.*, if(u_stay < c.p_yes, 'yes', 'no') AS cf_stay
       FROM c2 JOIN p_of c ON c.node = 'stay' AND c.parent_states = [engaged, cf_discount, cf_usage, price])
SELECT * FROM c3;

-- the seen states as rows (node, state)
CREATE OR REPLACE TEMP VIEW cz_seen AS
SELECT split_part(trim(x), '=', 1) AS node, split_part(trim(x), '=', 2) AS state
FROM cz_what_if, unnest(string_split(seen, ',')) t(x);

-- the twin's beliefs: each world given what was seen (the factual world's
-- prior unconditional, the counterfactual's under the change alone)
CREATE OR REPLACE TEMP VIEW cz_twin AS
WITH long AS (
  SELECT w, fit, 'factual' AS world, node, state FROM (
    SELECT w, (SELECT bool_and(CASE s.node WHEN 'discount' THEN c.discount = s.state WHEN 'stay' THEN c.stay = s.state
                                           WHEN 'usage' THEN c.usage = s.state WHEN 'engaged' THEN c.engaged = s.state
                                           ELSE true END) FROM cz_seen s) AS fit,
           unnest(['size', 'engaged', 'price', 'discount', 'usage', 'stay']) AS node,
           unnest([size, engaged, price, discount, usage, stay]) AS state
    FROM cz_worlds c
  )
  UNION ALL
  SELECT w, fit, 'counterfactual', node, state FROM (
    SELECT w, (SELECT bool_and(CASE s.node WHEN 'discount' THEN c.discount = s.state WHEN 'stay' THEN c.stay = s.state
                                           WHEN 'usage' THEN c.usage = s.state WHEN 'engaged' THEN c.engaged = s.state
                                           ELSE true END) FROM cz_seen s) AS fit,
           unnest(['size', 'engaged', 'price', 'discount', 'usage', 'stay']) AS node,
           unnest([size, engaged, price, cf_discount, cf_usage, cf_stay]) AS state
    FROM cz_worlds c
  )
),
agg AS (
  SELECT world, node, state, sum(w) FILTER (fit) AS w_fit, sum(w) AS w_all
  FROM long GROUP BY ALL
),
-- every state of every node in both worlds (a state no case reaches: 0)
grid AS (
  SELECT w.world, n.id AS node, s.state
  FROM cz_nodes n, unnest(n.states) s(state), (VALUES ('factual'), ('counterfactual')) w(world)
  WHERE n.net = 'discount'
)
SELECT g.world, g.node, g.state,
       coalesce(a.w_fit, 0) / sum(coalesce(a.w_fit, 0)) OVER (PARTITION BY g.world, g.node) AS probability,
       coalesce(a.w_all, 0) / sum(coalesce(a.w_all, 0)) OVER (PARTITION BY g.world, g.node) AS prior
FROM grid g LEFT JOIN agg a USING (world, node, state);

-- the twin's evidence: what was seen (factual), the change (do)
CREATE OR REPLACE TEMP VIEW cz_twin_evidence AS
SELECT node, state, 'observe' AS mode FROM cz_seen
UNION ALL
SELECT split_part(change, '=', 1), split_part(change, '=', 2), 'do' FROM cz_what_if;

-- ─────────────────────────────────────────────────────────────────────────────
-- A structure learner's arcs
-- ─────────────────────────────────────────────────────────────────────────────
-- strength: how often the learner found the arc in bootstrap resamples
-- (written by hand here). The reference is the network above.
CREATE OR REPLACE TEMP TABLE cz_learned AS
FROM (VALUES
  ('size', 'engaged', 0.97),
  ('engaged', 'discount', 0.94),
  ('discount', 'usage', 0.88),
  ('engaged', 'usage', 0.71),
  ('engaged', 'stay', 0.79),
  ('price', 'stay', 0.58),
  ('stay', 'usage', 0.66),
  ('discount', 'stay', 0.31),
  ('price', 'usage', 0.55),
  ('size', 'discount', 0.42)
) t("from", "to", strength);

CREATE OR REPLACE TEMP VIEW cz_threshold AS
SELECT coalesce(try_cast(getvariable('cz_learn')->>'threshold' AS DOUBLE), 0.5) AS t;

-- the learned arcs at the threshold against the reference: matching, extra,
-- missing, reversed; the structural Hamming distance counts all but matching
CREATE OR REPLACE TEMP VIEW cz_diff AS
WITH l AS (SELECT "from", "to" FROM cz_learned, cz_threshold WHERE strength >= t),
r AS (SELECT "from", "to" FROM cz_arcs WHERE net = 'discount')
SELECT l."from", l."to",
       CASE WHEN EXISTS (SELECT 1 FROM r WHERE r."from" = l."from" AND r."to" = l."to") THEN 'matching'
            WHEN EXISTS (SELECT 1 FROM r WHERE r."from" = l."to" AND r."to" = l."from") THEN 'reversed'
            ELSE 'extra' END AS kind
FROM l
UNION ALL
SELECT r."from", r."to", 'missing' FROM r
WHERE NOT EXISTS (SELECT 1 FROM l WHERE (l."from" = r."from" AND l."to" = r."to") OR (l."from" = r."to" AND l."to" = r."from"));

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Does the discount keep customers?'::DASHBOARD_TITLE;

-- ── 1 · Seeing or doing ──────────────────────────────────────────────────────
SELECT 'Seeing or doing'::TAB;

SELECT 'At a glance'::GROUP;
SELECT round(100 * probability, 1)::PERCENT, 'renew among customers who got the discount (seeing)'::LABEL
FROM cz_seeing_doing WHERE node = 'discount' AND value = 'yes' AND mode = 'observe';
SELECT round(100 * probability, 1)::PERCENT, 'would renew if we gave everyone the discount (doing)'::LABEL
FROM cz_seeing_doing WHERE node = 'discount' AND value = 'yes' AND mode = 'do';
SELECT round(100 * (s.probability - n.probability) - 100 * (d.probability - x.probability), 1)::METRIC,
       'points of the seen gap that are confounding, not the discount'::LABEL
FROM cz_seeing_doing s, cz_seeing_doing n, cz_seeing_doing d, cz_seeing_doing x
WHERE s.node = 'discount' AND s.value = 'yes' AND s.mode = 'observe'
  AND n.node = 'discount' AND n.value = 'no' AND n.mode = 'observe'
  AND d.node = 'discount' AND d.value = 'yes' AND d.mode = 'do'
  AND x.node = 'discount' AND x.value = 'no' AND x.mode = 'do';
SELECT 1::ENDGROUP;

SELECT node AS cz_node ::BUTTONS, 'Seeing or doing for'::LABEL, label::OPTION_LABEL
FROM (VALUES ('discount', 'the discount', 1), ('usage', 'weekly use', 2)) t(node, label, o) ORDER BY o;

SELECT 4::SPAN;
SELECT '**The question.** Customers who got the renewal discount renewed more often. Does the discount keep them, or does sales just give it to customers who would renew anyway?

**Seeing or doing.** *Seeing* compares the customers who got the discount with those who did not. *Doing* asks what happens if we give it: the network sets the node (do) and cuts its causes. The shaded gap between the two lines is confounding.

**The drawing.** **X** is the treatment, **Y** the outcome; the ringed nodes are the adjustment set. A dashed arc is on a back-door path, a way for the two to hang together that does not run through the treatment; red when open, a bar where the set blocks it. Pick another set above the drawing, or **none** to see the path open.

**Try it.** Click **yes** on *Got the discount*: renewal rises a lot. Switch it to **do** in its chip: much less.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 360::HEIGHT;
SELECT CASE value WHEN 'yes' THEN 'with' ELSE 'without' END AS "Treatment" ::XAXIS,
       100 * max(probability) FILTER (mode = 'observe') AS "seeing: renewal rate where it happened" ::LINECHART,
       100 * max(probability) FILTER (mode = 'do') AS "doing: renewal rate if we made it happen" ::LINECHART,
       100 * least(max(probability) FILTER (mode = 'observe'), max(probability) FILTER (mode = 'do')) ::BAND_LOWER,
       100 * greatest(max(probability) FILTER (mode = 'observe'), max(probability) FILTER (mode = 'do')) ::BAND_UPPER,
       CASE WHEN value = 'yes' THEN printf('confounding: %+.0f points',
         100 * (max(probability) FILTER (mode = 'observe') - max(probability) FILTER (mode = 'do'))) END ::ANNOTATE,
       'percent'::YFORMAT,
       'Seeing or doing: P(renewed | X) against P(renewed | do(X))'::TITLE,
       'The shaded gap is confounding: what seeing adds that doing does not'::SUBTITLE
FROM cz_seeing_doing
WHERE net = 'discount' AND node = coalesce(nullif(getvariable('cz_node'), ''), 'discount')
GROUP BY value ORDER BY value = 'yes';

SELECT 'Which customers to compare'::SECTION;

SELECT 4::SPAN;
SELECT set_id AS cz_set ::DROPDOWN, 'Adjust for'::LABEL, label::OPTION_LABEL
FROM (VALUES ('minimal', 'minimal set: Engaged customer', 1),
             ('optimal', 'optimal set: Engaged customer, Price rose', 2),
             ('canonical', 'canonical set: Large account, Engaged customer, Price rose', 3),
             ('none', 'nothing (compare all customers)', 4)) t(set_id, label, o)
ORDER BY o;
SELECT '**Adjusting.** Comparing customers *with the same engagement* blocks the back-door path Got the discount ← Engaged customer → Renewed: within each group, sales did not pick by engagement. Every listed set blocks it; the optimal one also takes *Price rose*, which only affects renewal, for a tighter estimate.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 460::HEIGHT;
SELECT 'cz_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM cz_arcs a WHERE a.net = 'discount'),
         beliefs := (SELECT list(b) FROM cz_beliefs b WHERE b.net = 'discount'),
         evidence := (SELECT list(e) FROM cz_evidence e WHERE e.net = 'discount'),
         tables := (SELECT list(t) FROM cz_tables t WHERE t.net = 'discount'),
         adjust := (SELECT list(a) FROM cz_adjust a
                    WHERE a.net = 'discount' AND a.set_id = coalesce(nullif(getvariable('cz_set'), ''), 'minimal')))
       AS "From the discount to renewal: which paths to block"
FROM cz_nodes n
WHERE n.net = 'discount';

SELECT 4::SPAN;
SELECT 'Seeing, the front door, the truth'::GROUP;
SELECT round(100 * (sum(p.p) FILTER (s.node = 'discount' AND s.state = 'yes' AND s.mode = 'observe')
                  - sum(p.p) FILTER (s.node = 'discount' AND s.state = 'no' AND s.mode = 'observe')), 1)::METRIC,
       'points: seeing (renewal with against without the discount)'::LABEL
FROM (SELECT DISTINCT net, scen, node, state, mode FROM cz_scenarios WHERE net = 'hidden' AND scen LIKE '%:%') s
JOIN cz_posterior p ON p.net = 'hidden' AND p.scen = s.scen AND p.node = 'stay' AND p.state = 'yes';
-- the front-door formula from the measured nodes' joint: Σm P(m | x)
-- Σx' P(renewed | m, x') P(x'), for x = yes and x = no
SELECT round(100 * (max(fd) FILTER (x = 'yes') - max(fd) FILTER (x = 'no')), 1)::METRIC,
       'points: the front-door estimate (doing), from discount, weekly use and renewal alone'::LABEL
FROM (
  WITH j AS (
    SELECT c."case", any_value(w.w) AS w,
           any_value(c.state) FILTER (c.node = 'discount') AS discount, any_value(c.state) FILTER (c.node = 'usage') AS usage,
           any_value(c.state) FILTER (c.node = 'stay') AS stay
    FROM cz_cases c
    JOIN (SELECT "case", product(f) AS w FROM cz_factors WHERE net = 'hidden' GROUP BY ALL) w USING ("case")
    WHERE c.net = 'hidden' GROUP BY c."case"
  ),
  pm AS (SELECT discount AS x, usage AS m, sum(w) / sum(sum(w)) OVER (PARTITION BY discount) AS p FROM j GROUP BY discount, usage),
  px AS (SELECT discount AS x, sum(w) AS p FROM j GROUP BY discount),
  py AS (SELECT discount AS x, usage AS m, sum(w) FILTER (stay = 'yes') / sum(w) AS p FROM j GROUP BY discount, usage),
  inner_m AS (SELECT py.m, sum(py.p * px.p) AS v FROM py JOIN px USING (x) GROUP BY py.m)
  SELECT pm.x, sum(pm.p * i.v) AS fd
  FROM pm JOIN inner_m i USING (m) GROUP BY pm.x
);
SELECT round(100 * (sum(p.p) FILTER (s.node = 'discount' AND s.state = 'yes' AND s.mode = 'do')
                  - sum(p.p) FILTER (s.node = 'discount' AND s.state = 'no' AND s.mode = 'do')), 1)::METRIC,
       'points: the true effect, do() on the whole network'::LABEL
FROM (SELECT DISTINCT net, scen, node, state, mode FROM cz_scenarios WHERE net = 'hidden' AND scen LIKE '%:%') s
JOIN cz_posterior p ON p.net = 'hidden' AND p.scen = s.scen AND p.node = 'stay' AND p.state = 'yes';
SELECT 1::ENDGROUP;

SELECT 4::SPAN;
SELECT '**Front door.** Here engagement is not measured: no set can block the back-door path, so it stays red. But the discount acts on renewal only through weekly use (**M**, a front-door mediator), and nothing unmeasured drives weekly use but the discount. The front-door formula then recovers the effect from the three measured nodes: the same as setting the discount on the whole network.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 400::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM cz_arcs a WHERE a.net = 'hidden'),
         beliefs := (SELECT list(b) FROM cz_beliefs b WHERE b.net = 'hidden'),
         tables := (SELECT list(t) FROM cz_tables t WHERE t.net = 'hidden'),
         adjust := (SELECT list(a) FROM cz_adjust a WHERE a.net = 'hidden'))
       AS "Engagement not measured: the front door through weekly use"
FROM cz_nodes n
WHERE n.net = 'hidden';

-- ── 2 · How big is the effect ────────────────────────────────────────────────
SELECT 'How big is the effect'::TAB;

SELECT ev AS cz_given ::DROPDOWN, 'CATE: the effect among customers who are'::LABEL, label::OPTION_LABEL
FROM (VALUES ('cate_engaged_yes', 'engaged', 1), ('cate_engaged_no', 'not engaged', 2),
             ('cate_price_yes', 'after a price rise', 3), ('cate_price_no', 'without a price rise', 4)) t(ev, label, o)
ORDER BY o;

SELECT 'Effect of the discount on renewal (points)'::GROUP;
SELECT round(estimate, 1)::METRIC, 'ATE: on average · 95 % ' || printf('%+.1f to %+.1f', lo, hi) || ' · true ' || printf('%+.1f', truth)::LABEL
FROM cz_effects WHERE effect = 'ate';
SELECT round(estimate, 1)::METRIC, 'CATE: given the pick · 95 % ' || printf('%+.1f to %+.1f', lo, hi) || ' · true ' || printf('%+.1f', truth)::LABEL
FROM cz_effects WHERE effect = coalesce(nullif(getvariable('cz_given'), ''), 'cate_engaged_yes');
SELECT round(estimate, 1)::METRIC, 'ETT: on those who got it · 95 % ' || printf('%+.1f to %+.1f', lo, hi) || ' · true ' || printf('%+.1f', truth)::LABEL
FROM cz_effects WHERE effect = 'ett';
SELECT round(estimate, 1)::METRIC, 'direct (NDE) + ' || printf('%.1f', (SELECT estimate FROM cz_effects WHERE effect = 'nie')) || ' indirect (NIE) = ' || printf('%.1f', (SELECT estimate FROM cz_effects WHERE effect = 'ate')) || ' total (ATE)'::LABEL
FROM cz_effects WHERE effect = 'nde';
SELECT 1::ENDGROUP;

SELECT 6::SPAN;
SELECT 340::HEIGHT;
SELECT CASE effect WHEN 'ate' THEN 'ATE: average' WHEN 'ett' THEN 'ETT: on the treated'
                   WHEN 'nde' THEN 'NDE: direct' WHEN 'nie' THEN 'NIE: through weekly use'
                   ELSE 'CATE: given the pick' END AS "Effect" ::XAXIS,
       estimate AS "estimate (95 % interval)" ::SCATTER, lo::YMIN, hi::YMAX,
       truth AS "true value (from the network)" ::SCATTER,
       0::REFLINE, 1::FLIP,
       'Effects with 95 % intervals (points of renewal)'::TITLE,
       'Estimated from 8,000 customers, adjusting for engagement and the price rise; 100 bootstrap resamples'::SUBTITLE
FROM cz_effects
WHERE effect IN ('ate', 'ett', 'nde', 'nie') OR effect = coalesce(nullif(getvariable('cz_given'), ''), 'cate_engaged_yes')
ORDER BY CASE effect WHEN 'ate' THEN 1 WHEN 'ett' THEN 3 WHEN 'nde' THEN 4 WHEN 'nie' THEN 5 ELSE 2 END;

SELECT 6::SPAN;
SELECT 340::HEIGHT;
SELECT part AS "Part" ::XAXIS, pt::WATERFALL, 'Total = direct + indirect (points of renewal)'::TITLE,
       'Direct: the discount itself; indirect: the extra weekly use it brings'::SUBTITLE
FROM (
  SELECT 1 AS o, 'direct (NDE)' AS part, estimate AS pt FROM cz_effects WHERE effect = 'nde'
  UNION ALL SELECT 2, 'indirect (NIE)', estimate FROM cz_effects WHERE effect = 'nie'
  UNION ALL SELECT 3, 'total (ATE)', NULL
) ORDER BY o;

-- ── 3 · What if ──────────────────────────────────────────────────────────────
SELECT 'What if'::TAB;

SELECT seen AS cz_case ::DROPDOWN, 'This customer'::LABEL, label::OPTION_LABEL
FROM (VALUES ('discount=yes, stay=yes', 'got the discount and renewed', 1),
             ('discount=yes, stay=no', 'got the discount and still left', 2),
             ('discount=no, stay=no', 'got no discount and left', 3)) t(seen, label, o)
ORDER BY o;
SELECT change AS cz_change ::DROPDOWN, 'What if they had'::LABEL, label::OPTION_LABEL
FROM (VALUES ('discount=no', 'not got the discount', 1), ('discount=yes', 'got the discount', 2),
             ('usage=no', 'not used it weekly', 3), ('usage=yes', 'used it weekly', 4)) t(change, label, o)
ORDER BY o;
SELECT 4::SPAN;
SELECT round(100 * probability, 1)::PERCENT, 'would have renewed in the counterfactual world'::LABEL
FROM cz_twin WHERE world = 'counterfactual' AND node = 'stay' AND state = 'yes';
SELECT 8::SPAN;
SELECT '**Two worlds.** Left, the factual world: what happened to this customer. Right, the counterfactual world: the same customer, the change made. In the middle, what the change does not reach — the customer''s size, engagement and the price — shared by both. The hand marks the changed node; its arcs from the background are cut.

**Reading it.** A customer who got the discount and renewed was mostly engaged: without the discount most would still have renewed. The discount was necessary only for the rest.'::MARKDOWN;

SELECT 12::SPAN;
SELECT 540::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM cz_arcs a WHERE a.net = 'discount'),
         beliefs := (SELECT list(b) FROM cz_twin b),
         evidence := (SELECT list(e) FROM cz_twin_evidence e),
         tables := (SELECT list(t) FROM cz_tables t WHERE t.net = 'discount'))
       AS "This customer, as it happened and as it would have been"
FROM cz_nodes n
WHERE n.net = 'discount';

-- ── 4 · Did a learner find it ────────────────────────────────────────────────
SELECT 'Did a learner find it'::TAB;

SELECT 'Learned against the true network'::GROUP;
SELECT count(*) FILTER (kind <> 'matching')::METRIC, 'structural Hamming distance (extra + missing + reversed)'::LABEL FROM cz_diff;
SELECT count(*) FILTER (kind = 'matching')::METRIC, 'arcs matching'::LABEL FROM cz_diff;
SELECT 1::ENDGROUP;

SELECT 4::SPAN;
SELECT field AS cz_learn ::FORM, type, value, min, max, step, label, help
FROM (VALUES ('threshold', 'slider', '0.5', 0.0, 1.0, 0.05, 'Keep arcs found in at least this share of resamples',
              'A learner''s arc strength: how often it found the arc in bootstrap resamples.')) t(field, type, value, min, max, step, label, help);
SELECT 8::SPAN;
SELECT '**Reading it.** A structure learner found most of the network: the arcs it got right are grey, the reversed one blue, an extra one amber, and the arc it missed is dashed red. Lower the threshold and the weak *Got the discount → Renewed* arc comes in — with an extra one; raise it and arcs drop out.'::MARKDOWN;


SELECT 12::SPAN;
SELECT 460::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(l) FROM cz_learned l),
         reference := (SELECT list(a ORDER BY a.k) FROM cz_arcs a WHERE a.net = 'discount'),
         threshold := (SELECT t FROM cz_threshold))
       AS "The learned network against the true one"
FROM (SELECT id, label, kind FROM cz_nodes WHERE net = 'discount') n;
