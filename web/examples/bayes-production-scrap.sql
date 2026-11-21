-- @title Which lever lowers scrap?
-- @group Logistics & production
-- @tags  diagram, bayesian network, causal, intervention, production, quality
-- @description A turning shop's lot records and a Bayesian network: seeing vs doing, what to check first, the cheapest plan for a scrap target.
--
-- Which lever lowers scrap? A turning shop (two lines of four CNC lathes,
-- day and night shifts) scraps too many parts: one lot in six has more than
-- 3 % scrap. The usual charts show a gap for every factor, and the biggest
-- one for the people: lots run by senior operators have half the high
-- scrap of juniors'. A Bayesian network learned from the 9,984 lot records
-- separates seeing from doing: what you expect when you *see* a senior
-- operator on the lot, and what happens when you *make* the operator senior.
--
-- The data are simulated, with a planted truth, so every answer below is
-- checked against what really happens in the plant. The network was learned
-- and solved elsewhere; this page only looks its answers up
-- (data/bn-production/, Parquet): beliefs.parquet holds the belief of every
-- node and state for each combination of evidence on the six levers (each
-- unset, observed or set with do(), 21,875 of them), arcs.parquet the
-- learned arcs with how often 200 bootstrap resamples found them,
-- usual/weekly.parquet the aggregates the charts show, check.parquet the
-- network against the truth. No probability tables are published, so the
-- drawing has no table on hover.

SELECT 'bayes-net'::REQUIRES;

-- The network: the six levers (settable: a click observes or sets them),
-- what happens in the machine, and the result. States in reading order.
CREATE OR REPLACE TEMP TABLE pd_nodes AS
FROM (VALUES
  ('shift', 'Shift', ['day', 'night'], 'lever', true),
  ('supplier', 'Supplier', ['A', 'B'], 'lever', true),
  ('operator', 'Operator', ['senior', 'junior'], 'lever', true),
  ('line_speed', 'Line speed', ['normal', 'high'], 'lever', true),
  ('maintenance', 'Maintenance', ['on time', 'overdue'], 'lever', true),
  ('material_quality', 'Material', ['ok', 'marginal', 'bad'], 'lever', true),
  ('setup_error', 'Setup error', ['no', 'yes'], 'process', false),
  ('tool_wear', 'Tool wear', ['low', 'medium', 'high'], 'process', false),
  ('temp_drift', 'Spindle temperature drift', ['low', 'high'], 'process', false),
  ('dimensional_defect', 'Out of tolerance', ['none', 'minor', 'major'], 'defect', false),
  ('surface_defect', 'Surface defects', ['none', 'minor', 'major'], 'defect', false),
  ('scrap_rate', 'Scrap', ['low', 'medium', 'high'], 'result', false),
  ('on_time_delivery', 'Delivery', ['on time', 'at risk'], 'result', false)
) t(id, label, states, "group", settable);

-- The lookup key: per lever 0 (unset), 1..k (observed at its k-th state in
-- alphabetical order) or k+1..2k (set with do()), a mixed-radix number in
-- the order below (mult = the product of the radixes 2k+1 before it).
CREATE OR REPLACE TEMP TABLE pd_levers AS
FROM (VALUES
  ('line_speed', ['high', 'normal'], 1),
  ('maintenance', ['on time', 'overdue'], 5),
  ('material_quality', ['bad', 'marginal', 'ok'], 25),
  ('operator', ['junior', 'senior'], 175),
  ('shift', ['day', 'night'], 875),
  ('supplier', ['A', 'B'], 4375)
) t(node, states, mult);

-- The answers, with the file's state names as the page shows them; the
-- file rounds to four digits, so each node's beliefs are scaled to sum to 1.
CREATE OR REPLACE TEMP VIEW pd_lookup AS
SELECT key, node,
       CASE WHEN node = 'on_time_delivery' AND state = 'yes' THEN 'on time'
            ELSE replace(replace(state, 'on_time', 'on time'), 'at_risk', 'at risk') END AS state,
       probability / sum(probability) OVER (PARTITION BY key, node) AS probability
