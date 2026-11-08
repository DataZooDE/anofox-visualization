-- @page Interactions and special features
-- @lead Beyond single charts: clicks that filter and drill, switchable views, composed figures, guided steps, annotations from data, diagrams — each a few lines of SQL.
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
