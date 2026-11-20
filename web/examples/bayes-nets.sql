-- @title What does the evidence say about the cause?
-- @group How it works
-- @tags  diagram, bayesian network, bayesian, evidence, intervention
-- @description Two Bayesian networks you query by clicking: why the grass is wet, and what a breathless patient with an abnormal X-ray has.
--
-- What does the evidence say about the cause? Two classic Bayesian networks,
-- as an inference engine writes them (nodes, arcs, beliefs, evidence, the
-- conditional probability tables and the active paths), drawn with
-- anofox_plot_bayes_net (#1275) and queried by clicking (#1276). Each node
-- shows its belief per state as bars, the prior (before any evidence) as a
-- faint bar behind them.
--
-- Click a state to observe it, click it again to retract it; shift-click a
-- node for soft evidence (a likelihood per state). The chip above the graph
-- switches an observation to an intervention, do(): the node is set, its
-- incoming arcs are cut. Each click is a dashboard input,
-- getvariable('sprinkler_ev') is the evidence as JSON text, and the SQL below
-- recomputes the beliefs from it: exact, by enumerating every case of the
-- network. getvariable('sprinkler_ev_prev') is the evidence before the last
-- click, so each bar also shows how far that click moved it.
--
-- Hover a node to light its Markov blanket (parents, children, the
-- children's other parents). Click a node's name, then hover another: the
-- paths between them that are open given the evidence (d-separation) light
-- up; the SQL finds them too. The arc strengths of the chest clinic are how
-- often a test found each arc in 200 resamples of 1,000 simulated patients
-- (a chi-square test at 5 %, given the child's other parents).

-- Bayesian networks are a plugin (#1275): a build without it says so.
SELECT 'bayes-net'::REQUIRES;

-- The two networks. The sprinkler: a cloudy day makes rain likelier and the
-- sprinkler less likely; either wets the grass. The chest clinic ("Asia",
-- Lauritzen & Spiegelhalter 1988): a visit to Asia and smoking raise the
-- risk of tuberculosis, lung cancer and bronchitis; an X-ray cannot tell
-- tuberculosis from cancer, shortness of breath comes with any of them.
CREATE OR REPLACE TEMP TABLE bn_nodes AS
FROM (VALUES
  ('sprinkler', 'cloudy', 'Cloudy', 'chance', ['yes', 'no'], NULL),
  ('sprinkler', 'sprinkler', 'Sprinkler on', 'chance', ['yes', 'no'], NULL),
  ('sprinkler', 'rain', 'Rain', 'chance', ['yes', 'no'], NULL),
  ('sprinkler', 'grass', 'Wet grass', 'chance', ['yes', 'no'], NULL),
  ('asia', 'asia', 'Visited Asia', 'chance', ['yes', 'no'], 'history'),
  ('asia', 'smoke', 'Smoker', 'chance', ['yes', 'no'], 'history'),
  ('asia', 'tub', 'Tuberculosis', 'chance', ['yes', 'no'], 'disease'),
  ('asia', 'lung', 'Lung cancer', 'chance', ['yes', 'no'], 'disease'),
  ('asia', 'bronc', 'Bronchitis', 'chance', ['yes', 'no'], 'disease'),
  ('asia', 'either', 'TB or cancer', 'deterministic', ['yes', 'no'], 'disease'),
  ('asia', 'xray', 'Abnormal X-ray', 'chance', ['yes', 'no'], 'finding'),
  ('asia', 'dysp', 'Short of breath', 'chance', ['yes', 'no'], 'finding')
) t(net, id, label, kind, states, "group");

-- the arcs, a node's parents in the order of its table's parent_states
CREATE OR REPLACE TEMP TABLE bn_arcs AS
FROM (VALUES
  ('sprinkler', 1, 'cloudy', 'sprinkler', NULL),
  ('sprinkler', 2, 'cloudy', 'rain', NULL),
  ('sprinkler', 3, 'sprinkler', 'grass', NULL),
  ('sprinkler', 4, 'rain', 'grass', NULL),
  ('asia', 1, 'asia', 'tub', 0.0),
  ('asia', 2, 'smoke', 'lung', 1.0),
  ('asia', 3, 'smoke', 'bronc', 1.0),
  ('asia', 4, 'tub', 'either', 1.0),
  ('asia', 5, 'lung', 'either', 1.0),
  ('asia', 6, 'either', 'xray', 1.0),
  ('asia', 7, 'bronc', 'dysp', 1.0),
  ('asia', 8, 'either', 'dysp', 1.0)
) t(net, k, "from", "to", strength);

CREATE OR REPLACE TEMP TABLE bn_tables AS
FROM (VALUES
  ('sprinkler', 'cloudy', []::VARCHAR[], 'yes', 0.5),
  ('sprinkler', 'cloudy', []::VARCHAR[], 'no', 0.5),
  ('sprinkler', 'sprinkler', ['yes'], 'yes', 0.1),
  ('sprinkler', 'sprinkler', ['yes'], 'no', 0.9),
  ('sprinkler', 'sprinkler', ['no'], 'yes', 0.5),
  ('sprinkler', 'sprinkler', ['no'], 'no', 0.5),
  ('sprinkler', 'rain', ['yes'], 'yes', 0.8),
  ('sprinkler', 'rain', ['yes'], 'no', 0.2),
  ('sprinkler', 'rain', ['no'], 'yes', 0.2),
  ('sprinkler', 'rain', ['no'], 'no', 0.8),
  ('sprinkler', 'grass', ['yes', 'yes'], 'yes', 0.99),
  ('sprinkler', 'grass', ['yes', 'yes'], 'no', 0.01),
  ('sprinkler', 'grass', ['yes', 'no'], 'yes', 0.9),
  ('sprinkler', 'grass', ['yes', 'no'], 'no', 0.1),
  ('sprinkler', 'grass', ['no', 'yes'], 'yes', 0.9),
  ('sprinkler', 'grass', ['no', 'yes'], 'no', 0.1),
  ('sprinkler', 'grass', ['no', 'no'], 'yes', 0.0),
  ('sprinkler', 'grass', ['no', 'no'], 'no', 1.0),
  ('asia', 'asia', []::VARCHAR[], 'yes', 0.01),
  ('asia', 'asia', []::VARCHAR[], 'no', 0.99),
  ('asia', 'smoke', []::VARCHAR[], 'yes', 0.5),
  ('asia', 'smoke', []::VARCHAR[], 'no', 0.5),
  ('asia', 'tub', ['yes'], 'yes', 0.05),
  ('asia', 'tub', ['yes'], 'no', 0.95),
  ('asia', 'tub', ['no'], 'yes', 0.01),
  ('asia', 'tub', ['no'], 'no', 0.99),
  ('asia', 'lung', ['yes'], 'yes', 0.1),
  ('asia', 'lung', ['yes'], 'no', 0.9),
  ('asia', 'lung', ['no'], 'yes', 0.01),
  ('asia', 'lung', ['no'], 'no', 0.99),
  ('asia', 'bronc', ['yes'], 'yes', 0.6),
  ('asia', 'bronc', ['yes'], 'no', 0.4),
  ('asia', 'bronc', ['no'], 'yes', 0.3),
  ('asia', 'bronc', ['no'], 'no', 0.7),
  ('asia', 'either', ['yes', 'yes'], 'yes', 1.0),
  ('asia', 'either', ['yes', 'yes'], 'no', 0.0),
  ('asia', 'either', ['yes', 'no'], 'yes', 1.0),
  ('asia', 'either', ['yes', 'no'], 'no', 0.0),
  ('asia', 'either', ['no', 'yes'], 'yes', 1.0),
  ('asia', 'either', ['no', 'yes'], 'no', 0.0),
  ('asia', 'either', ['no', 'no'], 'yes', 0.0),
  ('asia', 'either', ['no', 'no'], 'no', 1.0),
  ('asia', 'xray', ['yes'], 'yes', 0.98),
  ('asia', 'xray', ['yes'], 'no', 0.02),
  ('asia', 'xray', ['no'], 'yes', 0.05),
  ('asia', 'xray', ['no'], 'no', 0.95),
  ('asia', 'dysp', ['yes', 'yes'], 'yes', 0.9),
  ('asia', 'dysp', ['yes', 'yes'], 'no', 0.1),
  ('asia', 'dysp', ['yes', 'no'], 'yes', 0.8),
  ('asia', 'dysp', ['yes', 'no'], 'no', 0.2),
  ('asia', 'dysp', ['no', 'yes'], 'yes', 0.7),
  ('asia', 'dysp', ['no', 'yes'], 'no', 0.3),
  ('asia', 'dysp', ['no', 'no'], 'yes', 0.1),
  ('asia', 'dysp', ['no', 'no'], 'no', 0.9)
) t(net, node, parent_states, state, probability);

-- Every case of a network (2^4 and 2^8 of them), one row per node: case c
-- gives node k (with m states) the state (c / stride) % m, stride the
-- product of the state counts before it.
CREATE OR REPLACE TEMP TABLE bn_cases AS
WITH n AS (
  SELECT net, id, states, len(states) AS m,
         coalesce(product(len(states)) OVER (PARTITION BY net ORDER BY id
           ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), 1)::BIGINT AS stride,
         product(len(states)) OVER (PARTITION BY net)::BIGINT AS total
  FROM bn_nodes
)
SELECT n.net, c.range AS "case", n.id AS node, n.states[(c.range // n.stride) % n.m + 1] AS state
FROM (SELECT DISTINCT net, total FROM n) t, range(t.total) c, n
WHERE n.net = t.net;

-- Each node's factor in each case: its table's probability of its state
-- given its parents' states (in the order of its arcs).
CREATE OR REPLACE TEMP TABLE bn_factors AS
WITH parents AS (
  SELECT c.net, c."case", a."to" AS node, list(c.state ORDER BY a.k) AS parent_states
  FROM bn_arcs a JOIN bn_cases c ON c.net = a.net AND c.node = a."from"
  GROUP BY ALL
)
SELECT c.net, c."case", c.node, c.state, t.probability AS f
FROM bn_cases c
LEFT JOIN parents p USING (net, "case", node)
JOIN bn_tables t ON t.net = c.net AND t.node = c.node AND t.state = c.state
  AND t.parent_states = coalesce(p.parent_states, []::VARCHAR[]);

-- The prior: every belief before any evidence.
CREATE OR REPLACE TEMP TABLE bn_prior AS
WITH w AS (SELECT net, "case", product(f) AS w FROM bn_factors GROUP BY ALL)
SELECT c.net, c.node, c.state, sum(w.w) AS prior
FROM w JOIN bn_cases c USING (net, "case")
GROUP BY ALL;

-- The evidence, as the clicks left it ('now') and before the last click
-- ('before'): a JSON list of {node, state[, likelihood][, mode]} each.
CREATE OR REPLACE TEMP VIEW bn_evidence AS
WITH v AS (
  SELECT 'sprinkler' AS net, 'now' AS which, getvariable('sprinkler_ev') AS j
  UNION ALL SELECT 'sprinkler', 'before', getvariable('sprinkler_ev_prev')
  UNION ALL SELECT 'asia', 'now', getvariable('asia_ev')
  UNION ALL SELECT 'asia', 'before', getvariable('asia_ev_prev')
)
SELECT net, which, e.node, e.state, e.likelihood, coalesce(lower(e.mode), 'observe') AS mode
FROM (
  SELECT net, which, unnest(from_json(coalesce(nullif(j, ''), '[]'),
    '[{"node": "VARCHAR", "state": "VARCHAR", "likelihood": "DOUBLE", "mode": "VARCHAR"}]')) AS e
  FROM v
);

-- The beliefs given the evidence, now and before the last click. A case
-- weighs the product of its factors; an observed node keeps the cases with
-- its state, soft evidence weighs each by the state's likelihood; do() sets
-- the node: its own factor drops out (its parents no longer act on it) and
-- only the cases with the set state are kept.
CREATE OR REPLACE TEMP VIEW bn_beliefs AS
WITH k AS (SELECT DISTINCT net, which FROM bn_nodes, (VALUES ('now'), ('before')) v(which)),
on_node AS (
  SELECT net, which, node, bool_or(mode = 'do') AS is_do FROM bn_evidence GROUP BY ALL
),
lik AS (
  SELECT net, which, node, state, sum(coalesce(likelihood, 1.0)) AS lik FROM bn_evidence GROUP BY ALL
),
w AS (
  SELECT k.net, k.which, f."case",
         product(CASE WHEN o.is_do THEN 1.0 ELSE f.f END
                 * CASE WHEN o.node IS NULL THEN 1.0 ELSE coalesce(l.lik, 0.0) END) AS w
  FROM bn_factors f
  JOIN k ON k.net = f.net
  LEFT JOIN on_node o ON o.net = k.net AND o.which = k.which AND o.node = f.node
  LEFT JOIN lik l ON l.net = k.net AND l.which = k.which AND l.node = f.node AND l.state = f.state
  GROUP BY ALL
),
m AS (
  SELECT w.net, w.which, c.node, c.state,
         sum(w.w) / nullif(sum(sum(w.w)) OVER (PARTITION BY w.net, w.which, c.node), 0) AS p
  FROM w JOIN bn_cases c ON c.net = w.net AND c."case" = w."case"
  GROUP BY w.net, w.which, c.node, c.state
)
SELECT now.net, now.node, now.state, now.p AS probability, pr.prior, was.p AS previous
FROM m now
JOIN m was ON was.which = 'before' AND was.net = now.net AND was.node = now.node AND was.state = now.state
JOIN bn_prior pr ON pr.net = now.net AND pr.node = now.node AND pr.state = now.state
WHERE now.which = 'now';

-- The active paths (d-separation) between every two nodes, given the
-- evidence: every trail along the arcs (either way, no node twice) in the
-- graph do() leaves (an intervened node's incoming arcs cut), kept unless a
-- node on it blocks it — a node in the middle of a chain or a fork blocks
-- when observed or set; a collider (both arcs pointing in) blocks unless it
-- or one of its descendants has evidence. One row per step.
CREATE OR REPLACE TEMP VIEW bn_paths AS
WITH RECURSIVE
ev AS (SELECT * FROM bn_evidence WHERE which = 'now'),
g AS (
  SELECT a.net, a."from" AS u, a."to" AS v, true AS fwd FROM bn_arcs a
  WHERE NOT EXISTS (SELECT 1 FROM ev WHERE ev.net = a.net AND ev.node = a."to" AND ev.mode = 'do')
  UNION ALL
  SELECT a.net, a."to", a."from", false FROM bn_arcs a
  WHERE NOT EXISTS (SELECT 1 FROM ev WHERE ev.net = a.net AND ev.node = a."to" AND ev.mode = 'do')
),
below(net, a, d) AS (
  SELECT net, u, v FROM g WHERE fwd
  UNION
  SELECT b.net, b.a, g.v FROM below b JOIN g ON g.net = b.net AND g.u = b.d AND g.fwd
),
trail(net, ns, dirs) AS (
  SELECT net, [id], []::BOOLEAN[] FROM bn_nodes
  UNION ALL
  SELECT t.net, list_append(t.ns, g.v), list_append(t.dirs, g.fwd)
  FROM trail t JOIN g ON g.net = t.net AND g.u = t.ns[-1]
  WHERE NOT list_contains(t.ns, g.v)
),
paths AS (
  SELECT row_number() OVER () AS path_id, net, ns, dirs FROM trail
  WHERE len(ns) >= 2 AND ns[1] < ns[-1]
),
inner_nodes AS (
  SELECT path_id, net, ns[j] AS node, dirs[j - 1] AND NOT dirs[j] AS collider
  FROM (SELECT *, unnest(generate_series(2, len(ns) - 1)) AS j FROM paths)
),
blocked AS (
  SELECT DISTINCT i.path_id FROM inner_nodes i
  WHERE CASE WHEN i.collider
    -- a collider opens with evidence on it or below it (not a do() below)
    THEN NOT EXISTS (SELECT 1 FROM ev WHERE ev.net = i.net AND ev.mode <> 'do'
                       AND (ev.node = i.node
                            OR ev.node IN (SELECT d FROM below WHERE below.net = i.net AND below.a = i.node)))
    -- a chain or a fork closes when the node is observed or set
    ELSE EXISTS (SELECT 1 FROM ev WHERE ev.net = i.net AND ev.node = i.node AND ev.likelihood IS NULL)
  END
)
SELECT net, path_id, j AS seq, ns[j] AS "from", ns[j + 1] AS "to"
FROM (SELECT *, unnest(generate_series(1, len(ns) - 1)) AS j FROM paths
      WHERE path_id NOT IN (SELECT path_id FROM blocked));

SELECT 'Why is the grass wet?'::SECTION;

SELECT 4::SPAN;
SELECT '**The question.** The grass is wet this morning. Did it rain, or was the sprinkler on?

**Try it.** Click **yes** on *Wet grass*: rain rises from 50 % to 71 %, the sprinkler from 30 % to 43 %. Then click **yes** on *Rain*: the sprinkler falls to 19 %, rain *explains away* the wet grass. Each bar shows how far the last click moved it.

**Seeing or doing.** Start over (× on the chips) and click **yes** on *Sprinkler on*: seeing it on makes a cloudy sky unlikely (17 %), nobody waters on a cloudy day. Now switch it to **do** in its chip: turning it on yourself says nothing about the clouds, and *Cloudy* is back at 50 %.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 420::HEIGHT;
SELECT 'sprinkler_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM bn_arcs a WHERE a.net = 'sprinkler'),
         beliefs := (SELECT list(b) FROM bn_beliefs b WHERE b.net = 'sprinkler'),
         evidence := (SELECT list(e) FROM bn_evidence e WHERE e.net = 'sprinkler' AND e.which = 'now'),
         tables := (SELECT list(t) FROM bn_tables t WHERE t.net = 'sprinkler'),
         paths := (SELECT list(p) FROM bn_paths p WHERE p.net = 'sprinkler'))
       AS "Wet grass: rain or sprinkler?"
FROM bn_nodes n
WHERE n.net = 'sprinkler';

SELECT 'What does the patient have?'::SECTION;

SELECT 4::SPAN;
SELECT '**The question.** A patient is short of breath and the X-ray is abnormal. Tuberculosis, lung cancer or bronchitis?

**Try it.** Click **yes** on *Abnormal X-ray* and on *Short of breath*: lung cancer rises from 6 % to 62 %, tuberculosis from 1 % to 11 %, bronchitis from 45 % to 68 %, and the patient is now likely a smoker (79 %). The double outline is a logical node: *TB or cancer* is yes when either is.

**What connects what.** Click *Visited Asia*, then hover *Smoker*: no path is open, the two are independent. Observe the X-ray and the path through *TB or cancer* opens.

**The arcs.** The thin dashed arc from *Visited Asia* is one the data rarely confirms: in 1,000 patients only about ten went to Asia, too few to show its effect.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 540::HEIGHT;
SELECT 'asia_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a.k) FROM bn_arcs a WHERE a.net = 'asia'),
         beliefs := (SELECT list(b) FROM bn_beliefs b WHERE b.net = 'asia'),
         evidence := (SELECT list(e) FROM bn_evidence e WHERE e.net = 'asia' AND e.which = 'now'),
         tables := (SELECT list(t) FROM bn_tables t WHERE t.net = 'asia'),
         paths := (SELECT list(p) FROM bn_paths p WHERE p.net = 'asia'))
       AS "Short of breath, abnormal X-ray: what does the patient have?"
FROM bn_nodes n
WHERE n.net = 'asia';