FROM read_parquet('data/bn-production/beliefs.parquet');

-- The clicks now and before the last one: hard evidence on a lever counts;
-- soft evidence (likelihoods) has no precomputed answer and is left out.
CREATE OR REPLACE TEMP VIEW pd_evidence AS
WITH v AS (
  SELECT 'now' AS which, getvariable('pd_ev') AS j
  UNION ALL SELECT 'before', getvariable('pd_ev_prev')
)
SELECT which, e.node, e.state, e.likelihood, coalesce(lower(e.mode), 'observe') AS mode
FROM (SELECT which, unnest(from_json(coalesce(nullif(j, ''), '[]'),
        '[{"node": "VARCHAR", "state": "VARCHAR", "likelihood": "DOUBLE", "mode": "VARCHAR"}]')) AS e FROM v)
WHERE e.likelihood IS NULL AND e.node IN (SELECT node FROM pd_levers);

CREATE OR REPLACE TEMP VIEW pd_keys AS
SELECT w.which, coalesce(sum((list_position(l.states, e.state) + CASE WHEN e.mode = 'do' THEN len(l.states) ELSE 0 END) * l.mult), 0)::INTEGER AS key
FROM (VALUES ('now'), ('before')) w(which)
LEFT JOIN pd_evidence e ON e.which = w.which
LEFT JOIN pd_levers l ON l.node = e.node
GROUP BY w.which;

CREATE OR REPLACE TEMP VIEW pd_beliefs AS
SELECT now.node, now.state, now.probability, pr.probability AS prior, was.probability AS previous
FROM pd_lookup now
JOIN pd_lookup was ON was.node = now.node AND was.state = now.state
JOIN pd_lookup pr ON pr.node = now.node AND pr.state = now.state
WHERE now.key = (SELECT key FROM pd_keys WHERE which = 'now')
  AND was.key = (SELECT key FROM pd_keys WHERE which = 'before')
  AND pr.key = 0;

-- The learned arcs: strength = how often 200 bootstrap resamples found
-- them. One the planner required (a faster line ships earlier): the data
-- alone found it in 38 % of the resamples; a required arc stays solid.
CREATE OR REPLACE TEMP TABLE pd_arcs AS
SELECT "from", "to",
       CASE WHEN "from" = 'line_speed' AND "to" = 'on_time_delivery' THEN 0.38 ELSE strength END AS strength,
       CASE WHEN "from" = 'line_speed' AND "to" = 'on_time_delivery' THEN 'required' ELSE 'learned' END AS status
FROM read_parquet('data/bn-production/arcs.parquet');

-- The plant's true graph (the simulation's mechanism), to compare with.
CREATE OR REPLACE TEMP TABLE pd_plant_arcs AS
FROM (VALUES
  ('shift', 'operator'), ('shift', 'line_speed'), ('shift', 'maintenance'),
  ('supplier', 'material_quality'), ('operator', 'setup_error'), ('maintenance', 'tool_wear'),
  ('tool_wear', 'temp_drift'), ('line_speed', 'temp_drift'), ('tool_wear', 'dimensional_defect'),
  ('temp_drift', 'dimensional_defect'), ('setup_error', 'dimensional_defect'),
  ('material_quality', 'surface_defect'), ('line_speed', 'surface_defect'),
  ('dimensional_defect', 'scrap_rate'), ('surface_defect', 'scrap_rate'),
  ('scrap_rate', 'on_time_delivery'), ('line_speed', 'on_time_delivery')
) t("from", "to");

CREATE OR REPLACE TEMP TABLE pd_usual AS
SELECT CASE factor WHEN 'shift' THEN 'Shift' WHEN 'operator' THEN 'Operator' WHEN 'maintenance' THEN 'Maintenance'
         WHEN 'line_speed' THEN 'Line speed' WHEN 'supplier' THEN 'Supplier' ELSE 'Material' END AS factor,
       replace(level, 'on_time', 'on time') AS level, lots, scrap_pct, high_share
