-- @page Interactions and special features
-- @lead Beyond single charts: clicks that filter and drill, switchable views, composed figures, guided steps, annotations from data, diagrams, factor graphs, Bayesian networks and their automatic analysis — each a few lines of SQL.
CREATE TABLE geo AS SELECT * FROM (VALUES
  ('EU','Germany','Berlin'),('EU','Germany','Munich'),('EU','France','Paris'),('EU','France','Lyon'),
  ('Americas','USA','New York'),('Americas','USA','Chicago'),('Americas','Canada','Toronto'),
  ('APAC','Japan','Tokyo'),('APAC','Japan','Osaka'),('APAC','Australia','Sydney')) t(region, country, city);
CREATE TABLE orders AS
SELECT DATE '2026-01-05' + (7 * w)::INT AS week, g.region, g.country, g.city, c.channel,
       (abs(hash(g.city || c.channel || w)) % 60 + 20 + 2 * w) AS revenue
FROM geo g, (VALUES ('web'), ('app'), ('store')) c(channel), range(26) r(w);
SELECT 2::COLUMNS;

-- @tile Drill-down | `::DRILL` walks a hierarchy on a click — here after a click on EU; a breadcrumb leads back. | click: .dash svg [data-x="EU"]
SELECT 'region > country > city'::DRILL;
SELECT CASE getvariable('drill_level') WHEN 'region' THEN region WHEN 'country' THEN country ELSE city END AS place ::XAXIS,
       sum(revenue) ::BARCHART, ('Revenue by ' || getvariable('drill_level'))::TITLE
FROM orders WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)
GROUP BY 1 ORDER BY 2 DESC;
SELECT week ::XAXIS, sum(revenue) ::AREACHART, ('Weekly — ' || coalesce(nullif(getvariable('region'), ''), 'all regions'))::TITLE
FROM orders WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country) GROUP BY 1 ORDER BY 1;

-- @tile Named selections | `::SELECT_AS` makes a click set a variable of its own; selections combine. | click: .dash svg [data-x="APAC"]
SELECT 'regions multi'::SELECT_AS;
SELECT region ::XAXIS, sum(revenue) ::BARCHART, 'Regions — click to select'::TITLE FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT channel ::CATEGORY, sum(revenue) ::DONUTCHART, 'Channels in the selection'::TITLE FROM orders
WHERE len(getvariable('regions')) = 0 OR list_contains(getvariable('regions'), region) GROUP BY 1 ORDER BY 2 DESC;

-- @tile Treemap | Part-to-whole tiles nested by `::CATEGORY`; with a `::DRILL` they drill like bars.
SELECT 12::COL;
SELECT country ::XAXIS, region ::CATEGORY, sum(revenue) ::TREEMAP, 'Revenue by region and country'::TITLE FROM orders GROUP BY ALL;

-- @tile Alternative views | Consecutive `::VIEW` sections share a switcher: the same numbers as lines, bars or a table.
SELECT 12::COL;
SELECT 'Lines' AS shape ::VIEW;
SELECT week ::XAXIS, region ::CATEGORY, sum(revenue) ::LINECHART, 'Revenue by region'::TITLE FROM orders GROUP BY ALL ORDER BY 1;
SELECT 'Bars' ::VIEW;
SELECT week ::XAXIS, region ::CATEGORY, sum(revenue) ::BARCHART_STACKED, 'Revenue by region'::TITLE FROM orders GROUP BY ALL ORDER BY 1;
SELECT 'Table' ::VIEW;
SELECT region ::TABLE, sum(revenue) AS "Revenue" ::MONEY FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT 1::ENDVIEW;

-- @tile A composed figure | `::FIGURE` lays several charts out as one picture with aligned axes and panel tags.
SELECT 12::COL;
SELECT 'Revenue, three ways' ::FIGURE, '(hist / weekly) | regions' ::LAYOUT, '2,1' ::WIDTHS, 'A' ::TAG_LEVELS;
-- @name hist
SELECT revenue ::HISTOGRAM FROM orders;
-- @name weekly
SELECT week ::XAXIS, sum(revenue) ::LINECHART FROM orders GROUP BY 1 ORDER BY 1;
-- @name regions
SELECT region ::XAXIS, sum(revenue) ::BARCHART FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT 1 ::ENDFIGURE;

