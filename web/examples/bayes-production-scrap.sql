-- @title Which lever lowers scrap?
-- @group Logistics & production
-- @tags  diagram, bayesian network, causal, intervention, production, quality
-- @description Six months of lot records from a turning shop: the charts say senior operators halve scrap, a Bayesian network says what each lever would really do.
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

CREATE OR REPLACE TEMP TABLE pd_arcs AS
SELECT "from", "to", strength FROM read_parquet('data/bn-production/arcs.parquet');

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

SELECT 'The problem'::SECTION;

SELECT 12::COL;
SELECT '**Who asks:** the plant manager. One lot in six has more than 3 % scrap, and a sixth of the deliveries are at risk. **The data:** 9,984 lots of 400 parts over 26 weeks (May to October), as the MES logs them: shift, operator, supplier, line speed, hours since maintenance, tool wear, spindle temperature, the parts out of tolerance. **The question:** which lever is worth paying for — more senior operators on nights, maintenance on time, a slower line, the other supplier?'::MARKDOWN,
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

SELECT 'The usual analysis'::SECTION;

SELECT 4::COL;
SELECT '**Every factor has a gap.** Nights have twice the high-scrap lots of days; overdue machines three times those maintained on time; the fast line four times the normal one. The biggest people gap: lots of **senior operators** have 12 % high scrap, juniors 22 %.

**So, seniors for the night shift?** That is the expensive answer. But the factors move together: the day crew is mostly senior, nights run the line faster to catch up, and the maintenance crew works days, so night-run machines wait longer. A chart of one factor shows all the others that ride along. It can''t say which gap you would get if you paid for it.'::MARKDOWN;

SELECT 8::COL;
SELECT 380::HEIGHT;
SELECT factor || ': ' || level AS "factor"::XAXIS, round(100 * high_share, 1) AS "lots with > 3 % scrap (%)" ::BARCHART,
       factor::CATEGORY,
       'Lots with more than 3 % scrap by factor, % — a gap everywhere'::TITLE
FROM pd_usual ORDER BY factor, high_share;

SELECT 'Ask the network'::SECTION;

SELECT 4::COL;
SELECT '**Seeing.** Click **senior** on *Operator*: high scrap falls from 17 % to 13 %, the chart''s gap. Look at *Shift*: day is now 80 % likely, and *Maintenance* overdue less likely. Seeing a senior mostly tells you it''s a day lot.

**Doing.** Switch it to **do** in its chip (or Alt-click): you *make* the operator senior, the arc from *Shift* is cut, the shift stays 50/50. High scrap: 16 %. Making operators senior buys about one point.

**The levers.** Start over (× on the chips) and set **do(on time)** on *Maintenance*: 12 %. **do(normal)** on *Line speed*: 8 %. Combine them and see what is left.

Only the six levers can be clicked; the machine, the defects and the result only react. Each bar shows how far the last click moved it.'::MARKDOWN;

SELECT 8::COL;
SELECT 620::HEIGHT;
SELECT 'pd_ev evidence'::SELECT_AS;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a ORDER BY a."to", a."from") FROM pd_arcs a),
         beliefs := (SELECT list(b) FROM pd_beliefs b),
         evidence := (SELECT list(e) FROM pd_evidence e WHERE e.which = 'now'))
       AS "Why is scrap high? Click a lever"
FROM pd_nodes n;

SELECT 'Checked against the truth'::SECTION;

SELECT 4::COL;
SELECT '**Is the network right?** The plant is simulated, so its truth is known: forcing each lever on 400,000 simulated lots says what it really does. The network was learned from the 9,984 lots alone (plus the process order and one arc a planner knows: a faster line ships earlier).

It ranks the levers as the plant does, and it sees that *senior operator* mostly stands in for the shift: seen −3.9 points, done −0.9, truly −1.4. It underestimates maintenance a little (−4.9 vs −6.1). A second draw of the plant, learned the same way, gave the same ranking.'::MARKDOWN;

SELECT 8::COL;
SELECT 'High scrap, change in points against no evidence'::TITLE,
       lever AS "Lever" ::TABLE, seen AS "Seen (network)", done AS "Done (network)" ::COLORSCALE, truth AS "Done (true)"
FROM pd_check ORDER BY truth;