FROM read_parquet('data/bn-production/usual.parquet');

CREATE OR REPLACE TEMP TABLE pd_weekly AS FROM read_parquet('data/bn-production/weekly.parquet');

-- The check: high scrap, change in points against no evidence, the
-- network's answer and the plant's (simulated with the lever observed or
-- forced on 400,000 lots).
CREATE OR REPLACE TEMP TABLE pd_check AS
SELECT CASE variable WHEN 'line_speed' THEN 'normal line speed' WHEN 'shift' THEN 'day shift'
         WHEN 'maintenance' THEN 'maintenance on time' WHEN 'supplier' THEN 'supplier A'
         WHEN 'operator' THEN 'senior operator' ELSE 'good material' END AS lever,
       max(model_delta_pts) FILTER (WHERE mode = 'observe') AS seen,
       max(model_delta_pts) FILTER (WHERE mode = 'do') AS done,
       max(truth_delta_pts) FILTER (WHERE mode = 'do') AS truth
FROM read_parquet('data/bn-production/check.parquet')
WHERE outcome = 'scrap_rate' AND outcome_state = 'high'
  AND (variable, state) IN (('line_speed', 'normal'), ('shift', 'day'), ('maintenance', 'on_time'),
                            ('supplier', 'A'), ('operator', 'senior'), ('material_quality', 'ok'))
GROUP BY ALL;

-- Each lever at its good state: seen (observed) and done (set with do()),
-- high scrap in points against no evidence.
CREATE OR REPLACE TEMP TABLE pd_lever_best AS
FROM (VALUES
  ('line_speed', 'normal', 'normal line speed'), ('maintenance', 'on time', 'maintenance on time'),
  ('material_quality', 'ok', 'good material'), ('operator', 'senior', 'senior operator'),
  ('shift', 'day', 'day shift'), ('supplier', 'A', 'supplier A')
) t(node, best, lever);

CREATE OR REPLACE TEMP VIEW pd_seen_done AS
WITH k AS (
  SELECT b.node, b.lever, list_position(l.states, b.best) * l.mult AS seen_key,
         (len(l.states) + list_position(l.states, b.best)) * l.mult AS done_key
  FROM pd_lever_best b JOIN pd_levers l USING (node)
),
hi AS (SELECT key, probability FROM pd_lookup WHERE node = 'scrap_rate' AND state = 'high')
SELECT k.node, k.lever,
       round(100 * (s.probability - z.probability), 1) AS seen,
       round(100 * (d.probability - z.probability), 1) AS done
FROM k JOIN hi s ON s.key = k.seen_key JOIN hi d ON d.key = k.done_key JOIN hi z ON z.key = 0;

-- The adjustment sets, from the graph: for each lever as the treatment and
-- Scrap as the outcome, what to compare like with like on. Shift is a
-- common cause of operator, line speed and maintenance; the material's
-- only cause is the supplier, which reaches scrap through it alone, so it
-- needs no adjustment.
CREATE OR REPLACE TEMP TABLE pd_adjust AS
FROM (VALUES
  ('operator', 'scrap_rate', 'minimal', 'shift', 'minimal'),
  ('maintenance', 'scrap_rate', 'minimal', 'shift', 'minimal'),
  ('line_speed', 'scrap_rate', 'minimal', 'shift', 'minimal'),
  ('material_quality', 'scrap_rate', 'minimal', NULL, NULL)
) t(treatment, outcome, set_id, node, kind);