-- @tile Annotations from data | Event lines, shaded ranges, callouts and a target line, read per row from SQL.
SELECT 12::COL;
SELECT week ::XAXIS, sum(revenue) ::LINECHART,
       CASE WHEN week = DATE '2026-03-02' THEN week END ::XLINE, CASE WHEN week = DATE '2026-03-02' THEN 'launch' END ::XLINE_LABEL,
       CASE WHEN week = DATE '2026-05-04' THEN week END ::XBAND_FROM, CASE WHEN week = DATE '2026-05-04' THEN DATE '2026-05-25' END ::XBAND_TO,
       CASE WHEN week = DATE '2026-05-04' THEN 'promotion' END ::XBAND_LABEL,
       CASE WHEN sum(revenue) = max(sum(revenue)) OVER () THEN 'record week' END ::ANNOTATE,
       2400 ::REFLINE, 'target' ::REFLINE_LABEL, 'The launch lifted revenue; the promotion held it'::TITLE
FROM orders GROUP BY week ORDER BY week;

-- @tile Above and below a line | `::SIGN_SPLIT` colours one line, area or density red below a threshold and green above, cut exactly where it crosses — gains and losses, above or below a target.
SELECT 12::COL;
SELECT week ::XAXIS, sum(revenue) - 2400 AS "Revenue against the 2,400 target" ::AREACHART, 0 ::SIGN_SPLIT,
       'Weeks above and below target'::TITLE
FROM orders GROUP BY week ORDER BY week;

-- @tile Small multiples | `::FACET` draws one panel per value on shared axes.
SELECT 12::COL;
SELECT week ::XAXIS, channel ::CATEGORY, sum(revenue) ::AREACHART_STACKED, region ::FACET, 'Revenue by channel, per region'::TITLE
FROM orders GROUP BY ALL ORDER BY 1;

-- @tile A strip under a chart | `::TRACK` draws ticks, lanes or a small line under the chart above, on its x axis.
SELECT 12::COL;
SELECT week ::XAXIS, sum(revenue) ::LINECHART, 'Revenue with campaign weeks underneath'::TITLE FROM orders GROUP BY 1 ORDER BY 1;
SELECT week ::XAXIS, 'Campaigns' ::TRACK, 'campaign' ::LABEL FROM orders WHERE week IN (DATE '2026-02-02', DATE '2026-03-16', DATE '2026-05-11', DATE '2026-06-08') GROUP BY 1;

-- @tile Guided steps | `::GUIDE_STEP` turns sections into a sequence with Back / Next — onboarding, tutorials, wizards.
SELECT 12::COL;
SELECT 'Where the revenue comes from' AS tour ::GUIDE_STEP;
SELECT 'Step one looks at **regions**. Next shows the channels, then the trend.'::MARKDOWN;
SELECT region ::XAXIS, sum(revenue) ::BARCHART, 'Revenue by region'::TITLE FROM orders GROUP BY 1 ORDER BY 2 DESC;
SELECT 'Channels' ::GUIDE_STEP;
SELECT channel ::XAXIS, sum(revenue) ::BARCHART FROM orders GROUP BY 1 ORDER BY 2 DESC;

-- @tile Show a panel only when … | `::SHOW_IF` hides the next panel until a condition holds — here a toggle.
SELECT true AS details ::TOGGLE, 'Show the detail table'::LABEL;
SELECT getvariable('details') ::SHOW_IF;
SELECT city ::TABLE, sum(revenue) AS "Revenue" ::MONEY FROM orders GROUP BY 1 ORDER BY 2 DESC LIMIT 6;

-- @tile Text with live values | `::MARKDOWN` fills `{{col:fmt}}` from its own row, renders formulas and callouts.
SELECT 12::COL;
SELECT 'Revenue so far is **{{total:money}}**, {{share:percent}} of it from the web. Growth follows $r_t = \frac{y_t}{y_{t-1}} - 1$.' ::MARKDOWN,
       'note' ::CALLOUT, sum(revenue) AS total, round(100 * sum(revenue) FILTER (WHERE channel = 'web') / sum(revenue), 1) AS share FROM orders;

-- @tile A process diagram | `::DIAGRAM` draws nodes and edges from rows: a pipeline, a lineage, a process in swimlanes.
SELECT 12::COL;
SELECT id ::DIAGRAM, label ::LABEL, up ::UPSTREAM, lane ::LANE, 'From the order feed to the dashboard'::TITLE
FROM (VALUES ('feed', 'Order feed', []::VARCHAR[], 'Ingest'), ('clean', 'Clean and dedupe', ['feed'], 'Ingest'),
             ('fx', 'FX rates', []::VARCHAR[], 'Ingest'), ('model', 'Revenue model', ['clean', 'fx'], 'Model'),
             ('fc', 'Forecast', ['model'], 'Model'), ('dash', 'Dashboard', ['model', 'fc'], 'Serve')) t(id, label, up, lane);