-- The active paths given the evidence (d-separation): every trail along the
-- arcs (either way, no node twice) in the graph do() leaves, kept unless a
-- node on it blocks it. One row per step.
CREATE OR REPLACE TEMP VIEW pd_paths AS
WITH RECURSIVE
ev AS (SELECT * FROM pd_evidence WHERE which = 'now'),
g AS (
  SELECT a."from" AS u, a."to" AS v, true AS fwd FROM pd_arcs a
  WHERE NOT EXISTS (SELECT 1 FROM ev WHERE ev.node = a."to" AND ev.mode = 'do')
  UNION ALL
  SELECT a."to", a."from", false FROM pd_arcs a
  WHERE NOT EXISTS (SELECT 1 FROM ev WHERE ev.node = a."to" AND ev.mode = 'do')
),
below(a, d) AS (
  SELECT u, v FROM g WHERE fwd
  UNION
  SELECT b.a, g.v FROM below b JOIN g ON g.u = b.d AND g.fwd
),
trail(ns, dirs) AS (
  SELECT [id], []::BOOLEAN[] FROM pd_nodes
  UNION ALL
  SELECT list_append(t.ns, g.v), list_append(t.dirs, g.fwd)
  FROM trail t JOIN g ON g.u = t.ns[-1]
  WHERE NOT list_contains(t.ns, g.v)
),
paths AS (SELECT row_number() OVER () AS path_id, ns, dirs FROM trail WHERE len(ns) >= 2 AND ns[1] < ns[-1]),
inner_nodes AS (
  SELECT path_id, ns[j] AS node, dirs[j - 1] AND NOT dirs[j] AS collider
  FROM (SELECT *, unnest(generate_series(2, len(ns) - 1)) AS j FROM paths)
),
blocked AS (
  SELECT DISTINCT i.path_id FROM inner_nodes i
  WHERE CASE WHEN i.collider
    THEN NOT EXISTS (SELECT 1 FROM ev WHERE ev.mode <> 'do'
                       AND (ev.node = i.node OR ev.node IN (SELECT d FROM below WHERE below.a = i.node)))
    ELSE EXISTS (SELECT 1 FROM ev WHERE ev.node = i.node)
  END
)
SELECT path_id, j AS seq, ns[j] AS "from", ns[j + 1] AS "to"
FROM (SELECT *, unnest(generate_series(1, len(ns) - 1)) AS j FROM paths)
WHERE path_id NOT IN (SELECT path_id FROM blocked);

-- What to check first: for each lever not yet known, how much seeing it
-- would tell about Scrap given the evidence so far, the mutual information
-- I(Scrap; lever | evidence) in bits, from the looked-up answers.
CREATE OR REPLACE TEMP VIEW pd_voi AS
WITH now AS (SELECT key FROM pd_keys WHERE which = 'now'),
cand AS (
  SELECT l.node, l.mult, unnest(l.states) AS state, unnest(generate_series(1, len(l.states))) AS pos
  FROM pd_levers l WHERE l.node NOT IN (SELECT node FROM pd_evidence WHERE which = 'now')
),
ps AS (
  SELECT c.*, b.probability AS p FROM cand c
  JOIN pd_lookup b ON b.key = (SELECT key FROM now) AND b.node = c.node AND b.state = c.state
),
base AS (SELECT state, probability AS q FROM pd_lookup WHERE key = (SELECT key FROM now) AND node = 'scrap_rate')
SELECT ps.node, n.label,
       sum(ps.p * b.probability * ln(b.probability / base.q)) / ln(2) AS bits
FROM ps
JOIN pd_lookup b ON b.key = (SELECT key FROM now) + ps.pos * ps.mult AND b.node = 'scrap_rate'
JOIN base ON base.state = b.state
JOIN pd_nodes n ON n.id = ps.node
WHERE b.probability > 0 AND ps.p > 0
GROUP BY ps.node, n.label;

-- The plans: each of five levers set (do) or left, 32 plans, with the cost
-- the inputs give each lever; their high scrap and delivery risk looked up.
-- (Moving every lot to the day shift would halve the capacity: not a plan.)
CREATE OR REPLACE TEMP VIEW pd_plans AS
WITH lv(i, node, best, lever, cost) AS (
  SELECT * FROM (VALUES
    (0, 'maintenance', 'on time', 'maintenance on time', getvariable('pd_cost_maint')),
    (1, 'line_speed', 'normal', 'normal line speed', getvariable('pd_cost_speed')),
    (2, 'operator', 'senior', 'senior operators', getvariable('pd_cost_senior')),
    (3, 'supplier', 'A', 'supplier A only', getvariable('pd_cost_supplier')),
    (4, 'material_quality', 'ok', 'inspect incoming material', getvariable('pd_cost_material')))
),
codes AS (
  SELECT lv.*, l.mult * (len(l.states) + list_position(l.states, lv.best)) AS code
  FROM lv JOIN pd_levers l USING (node)
),
plans AS (
  SELECT p.range AS plan,
         coalesce(sum(c.code) FILTER (WHERE (p.range >> c.i) & 1 = 1), 0)::INTEGER AS key,
         coalesce(sum(c.cost) FILTER (WHERE (p.range >> c.i) & 1 = 1), 0) AS cost,
         count(*) FILTER (WHERE (p.range >> c.i) & 1 = 1) AS n,
         coalesce(string_agg(c.lever, ' + ' ORDER BY c.i) FILTER (WHERE (p.range >> c.i) & 1 = 1), 'nothing') AS levers
  FROM range(32) p, codes c GROUP BY p.range
)
SELECT plans.*, round(100 * hi.probability, 1) AS high, round(100 * rk.probability, 1) AS at_risk,
       100 * hi.probability <= getvariable('pd_target') AS meets
FROM plans
JOIN pd_lookup hi ON hi.key = plans.key AND hi.node = 'scrap_rate' AND hi.state = 'high'
JOIN pd_lookup rk ON rk.key = plans.key AND rk.node = 'on_time_delivery' AND rk.state = 'at risk';

-- ── 1 · The question ─────────────────────────────────────────────────────────
SELECT 'The question'::TAB;

SELECT 12::COL;
SELECT '**Who asks:** the plant manager. One lot in six has more than 3 % scrap, and a sixth of the deliveries are at risk. **The data:** 9,984 lots of 400 parts over 26 weeks (May to October), as the MES logs them: shift, operator, supplier, line speed, hours since maintenance, tool wear, spindle temperature, the parts out of tolerance. **The question:** which lever is worth paying for — more senior operators on nights, maintenance on time, a slower line, the other supplier?

**The tabs.** *Ask the network* — click the levers, see what each one tells and what to check next. *Seeing or doing* — why the chart''s answer is the wrong one. *Best plan* — the cheapest set of levers for a scrap target. *How the graph was found* — what the data found and what a planner added. *Checked against the truth* — the plant is simulated, so every answer is checked.'::MARKDOWN,
       'The question'::TITLE;

SELECT 'Six months at a glance'::GROUP;
SELECT round(sum(scrap_pct * lots) / sum(lots), 2)::METRIC, 'scrap, % of parts'::LABEL FROM pd_usual WHERE factor = 'Shift';
SELECT round(100 * sum(high_share * lots) / sum(lots), 1)::PERCENT, 'lots with more than 3 % scrap'::LABEL FROM pd_usual WHERE factor = 'Shift';
SELECT round(100 * sum(at_risk_share * lots) / sum(lots), 1)::PERCENT, 'deliveries at risk'::LABEL FROM pd_weekly;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT week::XAXIS, shift::CATEGORY, round(100 * high_share, 1) AS "lots with > 3 % scrap (%)" ::LINECHART,
       'Lots with more than 3 % scrap per week, % — nights twice as often, every week'::TITLE
FROM pd_weekly ORDER BY week;

SELECT 4::COL;
SELECT '**The usual analysis: every factor has a gap.** Nights have twice the high-scrap lots of days; overdue machines three times those maintained on time; the fast line four times the normal one. The biggest people gap: lots of **senior operators** have 12 % high scrap, juniors 22 %.