-- @tile A factor graph | `anofox_plot_factor_graph` draws a Bayesian model as a Forney-style factor graph — factors as boxes, variables as circles, observations filled — with what the model learned. Hover θ; Step walks the messages.
SELECT 'factor-graph'::REQUIRES;
SELECT 12::COL;
CREATE TABLE fg_nodes AS
SELECT * FROM (VALUES ('th', 'variable', 'theta', '\theta', NULL, 'param', NULL), ('eq', 'equality', '=', '=', NULL, 'param', NULL),
                      ('pr', 'factor', 'Beta', 'Beta', NULL, 'param', NULL), ('a', 'constant', '2', '2', NULL, 'param', NULL),
                      ('b', 'constant', '3', '3', NULL, 'param', NULL)) t(id, kind, name, label, slice, layer, plate)
UNION ALL SELECT 'y' || i, 'data', 'y[' || i || ']', 'y_{' || i || '}', i::VARCHAR, 'obs', 'y' FROM range(4) r(i)
UNION ALL SELECT 'f' || i, 'factor', 'Bernoulli', 'Bernoulli', i::VARCHAR, 'obs', 'y' FROM range(4) r(i);
CREATE TABLE fg_edges AS
SELECT * FROM (VALUES ('th', 'eq', NULL, 0), ('eq', 'pr', 'out', 0), ('a', 'pr', 'a', 1), ('b', 'pr', 'b', 2)) t(variable, factor, interface, seq)
UNION ALL SELECT 'y' || i, 'f' || i, 'out', 0 FROM range(4) r(i)
UNION ALL SELECT 'eq', 'f' || i, 'p', 1 FROM range(4) r(i);
-- θ ~ Beta(2, 3); three of four returns defective → Beta(5, 4)
SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_edges e),
         marginals := [{'variable': 'th', 'iteration': 1, 'mean': 5 / 9, 'sd': 0.1571, 'family': 'Beta'}],
         schedule := (SELECT list({'iteration': 1, 'step': i + 1, 'variable': 'eq', 'factor': f, 'interface': 'p', 'direction': 'to_variable'})
                      FROM (SELECT 'pr' AS f, 0 AS i UNION ALL SELECT 'f' || i, i + 1 FROM range(4) r(i))))
       AS "What share of returned units is defective?"
FROM fg_nodes n;

-- @tile What drives a target | A Bayesian network's automatic analysis: how common each state is among churners and the rest, as a `::DUMBBELL` named by its two columns, the lift as a `::LABEL`.
SELECT 12::COL;
SELECT node || ' = ' || state ::DUMBBELL, 100 * p_not AS "stayed" ::XMIN, 100 * p_churn AS "churned" ::XMAX,
       '×' || printf('%.1f', p_churn / (0.18 * p_churn + 0.82 * p_not)) ::LABEL, 'percent'::YFORMAT,
       'What sets churners apart'::TITLE
FROM (VALUES ('payment failed', 'yes', 0.21, 0.05), ('support tickets', '3 or more', 0.45, 0.12),
             ('usage', 'falling', 0.58, 0.17), ('tenure', 'under 1 year', 0.52, 0.21),
             ('contract', 'monthly', 0.86, 0.47), ('onboarded', 'yes', 0.51, 0.73),
             ('usage', 'rising', 0.11, 0.28), ('contract', 'annual', 0.14, 0.53)) t(node, state, p_churn, p_not)
ORDER BY p_churn / (0.18 * p_churn + 0.82 * p_not) DESC;

-- @tile Variables that hang together | `anofox_plot_matrix(…, cluster := true)` orders a heatmap by hierarchical clustering: related variables sit side by side.
SELECT 12::COL;
SELECT anofox_plot_matrix(t, row := 'a', col := 'b', value := 'mi', cluster := true, domain := [0, 1]),
       'Which variables hang together'::TITLE
FROM (VALUES ('churned', 'usage', 0.57), ('churned', 'tickets', 0.47), ('churned', 'contract', 0.35),
             ('churned', 'tenure', 0.24), ('churned', 'plan', 0.02), ('usage', 'tickets', 0.66),
             ('usage', 'contract', 0.07), ('usage', 'tenure', 0.05), ('usage', 'plan', 0.04),
             ('tickets', 'contract', 0.04), ('tickets', 'tenure', 0.03), ('tickets', 'plan', 0.16),
             ('contract', 'tenure', 0.82), ('contract', 'plan', 0.52), ('tenure', 'plan', 0.38)) t(a, b, mi);

-- @tile The next question | Information against cost: a `::LINECHART` over a `::SCATTER` joins the rows that have a value — the efficient frontier.
SELECT 12::COL;
WITH v AS (
    SELECT *, mi > coalesce(max(mi) OVER (ORDER BY cost, mi DESC ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), -1) AS efficient
    FROM (VALUES ('usage', 0.142, 2.0), ('support tickets', 0.118, 1.0), ('contract', 0.087, 0.5), ('tenure', 0.061, 0.5),
                 ('payment failed', 0.032, 1.0), ('onboarded', 0.024, 5.0), ('plan', 0.006, 0.5),
                 ('exit survey', 0.171, 15.0), ('review call', 0.158, 30.0)) t(test, mi, cost))