**So, seniors for the night shift?** That is the expensive answer. But the factors move together: the day crew is mostly senior, nights run the line faster to catch up, and the maintenance crew works days, so night-run machines wait longer. A chart of one factor shows all the others that ride along. It can''t say which gap you would get if you paid for it.'::MARKDOWN;

SELECT 8::COL;
SELECT 380::HEIGHT;
SELECT factor || ': ' || level AS "factor"::XAXIS, round(100 * high_share, 1) AS "lots with > 3 % scrap (%)" ::BARCHART,
       factor::CATEGORY,
       'Lots with more than 3 % scrap by factor, % — a gap everywhere'::TITLE
FROM pd_usual ORDER BY factor, high_share;

-- ── 2 · Ask the network ──────────────────────────────────────────────────────
SELECT 'Ask the network'::TAB;

SELECT 4::COL;
SELECT '**Seeing.** Click **senior** on *Operator*: high scrap falls from 17 % to 13 %, the chart''s gap. Look at *Shift*: day is now 80 % likely, and *Maintenance* overdue less likely. Seeing a senior mostly tells you it''s a day lot.

**Doing.** Switch it to **do** in its chip (or Alt-click): you *make* the operator senior, the arc from *Shift* is cut, the shift stays 50/50. High scrap: 16 %. Making operators senior buys about one point.

**The levers.** Start over (× on the chips) and set **do(on time)** on *Maintenance*: 12 %. **do(normal)** on *Line speed*: 8 %. Combine them and see what is left.

**What connects what.** Hover a node: its parents, children and their other parents light up (its Markov blanket). Click a node''s name, then hover another: the paths between them that are open given your clicks light up. Click *Supplier*''s name and hover *Scrap*: one path, through the material. Observe *Material* and it closes — once you know the material, the supplier tells you nothing more.

Only the six levers can be clicked; the rest only react. Each bar shows how far the last click moved it. ⤢ shows the network full size, still clickable.'::MARKDOWN;

SELECT 8::COL;
SELECT 620::HEIGHT;
SELECT 'pd_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a."to", a."from") FROM pd_arcs a),
         beliefs := (SELECT list(b) FROM pd_beliefs b),
         evidence := (SELECT list(e) FROM pd_evidence e WHERE e.which = 'now'),
         paths := (SELECT list(p) FROM pd_paths p))
       AS "Why is scrap high? Click a lever"
FROM pd_nodes n;

SELECT 12::COL;
SELECT 260::HEIGHT;
SELECT label::XAXIS, round(bits, 3) AS "information about scrap (bits)" ::BARCHART, 1::FLIP,
       'What to check first — now: ' || (SELECT label FROM pd_voi ORDER BY bits DESC LIMIT 1) ::TITLE,
       'How much seeing each lever on a lot would tell about its scrap, given your clicks (mutual information); a lever you have set or seen drops out'::SUBTITLE
FROM pd_voi ORDER BY bits DESC;

-- ── 3 · Seeing or doing ──────────────────────────────────────────────────────
SELECT 'Seeing or doing'::TAB;

SELECT 4::COL;
SELECT pd_x ::BUTTONS, 'The lever'::LABEL, label::OPTION_LABEL
FROM (VALUES ('operator', 'Operator', 1), ('maintenance', 'Maintenance', 2), ('line_speed', 'Line speed', 3),
             ('material_quality', 'Material', 4)) t(pd_x, label, o) ORDER BY o;
SELECT '**Why the chart misleads.** **X** is the lever, **Y** the scrap. A dashed arc is on a **back-door path**: a way for the lever and scrap to hang together that does not run through the lever — for the operator, *Operator ← Shift → Maintenance → … → Scrap*. Seeing a senior operator brings the day shift along, with its slower line and maintained machines.

**Comparing like with like.** The ringed node is what to compare on: lots of the *same shift*. That blocks every back-door path (the bar on the arc), and what is left is the operator''s own effect: what do() answers.

**The material** has no back door: its only cause, the supplier, reaches scrap through the material alone. Seeing and doing agree.'::MARKDOWN;

SELECT 8::COL;
SELECT 520::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a."to", a."from") FROM pd_arcs a),
         beliefs := (SELECT list(b) FROM pd_beliefs b),
         adjust := (SELECT list(a) FROM pd_adjust a WHERE a.treatment = coalesce(nullif(getvariable('pd_x'), ''), 'operator')))
       AS "From the lever to scrap: which paths to block"
FROM pd_nodes n;

SELECT 'Seen against done, points of high scrap'::GROUP;
SELECT seen::METRIC, 'seen: lots where it was so, against all lots'::LABEL
FROM pd_seen_done WHERE node = coalesce(nullif(getvariable('pd_x'), ''), 'operator');
SELECT done::METRIC, 'done: making it so on every lot'::LABEL
FROM pd_seen_done WHERE node = coalesce(nullif(getvariable('pd_x'), ''), 'operator');
SELECT round(seen - done, 1)::METRIC, 'what seeing adds: the lever''s company, not the lever'::LABEL
FROM pd_seen_done WHERE node = coalesce(nullif(getvariable('pd_x'), ''), 'operator');
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 320::HEIGHT;
SELECT lever::XAXIS, seen AS "seen (the chart's answer)" ::BARCHART, done AS "done (what it buys)" ::BARCHART,
       'Each lever at its good state: high scrap in points, seen against done'::TITLE,
       'Where the bars differ, the chart credits the lever with its company; the operator most'::SUBTITLE
FROM pd_seen_done ORDER BY done;

-- ── 4 · Best plan ────────────────────────────────────────────────────────────
SELECT 'Best plan'::TAB;

SELECT 'What each lever costs (k€ per month, your numbers) and the target'::GROUP;
SELECT 8 AS pd_cost_maint ::NUMBER, 'Maintenance on time (a night maintenance shift)'::LABEL;
SELECT 12 AS pd_cost_speed ::NUMBER, 'Normal line speed (overtime for the lost output)'::LABEL;
SELECT 20 AS pd_cost_senior ::NUMBER, 'Senior operators on every lot'::LABEL;
SELECT 5 AS pd_cost_supplier ::NUMBER, 'Supplier A only (its price premium)'::LABEL;
SELECT 4 AS pd_cost_material ::NUMBER, 'Inspect all incoming material'::LABEL;
SELECT 10 AS pd_target ::NUMBER, 'Target: lots with > 3 % scrap at most (%)'::LABEL;
SELECT 1::ENDGROUP;

SELECT 'The cheapest plan that meets the target'::GROUP;
SELECT levers::TEXT_LARGE, 'the plan'::LABEL FROM pd_plans WHERE meets ORDER BY cost, high LIMIT 1;
SELECT cost::METRIC, 'k€ per month'::LABEL FROM pd_plans WHERE meets ORDER BY cost, high LIMIT 1;
SELECT high::PERCENT, 'lots with > 3 % scrap (now 17 %)'::LABEL FROM pd_plans WHERE meets ORDER BY cost, high LIMIT 1;
SELECT at_risk::PERCENT, 'deliveries at risk (now 17 %)'::LABEL FROM pd_plans WHERE meets ORDER BY cost, high LIMIT 1;
SELECT 1::ENDGROUP;

SELECT 4::COL;
SELECT '**Every plan, looked up.** Five levers, each set or left: 32 plans. For each, the network''s answer to *do(all of them)* is one row of the precomputed answers; with your costs, the cheapest one under the target wins. Change a cost or the target and the plans re-rank.

**What the plans say.** Senior operators are in no cheap plan: they buy about one point. A slower line or maintenance on time with inspected material do the work. And once all material is inspected, *supplier A only* adds nothing: the supplier acts on scrap only through the material.

The costs are illustrative; put in your own.'::MARKDOWN;