SELECT cost ::XAXIS, mi AS "question" ::SCATTER, CASE WHEN efficient THEN mi END AS "efficient frontier" ::LINECHART,
       CASE WHEN efficient THEN test END ::LABEL, 'log10'::XSCALE, 'cost (minutes)'::XTITLE, 'information (bits)'::YTITLE,
       'Which question to ask next'::TITLE
FROM v ORDER BY cost;

-- @tile A Bayesian network | `anofox_plot_bayes_net` draws a Bayesian network from an engine's tables — a bar per state, the prior faint behind it, the observed node shaded. Hover a node for its probability table.
SELECT 'bayes-net'::REQUIRES;
SELECT 12::COL;
CREATE TABLE bn_nodes AS
SELECT * FROM (VALUES ('cloudy', 'Cloudy', 'chance'), ('sprinkler', 'Sprinkler on', 'chance'),
                      ('rain', 'Rain', 'chance'), ('grass', 'Wet grass', 'observed')) t(id, label, kind), (SELECT ['yes', 'no'] AS states);
-- exact beliefs with the grass seen wet, and before (prior)
CREATE TABLE bn_beliefs AS
SELECT node, s.state, CASE s.state WHEN 'yes' THEN p ELSE 1 - p END AS probability, CASE s.state WHEN 'yes' THEN q ELSE 1 - q END AS prior
FROM (VALUES ('cloudy', 0.5758, 0.5), ('sprinkler', 0.4298, 0.3), ('rain', 0.7079, 0.5), ('grass', 1.0, 0.6471)) t(node, p, q),
     (VALUES ('yes'), ('no')) s(state);
SELECT anofox_plot_bayes_net(n,
         arcs := [{'from': 'cloudy', 'to': 'sprinkler'}, {'from': 'cloudy', 'to': 'rain'},
                  {'from': 'sprinkler', 'to': 'grass'}, {'from': 'rain', 'to': 'grass'}],
         beliefs := (SELECT list(b) FROM bn_beliefs b),
         evidence := [{'node': 'grass', 'state': 'yes'}],
         tables := [{'node': 'rain', 'parent_states': ['yes'], 'state': 'yes', 'probability': 0.8},
                    {'node': 'rain', 'parent_states': ['yes'], 'state': 'no', 'probability': 0.2},
                    {'node': 'rain', 'parent_states': ['no'], 'state': 'yes', 'probability': 0.2},
                    {'node': 'rain', 'parent_states': ['no'], 'state': 'no', 'probability': 0.8}])
       AS "The grass is wet: did it rain?"
FROM bn_nodes n;

-- @tile Seeing or doing | Click a state to observe it (`'ev evidence'::SELECT_AS`) and the query recomputes the beliefs; the chip switches it to do(). Turning the sprinkler on yourself cuts its arc from the clouds (the hand), so Cloudy is back at 50 %, and each bar shows how far that click moved it.
SELECT 'bayes-net'::REQUIRES;
SELECT 12::COL;
CREATE TABLE bn_do_nodes AS
SELECT * FROM (VALUES ('cloudy', 'Cloudy'), ('sprinkler', 'Sprinkler on'), ('rain', 'Rain'), ('grass', 'Wet grass')) t(id, label),
              (SELECT 'chance' AS kind, ['yes', 'no'] AS states);
-- the beliefs with the sprinkler set on, do(), and before: seen on
CREATE TABLE bn_do_beliefs AS
SELECT node, s.state, CASE s.state WHEN 'yes' THEN p ELSE 1 - p END AS probability,
       CASE s.state WHEN 'yes' THEN q ELSE 1 - q END AS prior, CASE s.state WHEN 'yes' THEN w ELSE 1 - w END AS previous
FROM (VALUES ('cloudy', 0.5, 0.5, 0.1667), ('sprinkler', 1.0, 0.3, 1.0), ('rain', 0.5, 0.5, 0.3), ('grass', 0.945, 0.6471, 0.927)) t(node, p, q, w),
     (VALUES ('yes'), ('no')) s(state);
SELECT anofox_plot_bayes_net(n,
         arcs := [{'from': 'cloudy', 'to': 'sprinkler'}, {'from': 'cloudy', 'to': 'rain'},
                  {'from': 'sprinkler', 'to': 'grass'}, {'from': 'rain', 'to': 'grass'}],
         beliefs := (SELECT list(b) FROM bn_do_beliefs b),
         evidence := [{'node': 'sprinkler', 'state': 'yes', 'mode': 'do'}])
       AS "Turning the sprinkler on: the clouds stay at 50 %"
FROM bn_do_nodes n;