SELECT 8::COL;
SELECT 380::HEIGHT;
SELECT cost AS "cost (k€ / month)" ::XAXIS, high AS "lots with > 3 % scrap (%)" ::SCATTER,
       CASE WHEN meets THEN 'meets the target' ELSE 'misses it' END::CATEGORY,
       levers::TOOLTIP,
       getvariable('pd_target')::REFLINE,
       'All 32 plans: cost against high scrap — the cheapest point under the line wins'::TITLE
FROM pd_plans ORDER BY cost;

SELECT 12::COL;
SELECT 'Plans that meet the target, cheapest first'::TITLE,
       levers AS "Plan" ::TABLE, cost AS "k€ / month", high AS "high scrap (%)" ::COLORSCALE, at_risk AS "deliveries at risk (%)"
FROM pd_plans WHERE meets ORDER BY cost, high LIMIT 10;

-- ── 5 · How the graph was found ──────────────────────────────────────────────
SELECT 'How the graph was found'::TAB;

SELECT 4::COL;
SELECT 0.5 AS pd_threshold ::NUMBER, 'Solid from strength'::LABEL;
SELECT '**Learned from the lots.** A structure search over the 13 variables, with the process order as the only rule (the shift comes before the operator, the operator before the defects …). Each arc''s **strength** is how often 200 resamples of the lots found it: thick when always, dashed below the threshold above.

**One arc from a planner.** *Line speed → Delivery* (a faster line ships earlier) the data found in only 38 % of the resamples; the planner required it, so it stays solid.

**Against the plant''s true graph.** The plant is simulated, so its graph is known: grey arcs match it, amber is one the data added (*Spindle temperature drift → Scrap* — real in the data, since scrap counts the parts, not only the defect classes). Raise the threshold to 0.9 and see it dashed.'::MARKDOWN;

SELECT 8::COL;
SELECT 600::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a."to", a."from") FROM pd_arcs a),
         beliefs := (SELECT list(b) FROM pd_beliefs b),
         reference := (SELECT list(r) FROM pd_plant_arcs r),
         threshold := coalesce(getvariable('pd_threshold'), 0.5))
       AS "The learned graph against the plant's"
FROM pd_nodes n;

SELECT 12::COL;
SELECT 'The arcs'::TITLE,
       f.label || ' → ' || t.label AS "Arc" ::TABLE, round(100 * a.strength) AS "found in resamples (%)",
       CASE WHEN a.status = 'required' THEN 'required by the planner' ELSE 'learned' END AS "Origin" ::BADGE,
       CASE WHEN r."from" IS NULL THEN 'not in the plant' ELSE 'in the plant' END AS "Plant" ::BADGE
FROM pd_arcs a JOIN pd_nodes f ON f.id = a."from" JOIN pd_nodes t ON t.id = a."to"
LEFT JOIN pd_plant_arcs r ON r."from" = a."from" AND r."to" = a."to"
ORDER BY a.strength, f.label;

-- ── 6 · Checked against the truth ────────────────────────────────────────────
SELECT 'Checked against the truth'::TAB;

SELECT 4::COL;
SELECT '**Is the network right?** The plant is simulated, so its truth is known: forcing each lever on 400,000 simulated lots says what it really does. The network was learned from the 9,984 lots alone (plus the process order and one arc a planner knows: a faster line ships earlier).

It ranks the levers as the plant does, and it sees that *senior operator* mostly stands in for the shift: seen −3.9 points, done −0.9, truly −1.4. It underestimates maintenance a little (−4.9 vs −6.1). A second draw of the plant, learned the same way, gave the same ranking.'::MARKDOWN;

SELECT 8::COL;
SELECT 'High scrap, change in points against no evidence'::TITLE,
       lever AS "Lever" ::TABLE, seen AS "Seen (network)", done AS "Done (network)" ::COLORSCALE, truth AS "Done (true)"
FROM pd_check ORDER BY truth;
