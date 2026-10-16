// Browser dashboard builder — 100% client-side.
//   DuckDB-Wasm runs the SQL, anofox-visualization (wasm) plans the ::ROLE annotations and
//   renders each panel to SVG. No server, no DuckDB extension.
import init, {
  plan,
  render_panel,
  render_stack,
  render_contract,
  map_bounds,
  roles_json,
  format_number,
  format_number_locale,
  format_date_locale,
  resolve_dashboard_theme,
  page_sql,
  table_profile,
  human_header,
  period_resolve,
  kpi_parts,
  threshold_rules,
} from "./pkg/anofox_visualization.js";
import { animKit } from "./anim.js";

const dpAnim = animKit(window); // chart motion (web/anim.js)

// Examples, grouped for the sidebar. Each entry is a full dashboard script.
const SESSIONS = `CREATE OR REPLACE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);`;
const SALES = `CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('W1','app',30,1200.0),('W1','web',22,900.0),('W1','api',12,400.0),
  ('W2','app',41,1600.0),('W2','web',28,1100.0),('W2','api',15,520.0),
  ('W3','app',26,980.0),('W3','web',33,1300.0),('W3','api', 9,330.0),
  ('W4','app',48,2000.0),('W4','web',30,1250.0),('W4','api',18,640.0)
) t(week, channel, n, revenue);`;

const SAMPLE_GROUPS = [
  {
    group: "Start here",
    items: {
      Overview: `-- Annotate result columns with ::ROLE casts. Un-annotated statements
-- (this CREATE) are setup; annotated SELECTs become panels.
${SESSIONS}

SELECT 'Overview — click a pie slice or a table row to filter the KPIs'::LABEL;

SELECT 12::COL;
SELECT '**How it works:** every panel is a SQL query whose columns are tagged with \`::ROLE\` casts — \`::XAXIS\`, \`::CATEGORY\`, a chart kind like \`::BARCHART_STACKED\`, \`::PIE\`, \`::LINECHART\`, \`::TABLE\`, or a KPI (\`::COMPACT\`/\`::METRIC\`). Un-annotated statements (the \`CREATE TABLE\`) are setup. **Click a pie slice or a table row** — the KPI strip filters via \`getvariable(''selected'')\`.'::MARKDOWN, 'What this shows'::TITLE;

-- KPIs in a ::GROUP render as a compact strip. They opt into the cross-filter
-- (getvariable('selected')), so clicking a channel re-computes them.
SELECT 'Key metrics'::GROUP;
SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT count(DISTINCT channel)::METRIC, 'Channels'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT round(avg(n),1)::METRIC, 'Avg / cell'::LABEL FROM sessions WHERE getvariable('selected') IN ('', channel);
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions by channel'::TITLE
FROM sessions GROUP BY ALL ORDER BY week, channel;

SELECT 6::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Weekly trend'::TITLE
FROM sessions GROUP BY ALL ORDER BY week, channel;

SELECT 6::COL;
SELECT channel::CATEGORY, sum(n)::PIE, 'Share'::TITLE FROM sessions GROUP BY ALL;

SELECT 6::COL; SELECT 'single'::ROWSELECT; -- clicking a row sets getvariable('selected')
SELECT 'Detail'::TITLE, channel, week, sum(n) AS n ::TABLE
FROM sessions GROUP BY ALL ORDER BY channel, week;`,

      "Signal explorer": `-- Everything below is generated on the fly with range() — no table needed.
SELECT 'No table needed — a whole dashboard from pure SQL math'::LABEL;

SELECT 12::COL;
SELECT 'The point of this example: **you don''t need a data source.** Every panel is a formula over DuckDB''s **range()** (a row-number generator), so the entire dashboard is computed on the fly — no table, file, or database. Handy for **synthetic demos, parametric what-ifs, and teaching**. It still exercises the full chart vocabulary from math alone: a **line with a ± confidence band** (::BAND_LOWER / ::BAND_UPPER), a **gauge**, a **scatter**, a **histogram**, and a **heatmap surface**, plus a KPI strip. Hover any chart for the toolbox.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 'Signal stats'::GROUP;
SELECT round(avg(sin(i/6.0)*40+50),1)::METRIC, 'Mean'::LABEL, '<40:bad; <55:warn; good'::THRESHOLDS FROM range(0,120) t(i);
SELECT round(max(sin(i/6.0)*40+50),1)::METRIC, 'Peak'::LABEL FROM range(0,120) t(i);
SELECT round(stddev(sin(i/6.0)*40+50),1)::METRIC, 'Std dev'::LABEL FROM range(0,120) t(i);
SELECT 1::ENDGROUP;

SELECT 8::COL;
SELECT i::XAXIS,
       (50 + i*0.4 + sin(i/6.0)*22)::LINECHART,
       (50 + i*0.4 + sin(i/6.0)*22 - 10)::BAND_LOWER,
       (50 + i*0.4 + sin(i/6.0)*22 + 10)::BAND_UPPER,
       'Trend + seasonality (± band)'::TITLE
FROM range(0, 80) t(i);

SELECT 4::COL;
SELECT round(100.0*count(*) FILTER (WHERE sin(i/6.0) > 0)/count(*),0)::GAUGE, '0,100'::RANGE,
       '#e03131,#efc94c,#0ca678'::COLORS, 'Time above mean'::TITLE
FROM range(0,120) t(i);

SELECT 6::COL;
SELECT i::XAXIS, (sin(i/5.0)*25 + 50 + ((i*13) % 20))::SCATTER, 'Noisy samples'::TITLE
FROM range(0, 120) t(i);

SELECT 6::COL;
-- sum of three coprime modular sequences ≈ a bell (Irwin–Hall), all from range()
SELECT ((i*7) % 34 + (i*13) % 33 + (i*29) % 31)::HISTOGRAM, 'Value distribution'::TITLE FROM range(0, 500) t(i);

SELECT 12::COL;
SELECT (i % 12)::XAXIS, floor(i/12.0)::YAXIS,
       round(avg(sin(i/6.0)*cos(floor(i/12.0)/3.0)*40 + 50),0)::HEATMAP, 'sin · cos surface'::TITLE
FROM range(0, 96) t(i) GROUP BY 1,2 ORDER BY 1,2;`,
    },
  },

  {
    group: "Charts",
    items: {
      "Chart gallery": `${SALES}
CREATE OR REPLACE TABLE m AS
SELECT i AS id,
       ['app','web','api'][(i % 3) + 1] AS channel,
       'W' || ((i % 4) + 1) AS week,
       -- approximately normal (sum of 6 uniforms → central-limit bell),
       -- shifted per channel so the box/violin plots show real spread
       round((random()+random()+random()+random()+random()+random() - 3) * 18
             + 100 + (i % 3) * 12, 1) AS value
FROM range(0, 600) t(i);

-- a small continuous-x series for the step / smooth / bubble / markArea demos
CREATE OR REPLACE TABLE ts AS
SELECT i AS day, round(50 + 30 * sin(i / 3.0) + (random() - 0.5) * 12, 1) AS y
FROM range(1, 25) t(i);

-- a year of daily values for the calendar-heatmap demo
CREATE OR REPLACE TABLE cal AS
SELECT (DATE '2024-01-01' + CAST(i AS INTEGER)) AS d,
       round(15 + 30 * abs(sin(i / 9.0)) + (abs(hash(i)) % 25), 0) AS value
FROM range(0, 365) t(i);

-- an OHLC random walk for the candlestick demos (cast hash%N to a SIGNED int
-- before subtracting, else the UINT64 result underflows)
CREATE OR REPLACE TABLE ohlc AS
WITH w AS (SELECT i, 100 + sum((CAST(abs(hash(i)) % 100 AS INTEGER) - 50) / 12.0) OVER (ORDER BY i) AS o FROM range(0, 120) t(i)),
     oc AS (SELECT i, o, o + (CAST(abs(hash(i * 7)) % 80 AS INTEGER) - 40) / 12.0 AS c FROM w)
SELECT (DATE '2024-01-01' + CAST(i AS INTEGER)) AS d, round(o, 2) AS open, round(c, 2) AS close,
       round(greatest(o, c) + (abs(hash(i * 3)) % 40) / 12.0, 2) AS high,
       round(least(o, c) - (abs(hash(i * 5)) % 40) / 12.0, 2) AS low
FROM oc;

-- multi-series radar data + 5 groups for the multi-boxplot demo
CREATE OR REPLACE TABLE radar AS SELECT * FROM (VALUES
  ('Speed','Model A',85),('Power','Model A',90),('Range','Model A',70),('Comfort','Model A',60),('Safety','Model A',95),('Price','Model A',55),
  ('Speed','Model B',68),('Power','Model B',62),('Range','Model B',90),('Comfort','Model B',82),('Safety','Model B',74),('Price','Model B',88)
) t(metric, model, score);
CREATE OR REPLACE TABLE groups5 AS
SELECT 'G' || ((i % 5) + 1) AS grp, round((random()+random()+random()-1.5)*20 + 50 + (i%5)*8, 1) AS val FROM range(0, 500) t(i);

-- a 3-series version for the interactivity demos (legend toggle/focus etc.)
CREATE OR REPLACE TABLE tsm AS
SELECT i AS day, s.name AS series,
       round(50 + s.amp * sin((i + s.ph) / 3.0) + (random() - 0.5) * 8, 1) AS y,
       round(random() * 100, 1) AS z
FROM range(1, 25) t(i), (VALUES ('alpha', 22, 0), ('beta', 15, 4), ('gamma', 28, 8)) s(name, amp, ph);

SELECT 'Chart gallery — every chart kind, in tabs'::LABEL;

SELECT 'Bars & columns'::TAB;
SELECT 12::COL;
SELECT 'Charts are grouped into **tabs by family**: bars, lines & areas, scatter, distributions, pie/gauge/radar, waterfall/bullet/funnel, heatmaps & candlestick, sparklines — plus an **interactivity playground** and **maps**. This tab: a **stacked bar**, a **horizontal bar** (::FLIP swaps the axes), and **bars with data labels** (::DATALABELS).'::MARKDOWN, 'What this shows'::TITLE;
SELECT 6::COL; SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, '€'::YFORMAT, 'Revenue by week × channel (stacked, € axis)'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
-- ::FLIP swaps the axes → a horizontal bar chart
SELECT 6::COL; SELECT channel::XAXIS, sum(n)::BARCHART, TRUE::FLIP, 'Sessions by channel (horizontal, ::FLIP)'::TITLE FROM sales GROUP BY channel ORDER BY sum(n);
SELECT 12::COL; SELECT channel::XAXIS, sum(n)::BARCHART, ''::DATALABELS, 'Bars with data labels (::DATALABELS)'::TITLE FROM sales GROUP BY channel ORDER BY channel;

SELECT 'Lines & areas'::TAB;
SELECT 6::COL; SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Sessions — multi-series line'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 6::COL; SELECT day::XAXIS, y::STEP, 'Step line (::STEP)'::TITLE FROM ts ORDER BY day;
SELECT 6::COL; SELECT week::XAXIS, channel::CATEGORY, sum(n)::AREA_STACKED, 'Stacked area (::AREA_STACKED)'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 6::COL; SELECT week::XAXIS, sum(n)::BARCHART, sum(revenue)/50::LINECHART, 35::REFLINE, 'Combo — bar + line + reference line'::TITLE FROM sales GROUP BY ALL ORDER BY week;
SELECT 12::COL; SELECT day::XAXIS, y::LINECHART, CASE WHEN day>=17 THEN day END::MARKAREA,
       (CASE WHEN day%3=0 THEN 45 WHEN day%3=1 THEN 65 ELSE 85 END)::REFLINE,
       'Shaded region + reference lines (::MARKAREA/::REFLINE)'::TITLE FROM ts ORDER BY day;
-- annotations from data: events and ranges joined onto the days they mark
SELECT 12::COL; SELECT c.d::XAXIS, c.value::LINECHART, e.d::XLINE, e.what::XLINE_LABEL,
       b.f::XBAND_FROM, b.t::XBAND_TO, b.what::XBAND_LABEL,
       CASE WHEN c.d = arg_max(c.d, c.value) OVER () THEN 'Busiest week' END::ANNOTATE,
       40::REFLINE, 'Target'::REFLINE_LABEL,
       'Events, a shaded range and a callout from SQL rows (::XLINE_LABEL, ::XBAND_*, ::ANNOTATE)'::TITLE
FROM (SELECT date_trunc('week', d)::DATE AS d, round(avg(value), 1) AS value FROM cal GROUP BY 1) c
LEFT JOIN (VALUES (DATE '2024-02-05', 'Launch'), (DATE '2024-09-16', 'Price change')) e(d, what) ON e.d = c.d
LEFT JOIN (VALUES (DATE '2024-06-10', DATE '2024-07-01', 'Summer promo')) b(f, t, what) ON b.f = c.d
ORDER BY c.d;

SELECT 'Scatter & trends'::TAB;
SELECT 6::COL; SELECT day::XAXIS, y::SMOOTH, 'Scatter + LOESS trend (::SMOOTH)'::TITLE FROM ts ORDER BY day;
SELECT 6::COL; SELECT day::XAXIS, y::SCATTER, y::SIZE, 'Bubble — size = value (::SIZE)'::TITLE FROM ts ORDER BY day;
-- Jitter (::JITTER): a categorical scatter that spreads overlapping points.
SELECT 6::COL; SELECT grp::XAXIS, val::JITTER, 'Jittered scatter (::JITTER)'::TITLE FROM groups5;
-- A single-axis strip plot (jitter over one row).
SELECT 6::COL; SELECT val::XAXIS, 1::JITTER, 'Strip plot — one variable on a single axis'::TITLE FROM groups5 WHERE grp='G1';

SELECT 'Distributions'::TAB;
SELECT 6::COL; SELECT value::HISTOGRAM, 'Histogram'::TITLE FROM m;
SELECT 6::COL; SELECT value::DENSITY, channel::CATEGORY, 'Density by channel'::TITLE FROM m;
SELECT 6::COL; SELECT channel::XAXIS, value::BOXPLOT, 'Box plot (unfilled)'::TITLE FROM m;
SELECT 6::COL; SELECT channel::XAXIS, value::VIOLIN, 'Violin'::TITLE FROM m;

SELECT 'Pie, gauge & radar'::TAB;
SELECT 6::COL; SELECT channel::CATEGORY, sum(n)::PIE, 'Pie'::TITLE FROM sales GROUP BY ALL;
SELECT 6::COL; SELECT channel::CATEGORY, sum(revenue)::DONUTCHART, 'Donut'::TITLE FROM sales GROUP BY ALL;
SELECT 6::COL; SELECT sum(n) FILTER (WHERE week='W4')::GAUGE, '0,120'::RANGE, '#e03131,#efc94c,#0ca678'::COLORS, 'Gauge'::TITLE FROM sales;
-- ::THRESHOLDS: one rule set gives a gauge its status zones and a chart its threshold lines
SELECT 6::COL; SELECT 72::GAUGE, '0,100'::RANGE, '<50:bad; <80:warn; good'::THRESHOLDS, 'Gauge zones from ::THRESHOLDS — 72 is a warning'::TITLE;
SELECT 6::COL; SELECT day::XAXIS, y::LINECHART, '<35:bad; <70:warn; good'::THRESHOLDS, 'Threshold lines from the same kind of rules'::TITLE FROM ts ORDER BY day;
-- Radar (::RADAR): axes from x, one polygon per ::CATEGORY series.
SELECT 6::COL; SELECT metric::XAXIS, score::RADAR, model::CATEGORY, 'Radar — model comparison'::TITLE FROM radar;

SELECT 'Waterfall, bullet & funnel'::TAB;
-- Waterfall (::WATERFALL): the first row is the start total, the others signed steps;
-- a NULL value (or ::CATEGORY 'total') is a subtotal, and the bridge closes with an End total.
SELECT 12::COL; SELECT step::XAXIS, eur::WATERFALL, '€'::YFORMAT, 'Volume beat budget; price and FX gave half of it back'::TITLE
FROM (VALUES (1, 'Budget', 480), (2, 'Volume', 95), (3, 'Price', -38), (4, 'Mix', 22), (5, 'FX', -17)) t(i, step, eur) ORDER BY i;
-- Bullet (::BULLET): the measure per row against its ::GOAL tick over ::RANGE bands.
SELECT 6::COL; SELECT region::XAXIS, attained::BULLET, goal::GOAL, '60,85,110'::RANGE, '%'::YFORMAT, 'Two regions short of target'::TITLE
FROM (VALUES (1, 'North', 92, 90), (2, 'South', 71, 85), (3, 'East', 104, 95), (4, 'West', 66, 80)) t(i, region, attained, goal) ORDER BY i;
-- Funnel (::FUNNEL): ordered stages, widths by value, conversion from the previous stage.
SELECT 6::COL; SELECT stage::XAXIS, n::FUNNEL, 'A quarter of sign-ups never start a trial'::TITLE
FROM (VALUES (1, 'Visits', 48000), (2, 'Sign-ups', 6200), (3, 'Trials', 4650), (4, 'Paid', 1180)) t(i, stage, n) ORDER BY i;

SELECT 'Heatmap, calendar & candlestick'::TAB;
-- Heatmap (::HEATMAP): x × y tiles; rows keep the SQL order (first on top),
-- ::DATALABELS writes each value into its cell, a click cross-filters by the row.
SELECT 6::COL; SELECT week::XAXIS, channel::YAXIS, round(avg(value),1)::HEATMAP, ''::DATALABELS, 'Heatmap — week × channel'::TITLE FROM m GROUP BY ALL ORDER BY week, channel;
-- Candlestick (::CANDLESTICK): date + ::OPEN/::HIGH/::LOW + close as the measure.
SELECT 6::COL; SELECT d::XAXIS, open::OPEN, high::HIGH, low::LOW, close::CANDLESTICK, 'Candlestick — 30-day OHLC'::TITLE FROM ohlc WHERE d < DATE '2024-01-31' ORDER BY d;
SELECT 12::COL; SELECT d::XAXIS, open::OPEN, high::HIGH, low::LOW, close::CANDLESTICK, 'Candlestick — 120-day OHLC'::TITLE FROM ohlc ORDER BY d;
-- Calendar heatmap (::CALENDAR): a date axis + a daily value, laid out
-- GitHub-style with month labels on top and weekday rows.
SELECT 165::HEIGHT;
SELECT 12::COL; SELECT d::XAXIS, value::CALENDAR, 'Calendar heatmap — a year of daily activity'::TITLE FROM cal ORDER BY d;

SELECT 'Sparklines'::TAB;
SELECT 12::COL;
SELECT 'Sparklines are word-sized trend lines — ideal as small multiples in a table or grid. One **::SPARKLINE** per series; here revenue plus three activity series side by side.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 3::COL; SELECT sum(revenue)::SPARKLINE, 'Revenue'::TITLE FROM sales GROUP BY week ORDER BY week;
SELECT 3::COL; SELECT y::SPARKLINE, 'alpha'::TITLE FROM tsm WHERE series='alpha' ORDER BY day;
SELECT 3::COL; SELECT y::SPARKLINE, 'beta'::TITLE FROM tsm WHERE series='beta' ORDER BY day;
SELECT 3::COL; SELECT y::SPARKLINE, 'gamma'::TITLE FROM tsm WHERE series='gamma' ORDER BY day;

SELECT 'Interactive'::TAB;
-- A ::MARKDOWN column renders as a rich-text panel (headings, lists, links…).
SELECT 4::COL;
SELECT '## Every chart is interactive

Hover a chart for its **toolbox**:

- **⇄** line ↔ bar
- **▤** data view (rows as a table)
- **▧** brush-select a region
- **◧** value filter (dim by range)
- **⬍** range slider (drag to zoom x)
- **⟳** restore · **⭳** save PNG

Also: a crosshair **tooltip** on hover, **click** a legend to hide a series, **hover** it to *focus*, and scroll / drag to zoom.'::MARKDOWN, 'How to explore'::TITLE;
SELECT 8::COL; SELECT day::XAXIS, series::CATEGORY, y::LINECHART, 'Multi-series — toggle/focus the legend, ⇄ to bars, ⬍ to zoom'::TITLE FROM tsm ORDER BY series, day;
SELECT 6::COL; SELECT y::XAXIS, z::SCATTER, series::CATEGORY, 'Scatter — ▧ brush a box, ◧ filter by value'::TITLE FROM tsm ORDER BY y;
SELECT 6::COL; SELECT series::XAXIS, sum(y)::BARCHART, ''::DATALABELS, 'Bars — ◧ value filter dims bars below the range'::TITLE FROM tsm GROUP BY series ORDER BY series;

-- Maps read real GeoJSON in the browser via DuckDB's spatial extension
-- (ST_Read → ST_AsText → WKT), then ggplot-rs draws the geometry (::MAP).
SELECT 'Map'::TAB;

SELECT 'World'::SUBTAB;
SELECT 12::COL;
SELECT ST_AsText(geom) ::MAP,
       ln(POP_EST + 1) ::BARCHART,          -- fill = log population
       NAME ::LABEL,                        -- hover tooltip
       'World population by country — Natural Earth'::TITLE
FROM ST_Read('countries.geojson')
WHERE NAME <> 'Antarctica';

SELECT 'Earthquakes'::SUBTAB;
SELECT 12::COL;
-- Quake points coloured by magnitude, semi-transparent (::ALPHA) so overlaps
-- read as density, over a grey country basemap. The two layers ride in disjoint
-- rows: quakes fill ::MAP, countries fill ::BASEMAP.
SELECT ST_AsText(geom) ::MAP, mag ::BARCHART, place ::LABEL, 0.6 ::ALPHA, NULL ::BASEMAP,
       'USGS earthquakes (M≥2.5, past 30 days) — colour = magnitude'::TITLE
FROM ST_Read('quakes.geojson') WHERE mag IS NOT NULL
UNION ALL
SELECT NULL, NULL, NULL, 0.6, ST_AsText(geom), NULL
FROM ST_Read('countries.geojson') WHERE NAME <> 'Antarctica';

-- Linked maps: click a country on the left and the right map filters to the
-- earthquakes INSIDE it (true point-in-polygon, so no neighbours leak in).
-- Clicking sets getvariable('selected') = the country name; click empty space to
-- clear. IF NOT EXISTS keeps the spatial reads and the one-off point-in-polygon
-- join out of the per-click re-run.
SELECT 'Linked'::SUBTAB;

CREATE TABLE IF NOT EXISTS cgeo AS
SELECT NAME AS country, ST_AsText(geom) AS wkt, geom,
       ST_XMin(geom) AS x0, ST_XMax(geom) AS x1, ST_YMin(geom) AS y0, ST_YMax(geom) AS y1
FROM ST_Read('countries.geojson') WHERE NAME <> 'Antarctica';

-- Tag each quake with the country that CONTAINS it (offshore → NULL). The bbox
-- test short-circuits before the exact ST_Contains, so the join stays quick.
CREATE TABLE IF NOT EXISTS qcty AS
SELECT ST_AsText(q.geom) AS wkt, q.mag, q.place, c.country
FROM ST_Read('quakes.geojson') q
LEFT JOIN cgeo c
  ON ST_X(q.geom) BETWEEN c.x0 AND c.x1 AND ST_Y(q.geom) BETWEEN c.y0 AND c.y1
     AND ST_Contains(c.geom, ST_Point(ST_X(q.geom), ST_Y(q.geom)))
WHERE q.mag IS NOT NULL;

SELECT 12::COL;
SELECT 'Two **linked maps** rendered from real GeoJSON (DuckDB''s spatial extension → WKT → ::MAP). **Click a country** on the left choropleth and the right map filters to the earthquakes *inside* it — a true point-in-polygon join, so no neighbours leak in. Click empty space to clear.'::MARKDOWN, 'What this shows'::TITLE;

-- LEFT: world, filled by onshore-quake count per country. Click a country.
SELECT 6::COL;
SELECT c.wkt ::MAP, count(q.country) ::BARCHART, c.country ::LABEL,
       'Quakes per country — click one'::TITLE
FROM cgeo c LEFT JOIN qcty q ON q.country = c.country
GROUP BY c.wkt, c.country;

-- RIGHT: quakes inside the selected country (exact), semi-transparent, on a basemap.
SELECT 6::COL;
SELECT q.wkt ::MAP, q.mag ::BARCHART, q.place ::LABEL, 0.6 ::ALPHA, NULL ::BASEMAP,
       'Earthquakes in selection'::TITLE
FROM qcty q
WHERE getvariable('selected') = '' OR q.country = getvariable('selected')
UNION ALL
SELECT NULL, NULL, NULL, 0.6, wkt, NULL FROM cgeo;`,
    },
  },

  {
    group: "Forecast", // = FORECAST_GROUP (declared below)
    ext: true,
    items: {
      "How much will each item sell next year?": `-- Live in-browser SeasonalES forecast: DuckDB-Wasm loads a parquet and runs the
-- anofox-forecast extension entirely client-side — no server. On boot the app
-- registers the parquet and, on a v1.5.x engine, does the equivalent of:
--
--   SET custom_extension_repository='<app>/localext';  -- our local wasm build
--   INSTALL anofox_forecast; LOAD anofox_forecast;
--
-- (We serve a locally-built wasm because the signed community build doesn't link
-- against this DuckDB-Wasm runtime — DataZooDE/anofox-forecast#239.)

-- All ~30k M5 item×store series are forecast at once (SeasonalES, 12 months).
-- The heavy steps use CREATE TABLE IF NOT EXISTS so they run once per session
-- (~2s for every series) and clicking a row is then instant. Tables are prefixed
-- lx_ so they never clash with the other forecast dashboards' cached tables.
CREATE TABLE IF NOT EXISTS lx_m AS SELECT series, ds, y FROM read_parquet('m5_monthly.parquet');

CREATE TABLE IF NOT EXISTS lx_fc AS
  SELECT series, ds, round(yhat,0) AS yhat, round(yhat_lower,0) AS lo, round(yhat_upper,0) AS hi
  FROM ts_forecast_by('lx_m', series, ds, y, 'SeasonalES', 12, '1mo', MAP{'seasonal_period':'12'});

-- Small per-series summary computed straight from lx_m + lx_fc (no big materialised
-- history+forecast union, so re-running keeps memory low).
CREATE TABLE IF NOT EXISTS lx_summary AS
  SELECT h.series AS item, h.last_actual, c.next_fc, c.fc_total,
         round(100.0*(c.fc_total-h.actual_12)/nullif(h.actual_12,0),1) AS growth
  FROM (SELECT series, arg_max(y,ds) AS last_actual,
               sum(y) FILTER (WHERE ds > (SELECT max(ds) FROM lx_m)-INTERVAL 12 MONTH) AS actual_12
        FROM lx_m GROUP BY 1) h
  JOIN (SELECT series, sum(yhat) AS fc_total, arg_min(yhat,ds) AS next_fc
        FROM lx_fc GROUP BY 1) c USING(series);

-- 12-month backtest: hold out the last year, forecast it from the train with two
-- methods, and compare to the held-out actuals across every series.
CREATE TABLE IF NOT EXISTS lx_train AS
  SELECT series, ds, y FROM lx_m WHERE ds <= (SELECT max(ds) FROM lx_m)-INTERVAL 12 MONTH;
CREATE TABLE IF NOT EXISTS lx_bt AS
  WITH act AS (SELECT series, ds, y AS actual FROM lx_m WHERE ds > (SELECT max(ds) FROM lx_train))
  SELECT 'SeasonalES' AS method, a.series, a.ds, a.actual, f.yhat AS predicted
    FROM act a JOIN ts_forecast_by('lx_train', series, ds, y, 'SeasonalES', 12, '1mo', MAP{'seasonal_period':'12'}) f USING(series, ds)
  UNION ALL
  SELECT 'SeasonalNaive', a.series, a.ds, a.actual, f.yhat
    FROM act a JOIN ts_forecast_by('lx_train', series, ds, y, 'SeasonalNaive', 12, '1mo', MAP{'seasonal_period':'12'}) f USING(series, ds);
-- Per-series, per-method error metrics (one row per series × method).
CREATE TABLE IF NOT EXISTS lx_metrics AS
  SELECT series, method,
         round(avg(abs(actual-predicted)),2)                      AS mae,
         round(sqrt(avg(pow(actual-predicted,2))),2)              AS rmse,
         round(100*avg(abs(actual-predicted)/nullif(actual,0)),1) AS mape
  FROM lx_bt GROUP BY series, method;
-- One row per series: each method's KPIs side by side + the per-series winner
-- (the method with the lower MAE on that series' holdout).
CREATE TABLE IF NOT EXISTS lx_scores AS
  SELECT series AS item,
         arg_min(method, mae)                            AS winner,
         min(mae)  FILTER (WHERE method='SeasonalES')    AS es_mae,
         min(mae)  FILTER (WHERE method='SeasonalNaive') AS sn_mae,
         min(rmse) FILTER (WHERE method='SeasonalES')    AS es_rmse,
         min(rmse) FILTER (WHERE method='SeasonalNaive') AS sn_rmse
  FROM lx_metrics GROUP BY series;

SELECT 'Forecasting ' || (SELECT count(*) FROM lx_summary)::VARCHAR || ' of ' || (SELECT count(DISTINCT series) FROM lx_m)::VARCHAR || ' M5 item×store series in the browser — click a row to drill the chart'::LABEL;

SELECT 12::COL;
SELECT 'Fully **in-browser forecasting**: DuckDB-Wasm loads a Parquet and runs the anofox-forecast extension (SeasonalES) over ~30k M5 series — no server. The chart shows the selected item''s **history + 12-month forecast** (shaded = 95% interval) with **both methods'' backtest** over the held-out last year drawn on top. The **Summary** and **Backtest statistics** tabs below list every series — **click any row** to drill the chart to that item.'::MARKDOWN, 'What this shows'::TITLE;

-- The chart, prominent: history (lx_m) + 12-month SeasonalES forecast (lx_fc,
-- shaded 95% interval) + BOTH methods' backtest predictions over the 12-month
-- holdout (lx_bt), for the selected item (defaults to the largest by forecast;
-- a click on the chart itself selects a series name, not an item, and is ignored).
SELECT 12::COL;
SELECT ds       ::XAXIS,
       'Actual' ::CATEGORY,
       y        ::LINECHART,
       y        ::BAND_LOWER,
       y        ::BAND_UPPER,
       'History + 12-month SeasonalES forecast (shaded = 95% interval) + backtest'::TITLE
FROM lx_m WHERE series=COALESCE((SELECT item FROM lx_summary WHERE item = getvariable('selected')),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'Forecast', y, y, y, '' FROM lx_m
  WHERE series=COALESCE((SELECT item FROM lx_summary WHERE item = getvariable('selected')),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1)) AND ds=(SELECT max(ds) FROM lx_m)
UNION ALL
SELECT ds, 'Forecast', yhat, lo, hi, '' FROM lx_fc WHERE series=COALESCE((SELECT item FROM lx_summary WHERE item = getvariable('selected')),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'SeasonalES (backtest)', predicted, predicted, predicted, '' FROM lx_bt
  WHERE method='SeasonalES' AND series=COALESCE((SELECT item FROM lx_summary WHERE item = getvariable('selected')),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'SeasonalNaive (backtest)', predicted, predicted, predicted, '' FROM lx_bt
  WHERE method='SeasonalNaive' AND series=COALESCE((SELECT item FROM lx_summary WHERE item = getvariable('selected')),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
ORDER BY 1;

-- Two tabs share the chart above: a per-series Summary and the Backtest
-- statistics. Both tables are click-to-drill (set getvariable('selected')).
SELECT 'Summary'::TAB;
SELECT 12::COL;
SELECT 'Per-series **summary** of every forecast item — last actual, next-month and 12-month forecast totals, and growth vs the prior 12 months. **Click a row** to drill the chart above to that item.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 12::COL;
SELECT 'single'::ROWSELECT; -- click a row to drill the chart above
SELECT item        AS "Item × store"     ::PAGED,
       last_actual AS "Last actual"      ::COMPACT,
       next_fc     AS "Next month"       ::COMPACT,
       fc_total    AS "12-mo forecast"   ::COMPACT,
       growth      AS "vs prior 12mo %"  ::TREND
FROM lx_summary ORDER BY fc_total DESC;

SELECT 'Backtest statistics'::TAB;
SELECT 12::COL;
SELECT 'A 12-month-holdout **backtest** comparing **SeasonalES** vs **SeasonalNaive** across every series (MAE / RMSE / MAPE), with a per-series winner. **Overall winner (most series won by MAE): ' || (SELECT winner FROM lx_scores WHERE winner IS NOT NULL GROUP BY winner ORDER BY count(*) DESC LIMIT 1) || '.** Click a row to drill the chart above.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 6::COL;
SELECT method AS "Method" ::TABLE,
       round(avg(mae),2)     AS "Avg MAE"       ::COMPACT,
       round(avg(rmse),2)    AS "Avg RMSE"      ::COMPACT,
       round(median(mape),1) AS "Median MAPE %" ::PLAIN
FROM lx_metrics GROUP BY method ORDER BY 2;
SELECT 6::COL;
SELECT winner ::XAXIS, count(*) AS wins ::BARCHART, 'Series won per method (by MAE)'::TITLE
FROM lx_scores WHERE winner IS NOT NULL GROUP BY 1 ORDER BY 2 DESC;
SELECT 12::COL;
SELECT 'single'::ROWSELECT; -- click a row to drill the chart above
SELECT item    AS "Item × store" ::PAGED,
       winner  AS "Winner"       ::BADGE,
       es_mae  AS "ES · MAE"     ::COMPACT,
       sn_mae  AS "SN · MAE"     ::COMPACT,
       es_rmse AS "ES · RMSE"    ::COMPACT,
       sn_rmse AS "SN · RMSE"    ::COMPACT
FROM lx_scores ORDER BY item;`,
      "Which forecast method fits our demand?": `-- M5 forecast · decomposition · backtest on the anofox-forecast extension,
-- one dashboard across all categories (monthly). Methods: MFLES vs MSTL.
-- Heavy steps cached with CREATE TABLE IF NOT EXISTS.
CREATE TABLE IF NOT EXISTS an_cat AS
  SELECT split_part(series,'_',1) AS category, ds, sum(y) AS y
  FROM read_parquet('m5_monthly.parquet') GROUP BY 1,2;

-- 12-month forecast with BOTH methods (a method column tags each), so the panels
-- can overlay MFLES against MSTL on the same history.
CREATE TABLE IF NOT EXISTS an_fc AS
  SELECT 'MFLES' AS method, category, ds, round(yhat,0) AS yhat
    FROM ts_forecast_by('an_cat', category, ds, y, 'MFLES', 12, '1mo', MAP{'seasonal_period':'12'})
  UNION ALL
  SELECT 'MSTL', category, ds, round(yhat,0)
    FROM ts_forecast_by('an_cat', category, ds, y, 'MSTL', 12, '1mo', MAP{'seasonal_period':'12'});

-- Decomposition: take the smooth TREND from the extension's MSTL, then split the
-- detrended signal into a classical seasonal component (mean per calendar month,
-- centred) and the remainder — a simple, transparent additive decomposition.
CREATE TABLE IF NOT EXISTS an_decomp AS
  WITH d AS (SELECT category, generate_subscripts(trend,1) AS rn, unnest(trend) AS trend
             FROM ts_mstl_decomposition_by('an_cat', category, ds, y, MAP{'periods':'[12]'})),
       o AS (SELECT category, ds, y, row_number() OVER (PARTITION BY category ORDER BY ds) AS rn FROM an_cat),
       j AS (SELECT o.category, o.ds, o.y AS observed, d.trend, o.y - d.trend AS detrended, month(o.ds) AS mo
             FROM o JOIN d USING (category, rn)),
       s AS (SELECT category, mo, avg(detrended) AS smean FROM j GROUP BY 1,2),
       sc AS (SELECT category, mo, smean - avg(smean) OVER (PARTITION BY category) AS seasonal FROM s)
  SELECT j.category, j.ds, round(j.observed,0) AS observed, round(j.trend,0) AS trend,
         round(sc.seasonal,0) AS seasonal,
         round(j.observed - j.trend - sc.seasonal,0) AS remainder
  FROM j JOIN sc USING (category, mo);

-- Backtest: hold out the last 12 months, forecast them from the train with BOTH
-- methods, compare to actuals.
CREATE TABLE IF NOT EXISTS an_train AS SELECT * FROM an_cat WHERE ds <= (SELECT max(ds) FROM an_cat)-INTERVAL 12 MONTH;
CREATE TABLE IF NOT EXISTS an_bt AS
  SELECT 'MFLES' AS method, f.category, f.ds, round(f.yhat,0) AS predicted, t.y AS actual
    FROM ts_forecast_by('an_train', category, ds, y, 'MFLES', 12, '1mo', MAP{'seasonal_period':'12'}) f
    JOIN an_cat t ON f.category=t.category AND f.ds=t.ds
  UNION ALL
  SELECT 'MSTL', f.category, f.ds, round(f.yhat,0), t.y
    FROM ts_forecast_by('an_train', category, ds, y, 'MSTL', 12, '1mo', MAP{'seasonal_period':'12'}) f
    JOIN an_cat t ON f.category=t.category AND f.ds=t.ds;
-- Per category × method error metrics, then a per-category winner (lower MAE) with
-- both methods' scores side by side.
CREATE TABLE IF NOT EXISTS an_metrics AS
  SELECT category, method,
         round(avg(abs(actual-predicted)),0)                      AS mae,
         round(sqrt(avg(pow(actual-predicted,2))),0)              AS rmse,
         round(100*avg(abs(actual-predicted)/nullif(actual,0)),1) AS mape
  FROM an_bt GROUP BY 1,2;
CREATE TABLE IF NOT EXISTS an_scores AS
  SELECT category,
         arg_min(method, mae)                    AS winner,
         min(mae)  FILTER (WHERE method='MFLES')  AS mfles_mae,
         min(mae)  FILTER (WHERE method='MSTL')   AS mstl_mae,
         min(rmse) FILTER (WHERE method='MFLES')  AS mfles_rmse,
         min(rmse) FILTER (WHERE method='MSTL')   AS mstl_rmse
  FROM an_metrics GROUP BY category;

SELECT 'M5 — MFLES vs MSTL: forecast · decomposition · backtest'::LABEL;

SELECT 12::COL;
SELECT 'A head-to-head of two anofox-forecast methods — **MFLES** vs **MSTL** — on M5 monthly sales aggregated to category. Each small multiple overlays both 12-month forecasts on the history; the table scores a 12-month-holdout **backtest** (MAE / RMSE per category, with the winner). Each **category tab** drills in: both methods'' backtest fit, plus the MSTL **decomposition** (trend + seasonal + remainder) and **residual diagnostics** (histogram + normal **Q-Q plot** of the remainder).'::MARKDOWN, 'What this shows'::TITLE;

-- Forecast overview: one small multiple per category overlaying the history with
-- BOTH methods' 12-month forecasts (each bridged to the last actual so it joins).
-- Each panel auto-scales (FOODS ≫ HOBBIES).
SELECT 'Forecast — history + 12-month MFLES vs MSTL'::LABEL;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'FOODS'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='FOODS'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='FOODS'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='FOODS' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='FOODS' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOBBIES'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOBBIES'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='HOBBIES'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='HOBBIES' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='HOBBIES' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOUSEHOLD'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, method, yhat FROM an_fc WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, 'MFLES', y FROM an_cat WHERE category='HOUSEHOLD' AND ds=(SELECT max(ds) FROM an_cat)
      UNION ALL SELECT ds, 'MSTL', y FROM an_cat WHERE category='HOUSEHOLD' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;

-- Backtest overview: 12-month-holdout MAE / RMSE for each method per category,
-- side by side, with the per-category winner (lower MAE).
SELECT 'Backtest — 12-month holdout: MFLES vs MSTL'::LABEL;
SELECT 12::COL;
SELECT category   AS "Category"     ::TABLE,
       winner     AS "Winner (MAE)" ::BADGE,
       mfles_mae  AS "MFLES · MAE"  ::COMPACT,
       mstl_mae   AS "MSTL · MAE"   ::COMPACT,
       mfles_rmse AS "MFLES · RMSE" ::COMPACT,
       mstl_rmse  AS "MSTL · RMSE"  ::COMPACT
FROM an_scores ORDER BY category;

-- Per-category deep dive (one tab each): decomposition + residual diagnostics
-- (histogram + normal Q-Q of the remainder) + the backtest fit.
SELECT 'FOODS'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='FOODS';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='FOODS';
SELECT 12::COL; SELECT ds ::XAXIS, series ::CATEGORY, val ::LINECHART, 'Backtest — actual vs MFLES vs MSTL'::TITLE
FROM (SELECT ds, 'Actual' AS series, actual AS val FROM an_bt WHERE category='FOODS' AND method='MSTL'
      UNION ALL SELECT ds, method, predicted FROM an_bt WHERE category='FOODS') ORDER BY series, ds;

SELECT 'HOBBIES'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='HOBBIES';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='HOBBIES';
SELECT 12::COL; SELECT ds ::XAXIS, series ::CATEGORY, val ::LINECHART, 'Backtest — actual vs MFLES vs MSTL'::TITLE
FROM (SELECT ds, 'Actual' AS series, actual AS val FROM an_bt WHERE category='HOBBIES' AND method='MSTL'
      UNION ALL SELECT ds, method, predicted FROM an_bt WHERE category='HOBBIES') ORDER BY series, ds;

SELECT 'HOUSEHOLD'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='HOUSEHOLD';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='HOUSEHOLD';
SELECT 12::COL; SELECT ds ::XAXIS, series ::CATEGORY, val ::LINECHART, 'Backtest — actual vs MFLES vs MSTL'::TITLE
FROM (SELECT ds, 'Actual' AS series, actual AS val FROM an_bt WHERE category='HOUSEHOLD' AND method='MSTL'
      UNION ALL SELECT ds, method, predicted FROM an_bt WHERE category='HOUSEHOLD') ORDER BY series, ds;`,
    },
  },

  {
    group: "Interactivity",
    items: {
      "Filters & inputs": `CREATE OR REPLACE TABLE events AS SELECT * FROM (VALUES
  (DATE '2024-01-05','app','EU','launch',30),(DATE '2024-01-12','web','EU','promo',22),(DATE '2024-01-20','api','US','launch',12),
  (DATE '2024-02-03','app','US','promo',41),(DATE '2024-02-14','web','EU','launch',28),(DATE '2024-02-22','api','EU','promo',15),
  (DATE '2024-03-02','app','EU','promo',26),(DATE '2024-03-11','web','US','launch',33),(DATE '2024-03-19','api','US','promo', 9),
  (DATE '2024-03-28','app','US','launch',48)
) t(day, channel, region, note, n);

-- 80 categories → the multiselect switches to its searchable mode (>50 options).
CREATE OR REPLACE TABLE cats AS
SELECT 'CAT-' || lpad(i::VARCHAR, 3, '0') AS code, (abs(hash(i)) % 500)::INT AS v
FROM range(1, 81) t(i);

-- Two tables for the cross-filter tab; each emits a variable named after its
-- first column (sku / market) so the two selections compose independently.
CREATE OR REPLACE TABLE sales2 AS
SELECT sku, region, month, (abs(hash(sku || region || month)) % 80 + 40) AS amount
FROM (VALUES ('SKU-A'),('SKU-B'),('SKU-C')) a(sku),
     (VALUES ('EU'),('US')) b(region),
     (VALUES ('2024-01'),('2024-02'),('2024-03'),('2024-04'),('2024-05'),('2024-06')) c(month);

SELECT 'Filters & inputs — controls, date range & cross-filter'::LABEL;

SELECT 'Inputs'::TAB;
SELECT 12::COL;
SELECT 'Interactive **controls** driven entirely from SQL: a dropdown, two multiselects (the 80-option one is **searchable**), a slider (::NUMBER), free text, and a date. Each control''s output **column name becomes a DuckDB variable** — the charts read it with **getvariable(''name'')**, so changing a control re-runs their queries live. **The address bar follows**: every changed control lands in the URL as \`?v.<name>=<value>\` (and Share copies it), so a reload or a link restores this exact view. (Date range and cross-filter are on the next tabs.)'::MARKDOWN, 'What this shows'::TITLE;

-- the output COLUMN NAME becomes the DuckDB variable (getvariable('name'))
SELECT 'Filters'::GROUP;
SELECT DISTINCT region::DROPDOWN FROM events ORDER BY region;      -- single-select
SELECT DISTINCT channel::MULTISELECT FROM events ORDER BY channel; -- multi-select (a few)
SELECT DISTINCT code::MULTISELECT FROM cats ORDER BY code;         -- many → searchable
SELECT 5 AS min_n ::NUMBER;                                        -- number
SELECT '' AS note ::TEXT;                                          -- free text (try 'promo')
SELECT DATE '2024-01-01' AS since ::DATE;                          -- single date
SELECT 1::ENDGROUP;

-- KPI: honours every input AND the click cross-filter (click a bar segment
-- below to drill the number to that channel; click empty space to clear).
SELECT 4::COL;
SELECT sum(n)::METRIC, 'Sessions (inputs + click)'::LABEL FROM events
WHERE region = getvariable('region')
  AND list_contains(getvariable('channel'), channel)
  AND n >= getvariable('min_n')
  AND day >= getvariable('since')::DATE
  AND (getvariable('note') = '' OR note ILIKE '%' || getvariable('note') || '%')
  AND (getvariable('selected') = '' OR channel = getvariable('selected'));

SELECT 8::COL;
-- ::EMPTY: what this panel says when the filters leave no rows (try min_n 60).
SELECT 'No sessions match — lower the minimum or clear the text filter'::EMPTY;
SELECT day::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions (all filters applied)'::TITLE
FROM events
WHERE region = getvariable('region')
  AND list_contains(getvariable('channel'), channel)
  AND n >= getvariable('min_n')
  AND day >= getvariable('since')::DATE
  AND (getvariable('note') = '' OR note ILIKE '%' || getvariable('note') || '%')
GROUP BY ALL ORDER BY day, channel;

-- The searchable multiselect drives its own KPIs over the 80-row cats table.
SELECT 4::COL;
SELECT count(*)::METRIC, 'Categories in filter'::LABEL FROM cats
WHERE list_contains(getvariable('code'), code);
SELECT 8::COL;
SELECT sum(v)::METRIC, 'Value across selected categories'::LABEL FROM cats
WHERE list_contains(getvariable('code'), code);

-- An optional input: one dashboard serves many runs, and not every run writes
-- this file. \`-- @requires\` turns its absence into "Not available".
SELECT 12::COL;
-- @requires data/events/anomalies.parquet
SELECT day::XAXIS, score::LINECHART, 'Anomaly score (optional output)'::TITLE
FROM 'data/events/anomalies.parquet';

SELECT 'Switch the view'::TAB;
SELECT 12::COL;
SELECT 'Compact inputs for a few choices: **::BUTTONS** is a segmented button group (one option pressed; the arrow keys move between them) and **::TOGGLE** an on/off switch whose variable is a BOOLEAN. Together they make a **metric / dimension switcher**: the chart picks its measure with **CASE getvariable(''metric'') WHEN …** and its grouping column with **COLUMNS(getvariable(''dim''))**. Like every input they land in the URL (\`?v.metric=Peak%20day\`).'::MARKDOWN, 'What this shows'::TITLE;

SELECT 'View'::GROUP;
SELECT m AS metric::BUTTONS, 'Measure'::LABEL FROM (VALUES (1, 'Sessions'), (2, 'Peak day'), (3, 'Active days')) t(o, m) ORDER BY o;
SELECT d AS dim::BUTTONS, 'Split by'::LABEL FROM (VALUES (1, 'channel'), (2, 'region')) t(o, d) ORDER BY o;
SELECT false AS launches_only::TOGGLE, 'Launch days only'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT COLUMNS(getvariable('dim'))::XAXIS,
       CASE getvariable('metric')
         WHEN 'Sessions' THEN sum(n)
         WHEN 'Peak day' THEN max(n)
         ELSE count(DISTINCT day)
       END::BARCHART,
       (getvariable('metric') || ' by ' || getvariable('dim'))::TITLE
FROM events
WHERE NOT getvariable('launches_only') OR note = 'launch'
GROUP BY ALL ORDER BY 2 DESC;

SELECT 'Date range'::TAB;
SELECT 12::COL;
SELECT 'A **::DATERANGE** control — two columns (from / to) become a pair of linked date pickers seeded from the data''s min/max. **Drag the dates** and the KPI and chart below recompute for the selected window via **getvariable(''from_day'')** / **getvariable(''to_day'')**.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 'Window'::GROUP;
SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE FROM events;
SELECT 1::ENDGROUP;

SELECT 4::COL;
SELECT sum(n)::METRIC, 'Total sessions'::LABEL FROM events
WHERE day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE;
SELECT 8::COL;
SELECT day::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED, 'Sessions by day (in range)'::TITLE FROM events
WHERE day BETWEEN getvariable('from_day')::DATE AND getvariable('to_day')::DATE
GROUP BY ALL ORDER BY day, channel;

SELECT 'Cross-filter'::TAB;
SELECT 12::COL;
SELECT 'Two **independent named cross-filters**, opted in per table with **::ROWSELECT**. Each table emits its own variable, named after its first column: the SKU table is \`''multi''\` — **tick several SKUs** and getvariable(''sku'') is a list — the market table is \`''single''\` (getvariable(''market'') is one value). The KPI and chart below filter by *both* at once. Click a row again to clear it; tables without ::ROWSELECT are not clickable.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 4::COL; SELECT 'multi'::ROWSELECT;
SELECT sku, sum(amount) AS total ::TABLE FROM sales2 GROUP BY sku ORDER BY total DESC;
SELECT 4::COL; SELECT 'single'::ROWSELECT;
SELECT region AS market, sum(amount) AS total ::TABLE FROM sales2 GROUP BY region ORDER BY total DESC;

SELECT 4::COL;
SELECT sum(amount)::METRIC, 'Total (filtered)'::LABEL FROM sales2
WHERE (len(COALESCE(getvariable('sku'), []::VARCHAR[])) = 0 OR list_contains(getvariable('sku'), sku))
  AND (COALESCE(getvariable('market'),'') = '' OR region = getvariable('market'));

SELECT 12::COL;
SELECT month::XAXIS, sum(amount)::LINECHART, 'Monthly (by SKU & market)'::TITLE FROM sales2
WHERE (len(COALESCE(getvariable('sku'), []::VARCHAR[])) = 0 OR list_contains(getvariable('sku'), sku))
  AND (COALESCE(getvariable('market'),'') = '' OR region = getvariable('market'))
GROUP BY month ORDER BY month;`,
      "Selections & drill": `-- A small sales cube: region > country > city, three channels, 26 weeks.
CREATE OR REPLACE TABLE geo AS SELECT * FROM (VALUES
  ('EU','Germany','Berlin'),('EU','Germany','Munich'),('EU','France','Paris'),('EU','France','Lyon'),
  ('Americas','USA','New York'),('Americas','USA','Chicago'),('Americas','Canada','Toronto'),
  ('APAC','Japan','Tokyo'),('APAC','Japan','Osaka'),('APAC','Australia','Sydney')
) t(region, country, city);
CREATE OR REPLACE TABLE orders AS
SELECT DATE '2024-01-01' + (7 * w)::INT AS week, g.region, g.country, g.city, c.channel,
       (abs(hash(g.city || c.channel || w)) % 60 + 20 + 2 * w) AS revenue
FROM geo g, (VALUES ('web'),('app'),('store')) c(channel), range(26) r(w);

SELECT 'Selections & drill — named selections, brushing, drill-down and drill-through'::LABEL;

SELECT 'Named selections'::TAB;
SELECT 12::COL;
SELECT 'Each chart below writes **its own variable** with **::SELECT_AS** instead of the shared getvariable(''selected''), so the selections compose. **Regions** is \`''regions multi''\`: click a bar to pick one, **shift/ctrl-click** to add more — getvariable(''regions'') is a list. **Channel** is single: getvariable(''channel'') is one value. On the **weekly line**, open the toolbox **▧ brush** and drag across a few weeks: the brushed range lands in getvariable(''week_from'') / getvariable(''week_to''). Each selection shows as a chip (× clears it); **Esc** or a click on empty space clears them all, and the address bar keeps them (\`?v.regions=EU&v.week_from=…\`).'::MARKDOWN, 'What this shows'::TITLE;

SELECT 4::COL; SELECT 'regions multi'::SELECT_AS;
SELECT region::XAXIS, sum(revenue)::BARCHART, 'Regions (click, shift-click)'::TITLE FROM orders
WHERE channel = coalesce(nullif(getvariable('channel'), ''), channel)
  AND week BETWEEN coalesce(getvariable('week_from')::DATE, week) AND coalesce(getvariable('week_to')::DATE, week)
GROUP BY region ORDER BY 2 DESC;
SELECT 4::COL; SELECT 'channel'::SELECT_AS;
SELECT channel::XAXIS, sum(revenue)::BARCHART, ''::FLIP, 'Channel (click)'::TITLE FROM orders
WHERE (len(getvariable('regions')) = 0 OR list_contains(getvariable('regions'), region))
  AND week BETWEEN coalesce(getvariable('week_from')::DATE, week) AND coalesce(getvariable('week_to')::DATE, week)
GROUP BY channel ORDER BY 2;
SELECT 4::COL;
SELECT sum(revenue)::MONEY, 'Revenue in selection'::LABEL FROM orders
WHERE (len(getvariable('regions')) = 0 OR list_contains(getvariable('regions'), region))
  AND channel = coalesce(nullif(getvariable('channel'), ''), channel)
  AND week BETWEEN coalesce(getvariable('week_from')::DATE, week) AND coalesce(getvariable('week_to')::DATE, week);

SELECT 12::COL; SELECT 'week'::SELECT_AS;
SELECT week::XAXIS, sum(revenue)::LINECHART, 'Weekly revenue (▧ brush a range)'::TITLE FROM orders
WHERE (len(getvariable('regions')) = 0 OR list_contains(getvariable('regions'), region))
  AND channel = coalesce(nullif(getvariable('channel'), ''), channel)
GROUP BY week ORDER BY week;

SELECT 'Drill-down'::TAB;
SELECT 12::COL;
SELECT '**::DRILL** turns one chart into a drill-down: \`''region > country > city''\` shows regions first — **click a bar** to see that region''s countries, click again for its cities. The **breadcrumb** above the chart goes back (All › EU › Germany). The chart reads getvariable(''drill_level'') to pick its x column, and every level above sets a variable named after it (getvariable(''region''), getvariable(''country''); empty until drilled), so the trend on the right follows the drill too. The path is in the URL (\`?v.drill_path=EU\`).'::MARKDOWN, 'What this shows'::TITLE;

SELECT 7::COL; SELECT 'region > country > city'::DRILL;
SELECT CASE getvariable('drill_level') WHEN 'region' THEN region WHEN 'country' THEN country ELSE city END AS place ::XAXIS,
       sum(revenue)::BARCHART, ('Revenue by ' || getvariable('drill_level'))::TITLE
FROM orders
WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)
GROUP BY 1 ORDER BY 2 DESC;
SELECT 5::COL;
SELECT week::XAXIS, sum(revenue)::AREACHART,
       ('Weekly revenue — ' || coalesce(nullif(getvariable('country'), ''), nullif(getvariable('region'), ''), 'all regions'))::TITLE
FROM orders
WHERE getvariable('region') IN ('', region) AND getvariable('country') IN ('', country)
GROUP BY week ORDER BY week;

SELECT 'Drill-through'::TAB;
SELECT 12::COL;
SELECT 'A **::LINK** column is a drill-through target. Its value is a template — \`{value}\` is the clicked bar (or the table row''s first column), \`{column}\` any column of a table row. \`#Tab?v.x={value}\` opens a tab with variables set (an input only accepts one of its own options), \`/d/<id>?v.x={value}\` opens another served dashboard pre-filtered, an https: URL opens in a new browser tab. **Click a city bar or a table link** to open its detail page; **click a country** for its Wikipedia article.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 6::COL;
SELECT city::XAXIS, sum(revenue)::BARCHART, '#City detail?v.pick_city={value}'::LINK, 'Cities — click for the detail page'::TITLE
FROM orders GROUP BY city ORDER BY 2 DESC;
SELECT 6::COL;
SELECT country::XAXIS, sum(revenue)::BARCHART, 'https://en.wikipedia.org/wiki/{value}'::LINK, 'Countries — click opens Wikipedia'::TITLE
FROM orders GROUP BY country ORDER BY 2 DESC;
SELECT 12::COL;
SELECT city::TABLE, country, region, sum(revenue) AS revenue ::MONEY,
       '#City detail?v.pick_city={city}' AS detail ::LINK
FROM orders GROUP BY ALL ORDER BY revenue DESC;

SELECT 'City detail'::TAB;
SELECT 'City'::GROUP;
SELECT DISTINCT city AS pick_city ::DROPDOWN FROM geo ORDER BY 1;
SELECT 1::ENDGROUP;
SELECT 4::COL;
SELECT sum(revenue)::MONEY, ('Revenue — ' || getvariable('pick_city'))::LABEL FROM orders WHERE city = getvariable('pick_city');
SELECT 8::COL;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, ('Weekly revenue by channel — ' || getvariable('pick_city'))::TITLE
FROM orders WHERE city = getvariable('pick_city') GROUP BY ALL ORDER BY week, channel;`,
    },
  },

  {
    group: "Tables",
    items: {
      "Formatting & paging": `CREATE OR REPLACE TABLE fc AS SELECT * FROM (VALUES
  ('SKU-A',12400, 4.2,'on track',  8.5),
  ('SKU-B', 7300,11.8,'at risk',  -3.1),
  ('SKU-C',21850, 6.5,'on track',  2.7),
  ('SKU-D', 4200,23.4,'breach',  -12.0)
) t(sku, forecast, mape, status, growth);
CREATE OR REPLACE TABLE hist AS SELECT * FROM (VALUES
  ('SKU-A',1,100),('SKU-A',2,108),('SKU-A',3,104),('SKU-A',4,120),('SKU-A',5,126),
  ('SKU-B',1, 90),('SKU-B',2, 85),('SKU-B',3, 70),('SKU-B',4, 72),('SKU-B',5, 66),
  ('SKU-C',1,200),('SKU-C',2,205),('SKU-C',3,210),('SKU-C',4,208),('SKU-C',5,215),
  ('SKU-D',1, 60),('SKU-D',2, 52),('SKU-D',3, 48),('SKU-D',4, 40),('SKU-D',5, 35)
) t(sku, m, sales);

SELECT 'Tables — rich per-column formatting & scalable paging'::LABEL;

-- Rich formatting: each column picks its own presentation role.
SELECT 'Formatted'::TAB;
SELECT 12::COL;
SELECT 'A **rich data table** — each column carries its own formatting role: **::MONEY** currency, **::COLORSCALE** heat-shaded cells, **::BADGE** status pills, **::TREND** an up/down arrow, and an in-cell **::SPARKLINE**. One SELECT, per-column presentation. The second table adds options for the next table: **::TOTALS** picks each total (click a total''s tag to change it), **::COLGROUP** groups the header, **::FLAG** marks cells with ⚠, **::HIGHLIGHT** ranks the top n, and **::COLORSCALE_HIGH_GOOD** / **::COLORSCALE_DIVERGING** shade where high is good / around zero. The third groups its rows with **::SUBTOTAL** and marks each margin good / warn / bad with **::THRESHOLDS**.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 12::COL;
SELECT sku::TABLE,
       forecast::MONEY,
       mape AS "MAPE %" ::COLORSCALE,
       status::BADGE,
       growth AS "growth %" ::TREND,
       (SELECT list(sales ORDER BY m) FROM hist WHERE hist.sku = fc.sku) AS trend ::SPARKLINE
FROM fc ORDER BY forecast DESC;

-- Totals per column (::TOTALS; click a total's tag to change it), a grouped
-- header (::COLGROUP), flagged cells (::FLAG), the top n (::HIGHLIGHT), colour
-- scales where high is good / around zero, and the currency once in the header.
SELECT 12::COL;
SELECT 'revenue: sum, Margin %: mean, accuracy: median, bias: min'::TOTALS;
SELECT 'Money: plan, revenue | Forecast quality: accuracy, bias'::COLGROUP;
SELECT 'Margin % < 12'::FLAG;
SELECT 'revenue: top 2'::HIGHLIGHT;
SELECT region::TABLE, plan::MONEY, revenue::MONEY, margin AS "Margin %",
       accuracy::COLORSCALE_HIGH_GOOD, bias::COLORSCALE_DIVERGING,
       'Regions — a chosen total per column, grouped header, flags and top 2'::TITLE
FROM (VALUES ('North', 4.2e6, 4.61e6, 18.2, 0.94, 2.1), ('South', 3.8e6, 3.42e6, 12.5, 0.88, -5.3),
             ('East', 5.1e6, 5.35e6, 21.0, 0.97, 0.4), ('West', 2.9e6, 2.71e6, 9.8, 0.81, -3.2),
             ('Central', 3.3e6, 3.55e6, 15.1, 0.91, 1.6)) v(region, plan, revenue, margin, accuracy, bias)
ORDER BY revenue DESC;

-- Subtotals (::SUBTOTAL): the rows grouped by a column, a subtotal row after
-- each group; sorting by another column sorts within the groups.
SELECT 12::COL;
SELECT 'region'::SUBTOTAL;
SELECT 'Margin %: mean'::TOTALS;
SELECT 'Margin %: <14:bad; <18:warn; good'::THRESHOLDS;
SELECT region::TABLE, store, revenue::MONEY, orders, margin AS "Margin %",
       'Stores by region — a subtotal per region, margins by status (sort any column: it sorts within the regions)'::TITLE
FROM (VALUES ('North', 'Hamburg', 1.92e6, 18400, 19.1), ('North', 'Bremen', 1.21e6, 11900, 17.4),
             ('North', 'Kiel', 0.68e6, 7100, 16.0), ('South', 'Munich', 2.05e6, 19800, 13.9),
             ('South', 'Stuttgart', 1.37e6, 13300, 11.2), ('East', 'Berlin', 3.10e6, 29400, 22.3),
             ('East', 'Leipzig', 1.08e6, 10200, 18.7), ('East', 'Dresden', 1.17e6, 11600, 20.5))
     v(region, store, revenue, orders, margin)
ORDER BY region, revenue DESC;

-- Paging: a plain ::TABLE returns everything and the browser paginates it.
SELECT '1,000 rows (client)'::TAB;
SELECT 12::COL;
SELECT 'Two ways to page big results. A plain **::TABLE** returns all rows and the browser paginates them (fine for ~1k). The next tab shows **::PAGED**, which pages in DuckDB so it scales to 100k+ rows.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 'ID-' || lpad(i::VARCHAR, 4, '0') AS id, ['app','web','api','cli'][(i % 4) + 1] AS channel,
       ((i * 37) % 100) AS score, ((i * 7) % 500) AS events ::TABLE
FROM range(1, 1001) t(i) ORDER BY i;

-- Paging: ::PAGED runs LIMIT/OFFSET + COUNT in DuckDB — one page at a time, so it
-- scales to huge / remote tables (parquet in S3, MotherDuck). Sorting is server-side.
SELECT '100,000 rows (::PAGED)'::TAB;
SELECT 12::COL;
SELECT '**::PAGED** runs LIMIT/OFFSET + COUNT in DuckDB — **one page at a time** — so it scales to huge / remote tables (Parquet in S3, MotherDuck) with server-side sorting. Same grid, 100k rows, still instant.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 'score: median'::TOTALS;
SELECT 'ID-' || lpad(i::VARCHAR, 6, '0') AS id, ['app','web','api','cli'][(i % 4) + 1] AS channel,
       ((i * 37) % 1000) AS score, ((i * 91) % 100) AS load_pct ::PAGED
FROM range(1, 100001) t(i);`,
    },
  },

  {
    group: "Layout",
    items: {
      "Groups, tabs & height": `${SALES}

SELECT 'Layout — groups, tabs & height'::LABEL;
SELECT 12::COL;
SELECT 'The **layout primitives**: **::GROUP** boxes cluster KPIs/panels together, **::COL** sets a panel''s grid width (1–12), **::HEIGHT** sets its pixel height, and **::TAB** / **::SUBTAB** organise panels into (nested) tabs. Mix them to compose any dashboard shape from plain SQL.'::MARKDOWN, 'What this shows'::TITLE;

-- KPIs in a ::GROUP box (compact strip). They react to plot clicks: clicking a
-- channel segment/bar below sets getvariable('selected'), and each KPI filters
-- by it ( IN ('', channel) → all when nothing is selected ). Click empty space
-- to clear.
SELECT 'Key metrics — click a bar to filter by channel'::GROUP;
SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales WHERE getvariable('selected') IN ('', channel);
SELECT sum(n)::COMPACT, 'Sessions'::LABEL FROM sales WHERE getvariable('selected') IN ('', channel);
SELECT round(avg(n),1)::METRIC, 'Avg / week'::LABEL FROM sales WHERE getvariable('selected') IN ('', channel);
SELECT 1::ENDGROUP;

SELECT 1::COLUMNS;

-- top-level ::TAB, each with nested ::SUBTAB; ::HEIGHT sets a taller box
SELECT 'Revenue'::TAB;
SELECT 'By week'::SUBTAB;
SELECT 420::HEIGHT;
SELECT week::XAXIS, channel::CATEGORY, sum(revenue)::BARCHART_STACKED, '€'::YFORMAT, 'Revenue by week (tall)'::TITLE
FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 'By channel'::SUBTAB;
SELECT channel::XAXIS, sum(revenue)::BARCHART, 'Revenue by channel'::TITLE FROM sales GROUP BY ALL ORDER BY channel;

SELECT 'Sessions'::TAB;
SELECT 'Trend'::SUBTAB;
SELECT week::XAXIS, channel::CATEGORY, sum(n)::LINECHART, 'Sessions trend'::TITLE FROM sales GROUP BY ALL ORDER BY week, channel;
SELECT 'Share'::SUBTAB;
SELECT channel::CATEGORY, sum(n)::PIE, 'Session share'::TITLE FROM sales GROUP BY ALL;`,
    },
  },
];

// Present the examples as a learning path: start → the visual vocabulary →
// making it interactive → tables → page layout → theming. The showcases of
// the anofox DuckDB extensions live in their own sidebar area ("anofox
// extensions", groups with `ext: true`), one group per extension, so they
// never mix with the core feature tour (and a missing extension never blocks
// the core demos).
const STATS_GROUP = "Statistics";
const FORECAST_GROUP = "Forecast";
const TABULAR_GROUP = "Tabular";
const OPTIMIZE_GROUP = "Optimize";
const DECIDE_GROUP = "Decide";
const INVENTORY_GROUP = "Inventory";
const THEMING_GROUP = "Theming";
const EXT_GROUPS = [STATS_GROUP, FORECAST_GROUP, TABULAR_GROUP, OPTIMIZE_GROUP, DECIDE_GROUP, INVENTORY_GROUP];
const GROUP_ORDER = ["Start here", "Charts", "Interactivity", "Tables", "Layout", THEMING_GROUP, ...EXT_GROUPS];
const sortGroups = () =>
  SAMPLE_GROUPS.sort((a, b) => GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group));
sortGroups();

const SAMPLES = Object.fromEntries(SAMPLE_GROUPS.flatMap((g) => Object.entries(g.items)));

// Examples kept in their own files under web/examples/ — the single source of
// a dashboard that is also run natively (examples/<name>/showcase.sql links to
// it). Fetched once at boot; a missing file just leaves the example out.
// "stored results" showcases read tables computed natively (web/data/<ext>/,
// scripts/gen-showcase-data.sh) because their extension can't run in
// DuckDB-Wasm; their names and dashboards say so.
const FILE_SAMPLES = [
  { group: "Charts", name: "How does data move through our pipeline?", url: "examples/diagram-pipeline.sql" },
  { group: "Interactivity", name: "Period & KPIs", url: "examples/period-kpis.sql" },
  { group: "Interactivity", name: "How will demand develop after today?", url: "examples/motion-demand.sql" },
  { group: "Charts", name: "When could we run out of stock?", url: "examples/stock-outlook.sql" },
  { group: THEMING_GROUP, name: "A sales cockpit in your brand", url: "examples/theming-branded.sql" },
  { group: THEMING_GROUP, name: "A German branch report", url: "examples/theming-de-locale.sql" },
  {
    group: STATS_GROUP,
    name: "What sets the price of a home?",
    url: "examples/statistics-showcase.sql",
  },
  { group: STATS_GROUP, name: "Did the new checkout lift conversion?", url: "examples/statistics-ab-test.sql" },
  { group: STATS_GROUP, name: "Why do customers cancel?", url: "examples/statistics-churn-drivers.sql" },
  { group: FORECAST_GROUP, name: "Will any substation exceed its capacity next week?", url: "examples/forecast-energy-load.sql" },
  { group: FORECAST_GROUP, name: "How many agents do we need per half-hour?", url: "examples/forecast-callcentre-staffing.sql" },
  {
    group: TABULAR_GROUP,
    name: "Can we switch on the new CRM?",
    url: "examples/tabular-showcase.sql",
  },
  { group: TABULAR_GROUP, name: "Is today's supplier feed safe to load?", url: "examples/tabular-feed.sql" },
  { group: TABULAR_GROUP, name: "Which duplicate customers should we merge?", url: "examples/tabular-dedup.sql" },
  {
    group: OPTIMIZE_GROUP,
    name: "How do we plan a busy day at the distribution centre?",
    url: "examples/optimize-showcase.sql",
  },
  { group: OPTIMIZE_GROUP, name: "Which line runs which order, with the least changeover?", url: "examples/optimize-production.sql" },
  { group: OPTIMIZE_GROUP, name: "Which van serves which stop tomorrow?", url: "examples/optimize-delivery.sql" },
  {
    group: OPTIMIZE_GROUP,
    name: "What should we order this week to sell the most?",
    url: "examples/optimize-replenishment.sql",
  },
  {
    group: DECIDE_GROUP,
    name: "Can a model triage the support inbox?",
    url: "examples/decide-showcase.sql",
  },
  { group: DECIDE_GROUP, name: "Which claims can we fast-track?", url: "examples/decide-claims.sql" },
  { group: DECIDE_GROUP, name: "Who should sales call first?", url: "examples/decide-leads.sql" },
  {
    group: INVENTORY_GROUP,
    name: "Where will stock run short, and can we trust the plan?",
    url: "examples/inventory-twin.sql",
  },
];
async function loadFileSamples() {
  // Fetch in parallel, register in FILE_SAMPLES order (stable sidebar order).
  const texts = await Promise.all(
    FILE_SAMPLES.map(async (f) => {
      try {
        const r = await fetch(f.url);
        return r.ok ? await r.text() : null;
      } catch (_) {
        return null; /* offline / file:// — skip the example */
      }
    })
  );
  FILE_SAMPLES.forEach((f, i) => {
    const sql = texts[i];
    if (sql == null) return;
    let g = SAMPLE_GROUPS.find((x) => x.group === f.group);
    if (!g) SAMPLE_GROUPS.push((g = { group: f.group, ext: EXT_GROUPS.includes(f.group), items: {} }));
    g.items[f.name] = sql;
    SAMPLES[f.name] = sql;
  });
  sortGroups();
}

const $ = (id) => document.getElementById(id);
const status = (t) => ($("status").textContent = t);

// SQL syntax highlighting for the editor overlay (no dependency).
const SQL_KW =
  /^(SELECT|FROM|WHERE|GROUP|ORDER|BY|HAVING|LIMIT|OFFSET|AS|AND|OR|NOT|IN|IS|NULL|LIKE|ILIKE|BETWEEN|CASE|WHEN|THEN|ELSE|END|JOIN|LEFT|RIGHT|INNER|OUTER|FULL|CROSS|ON|USING|UNION|EXCEPT|INTERSECT|ALL|DISTINCT|CREATE|REPLACE|TEMP|TEMPORARY|TABLE|VIEW|IF|EXISTS|INSERT|INTO|VALUES|UPDATE|DELETE|SET|VARIABLE|WITH|DESC|ASC|OVER|PARTITION|FILTER|CAST|COALESCE|NULLIF|COUNT|SUM|AVG|MIN|MAX|ROUND|ABS|FLOOR|CEIL|LENGTH|LOWER|UPPER|SUBSTR|LPAD|LIST|RANGE|GETVARIABLE|LIST_CONTAINS|INSTALL|LOAD|ATTACH|PRAGMA|SUMMARIZE|DESCRIBE|SHOW|CALL)$/i;
function highlightSQL(code) {
  const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
  const token = /--[^\n]*|'(?:[^']|'')*'|::\w+|\b\d+(?:\.\d+)?\b|[A-Za-z_]\w*|\s+|[^\sA-Za-z0-9_']+/g;
  let out = "",
    m;
  while ((m = token.exec(code))) {
    const t = m[0];
    if (t.startsWith("--")) out += `<span class="com">${esc(t)}</span>`;
    else if (t[0] === "'") out += `<span class="str">${esc(t)}</span>`;
    else if (t.startsWith("::")) out += `<span class="role">${esc(t)}</span>`;
    else if (/^\d/.test(t)) out += `<span class="num">${esc(t)}</span>`;
    else if (SQL_KW.test(t)) out += `<span class="kw">${esc(t)}</span>`;
    else out += esc(t);
  }
  return out + "\n";
}
function syncHL() {
  const hl = $("hl");
  if (!hl) return;
  hl.innerHTML = highlightSQL($("sql").value);
  hl.scrollTop = $("sql").scrollTop;
  hl.scrollLeft = $("sql").scrollLeft;
}

let backend = "wasm"; // "live" (HTTP /query) or "wasm" (DuckDB-Wasm)
let conn = null;
// Served (locked) mode: the server owns the SQL — there is no SQL endpoint.
// The client names a statement of the served dashboard by its plan index and
// sends variable VALUES (JSON), which the server binds as typed literals:
//   POST /api/panel {dashboard, panel, vars, page?}
// SET VARIABLE statements are captured here (never sent), so every request is
// self-contained (multi-user safe — no shared session state).
let servedMode = false;
const servedVars = {};
let servedIndex = null; // planned statement SQL -> plan index
// Superseding: a request with a `kind` aborts this page's previous request of
// the same panel + kind (a ::PAGED table's page / scroll block), and names
// itself in X-Anofox-Supersede so the server interrupts the old query too.
// The viewer id is 128 random bits, so nobody can cancel another viewer's.
const servedViewer = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
const servedAborts = new Map(); // "panel/kind" -> AbortController
let servedTables = 0; // per-table-instance suffix for the kinds
const isAbort = (e) => e && e.name === "AbortError";

// Parse a SQL literal the client itself produced ('…', [ '…', … ], number,
// TRUE/FALSE/NULL) back into a JSON value for /api/panel.
function sqlLiteralToJson(lit) {
  const t = String(lit).trim();
  if (/^null$/i.test(t)) return null;
  if (/^true$/i.test(t)) return true;
  if (/^false$/i.test(t)) return false;
  if (t.startsWith("'") && t.endsWith("'") && t.length >= 2) return t.slice(1, -1).replace(/''/g, "'");
  const list = t.replace(/::VARCHAR\[\]$/i, "");
  if (list.startsWith("[") && list.endsWith("]")) {
    const inner = list.slice(1, -1).trim();
    if (!inner) return [];
    const strs = [...inner.matchAll(/'((?:[^']|'')*)'/g)].map((m) => m[1].replace(/''/g, "'"));
    if (strs.length) return strs;
    return inner.split(",").map((x) => sqlLiteralToJson(x));
  }
  const n = Number(t);
  return t !== "" && Number.isFinite(n) ? n : t;
}

async function servedRequest(sql, page, kind) {
  if (!servedIndex) {
    servedIndex = new Map();
    JSON.parse(plan(window.__served.sql)).forEach((st, i) => {
      if (!servedIndex.has(st.sql)) servedIndex.set(st.sql, i);
    });
  }
  const idx = servedIndex.get(page ? page.base : sql);
  if (idx === undefined) throw new Error("this dashboard is served locked: only its own statements can run");
  const body = { dashboard: window.__served.id, panel: idx, vars: servedVars };
  if (page) {
    const { base, ...rest } = page;
    body.page = rest;
  }
  const headers = { "Content-Type": "application/json" };
  const slot = kind && `${idx}/${kind}`;
  const ac = slot && new AbortController();
  if (slot) {
    servedAborts.get(slot)?.abort();
    servedAborts.set(slot, ac);
    headers["X-Anofox-Supersede"] = `${servedViewer}/${slot}`;
  }
  try {
    let r;
    for (let attempt = 1; ; attempt++) {
      r = await fetch("/api/panel", { method: "POST", headers, body: JSON.stringify(body), signal: ac?.signal });
      // 503: the server has too many results in flight or queries waiting;
      // try again after its Retry-After (twice at most).
      if (r.status !== 503 || attempt > 2) break;
      const wait = Math.min(5, Number(r.headers.get("Retry-After")) || 1) * attempt;
      await new Promise((res) => setTimeout(res, wait * 1000));
      if (ac?.signal.aborted) throw new DOMException("superseded", "AbortError");
    }
    if (r.status === 409) throw new DOMException("superseded", "AbortError");
    if (!r.ok) throw new Error(await r.text());
    return (await r.text()) || "[]";
  } finally {
    if (slot && servedAborts.get(slot) === ac) servedAborts.delete(slot);
  }
}
let db = null; // AsyncDuckDB (needed to register remote geo files for the maps)

// The map examples read remote GeoJSON with DuckDB's `spatial` extension. Load
// it and register the two datasets on first use (memoised), so `ST_Read(...)`
// works from plain example SQL without a server.
let geoReady = null;
async function ensureGeo() {
  if (backend === "live") return; // the live backend brings its own spatial setup
  if (!db) throw new Error("in-browser DuckDB not ready");
  if (!geoReady) {
    geoReady = (async () => {
      await conn.query("INSTALL spatial; LOAD spatial;");
      const reg = async (name, url) => {
        const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
        await db.registerFileBuffer(name, bytes);
      };
      await reg(
        "countries.geojson",
        "https://cdn.jsdelivr.net/gh/nvkelso/natural-earth-vector@master/geojson/ne_110m_admin_0_countries.geojson"
      );
      await reg(
        "quakes.geojson",
        "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_month.geojson"
      );
    })().catch((e) => {
      geoReady = null; // let a later run retry after a transient network failure
      throw e;
    });
  }
  return geoReady;
}

// The anofox DuckDB-Wasm extensions (anofox_forecast, anofox_statistics) are
// served same-origin from web/localext/<duckdb version>/<platform>/ — fixed
// copies of the get.erpl.io builds made by scripts/fetch-wasm-extensions.sh
// (get.erpl.io sends no CORS headers, and its v1.5.6 files carry a duplicated
// signature section, DataZooDE/anofox-statistics#192).
// TODO: INSTALL … FROM 'https://get.erpl.io' once #192 and CORS are fixed.
// Each extension loads once, on first use by a statement (memoised; a failure
// is remembered so we don't re-hammer the server for every panel).
const extReady = {};
async function ensureExtension(name) {
  if (backend === "live") return; // the native backend brings its own extensions
  if (!conn) throw new Error("in-browser DuckDB not ready");
  if (!extReady[name]) {
    extReady[name] = (async () => {
      // json (from the default repo) is needed by some anofox macros.
      try {
        await conn.query("INSTALL json; LOAD json;");
      } catch (_) {
        /* usually bundled */
      }
      const repo = new URL("localext", document.baseURI).href.replace(/\/$/, "");
      await conn.query(`SET custom_extension_repository='${repo}';`);
      try {
        await conn.query(`INSTALL ${name};`);
        await conn.query(`LOAD ${name};`);
      } finally {
        // Restore the default repo so other autoloaded extensions (parquet,
        // spatial, …) still resolve — ours only hosts the anofox extensions.
        await conn.query("RESET custom_extension_repository;");
      }
    })().catch((e) => {
      throw new Error(
        `Couldn't load the ${name} wasm extension in the browser (expected at ` +
          `localext/<version>/<platform>/${name}.duckdb_extension.wasm — run ` +
          `scripts/fetch-wasm-extensions.sh). Underlying error: ` +
          (e.message || e)
      );
    });
  }
  return extReady[name];
}

// The code of a statement without its string literals and comments, so a
// title or a note that merely names a function ('… with ts_forecast_by')
// never loads an extension.
function sqlCode(sql) {
  return String(sql).replace(/'(?:[^']|'')*'|--[^\n]*|\/\*[\s\S]*?\*\//g, " ");
}
// Statements that call anofox_statistics (its producers, fit/test aggregates).
const STATS_RE =
  /\b(?:\w+_(?:tidy|glance|augment|prediction|effects|partial_residuals|rolling_tidy|path|cv|fit_predict)_by|quantile_process_by|(?:roc|pr|calibration)_curve_by|kaplan_meier_by|p?acf_by|tukey_hsd_by|pairwise_t_test_by|dunn_test_by|\w+_fit_agg|\w+_fit_predict_agg|\w+_test_agg|\w+_fit_predict|anofox_contract_version|anofox_statistics)\b/i;

// Forecast example: load a small M5 parquet into the browser DB, and (for
// statements that need it) load anofox_forecast so ts_forecast_by() runs
// client-side. The parquet always loads; the extension step is separate so a
// load failure is reported clearly instead of a cryptic "function does not
// exist".
let fcParquet = null;
async function ensureForecast(sql) {
  if (backend === "live") return; // native backend resolves paths + extension itself
  if (!db) throw new Error("in-browser DuckDB not ready");
  // 1. Register the parquet (memoised) — only for statements that read it.
  if (/m5_monthly/i.test(sql) && !fcParquet) {
    fcParquet = (async () => {
      const bytes = new Uint8Array(await (await fetch("m5_monthly.parquet")).arrayBuffer());
      await db.registerFileBuffer("m5_monthly.parquet", bytes);
    })().catch((e) => {
      fcParquet = null;
      throw e;
    });
  }
  if (fcParquet) await fcParquet;
  // 2. The extension, only for statements that use it.
  if (/\bts_\w+\b|\banofox_forecast\b/i.test(sqlCode(sql))) await ensureExtension("anofox_forecast");
}

// Stored showcase results: an example that reads 'data/<ext>/<table>.csv'
// (web/data/, written natively by scripts/gen-showcase-data.sh for extensions
// that can't run in the browser) gets each file fetched and registered with
// DuckDB-Wasm on first use (memoised per file; a failure is retried later).
const dataFiles = {};
async function ensureDataFiles(sql) {
  if (backend === "live") return; // the native backend reads them from disk
  if (!db) throw new Error("in-browser DuckDB not ready");
  const names = new Set([...sql.matchAll(/'(data\/[\w-]+\/[\w.-]+\.(?:csv|parquet))'/g)].map((m) => m[1]));
  await Promise.all(
    [...names].map((n) => {
      if (!dataFiles[n])
        dataFiles[n] = (async () => {
          const r = await fetch(n);
          if (!r.ok) throw new Error(`couldn't fetch ${n} (HTTP ${r.status})`);
          await db.registerFileBuffer(n, new Uint8Array(await r.arrayBuffer()));
        })().catch((e) => {
          delete dataFiles[n];
          throw e;
        });
      return dataFiles[n];
    })
  );
}

// Run one SQL statement and return its rows as a JSON string ([{c0,…}, …]).
// `page` (served mode only) describes a ::PAGED request structurally —
// {base, count?, limit?, offset?, sort?, desc?, filter?} — so the locked server
// builds the paging SQL itself. `kind` (served mode only): a newer request of
// the same panel + kind supersedes this one (it then rejects with an AbortError).
async function runSql(sql, page, kind) {
  const t0 = performance.now();
  try {
    return await runSqlInner(sql, page, kind);
  } finally {
    // timing for the perf check (scripts/perf-e2e.mjs reads the "dp:sql" entries)
    performance.measure("dp:sql", { start: t0, detail: String(sql).slice(0, 120) });
  }
}
async function runSqlInner(sql, page, kind) {
  const setv = /^\s*SET\s+VARIABLE\s+([A-Za-z_]\w*)\s*=\s*([\s\S]+?);?\s*$/i.exec(sql);
  if (setv) dpVarNow[setv[1]] = setv[2]; // what changed since the last run (markPending)
  const code = sqlCode(sql); // (the data/… file check below reads the strings)
  if (/\bST_Read\b|\bspatial\b/i.test(code)) await ensureGeo();
  if (/\bts_\w+\b|\banofox_forecast\b/i.test(code) || /m5_monthly/i.test(sql)) await ensureForecast(sql);
  if (STATS_RE.test(code)) await ensureExtension("anofox_statistics");
  if (!servedMode && /'data\/[\w-]+\//.test(sql)) await ensureDataFiles(sql);
  if (servedMode) {
    // Capture a SET VARIABLE (never sent as SQL) …
    const m = sql.match(/^\s*SET\s+VARIABLE\s+([A-Za-z_]\w*)\s*=\s*([\s\S]+?);?\s*$/i);
    if (m) {
      servedVars[m[1]] = sqlLiteralToJson(m[2]);
      return "[]";
    }
    // … and send the statement's plan index + the variable values.
    return servedRequest(sql, page, kind);
  }
  if (backend === "live") {
    const r = await fetch("/query", { method: "POST", body: sql });
    if (!r.ok) throw new Error(await r.text());
    return (await r.text()) || "[]";
  }
  const res = await conn.query(sql);
  // DuckDB-Wasm returns DATE/TIMESTAMP as epoch numbers; convert those columns
  // back to ISO strings so date variables and date axes read as YYYY-MM-DD.
  const dateCols = new Map(); // name -> "date" | "time"
  const decCols = new Map(); // name -> scale (DECIMAL comes back as the unscaled mantissa)
  try {
    for (const f of res.schema.fields) {
      const t = String(f.type);
      if (/date/i.test(t)) dateCols.set(f.name, "date");
      else if (/timestamp/i.test(t)) dateCols.set(f.name, "time");
      else if (/decimal/i.test(t)) decCols.set(f.name, Number(f.type.scale) || 0);
    }
  } catch (_) {}
  const rows = res.toArray().map((row) => {
    const o = row.toJSON();
    for (const [c, kind] of dateCols) {
      if (o[c] != null) o[c] = toIso(o[c], kind === "date");
    }
    for (const [c, scale] of decCols) {
      if (o[c] != null && scale > 0) o[c] = Number(o[c]) / 10 ** scale;
    }
    return o;
  });
  return JSON.stringify(rows, (_, v) => (typeof v === "bigint" ? Number(v) : v));
}

// Normalise a DuckDB-Wasm date/time value (Date, or epoch as days/ms/µs) to an
// ISO string — YYYY-MM-DD for dates, "YYYY-MM-DD HH:MM:SS" for timestamps.
function toIso(v, dateOnly) {
  let d;
  if (v instanceof Date) d = v;
  else {
    let n = Number(v);
    if (!isFinite(n)) return String(v);
    if (Math.abs(n) < 1e11) n *= 86400000; // days -> ms
    else if (Math.abs(n) > 1e14) n = Math.round(n / 1000); // µs -> ms
    d = new Date(n);
  }
  if (isNaN(d.getTime())) return String(v);
  const iso = d.toISOString();
  return dateOnly ? iso.slice(0, 10) : iso.replace("T", " ").slice(0, 19);
}

async function boot() {
  await init(); // anofox-visualization wasm (plan + render_panel — used in both modes)
  loadRoleSets();

  // Prefer a live DuckDB bridge (served by `anofox-visualization serve`); else DuckDB-Wasm.
  // A locked served dashboard always uses its server (/api/panel, no SQL).
  if (window.__served) backend = "live";
  else
    try {
      const r = await fetch("/query", { method: "POST", body: "SELECT 1 AS ok" });
      if (r.ok) backend = "live";
    } catch (_) {}

  if (backend !== "live") {
    const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.33.1-dev65.0/+esm");
    const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" })
    );
    db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), new Worker(workerUrl));
    await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
    // Allow unsigned extensions: the anofox wasm extensions are served from
    // web/localext/ (fixed copies of the get.erpl.io builds — see
    // scripts/fetch-wasm-extensions.sh).
    try {
      await db.open({ allowUnsignedExtensions: true });
    } catch (_) {
      /* older duckdb-wasm without the option — extensions just won't load */
    }
    conn = await db.connect();
    // Memory-friendly defaults for the in-browser (in-memory, ~capped) engine:
    // dropping insertion-order preservation avoids OOM on large ops like
    // forecasting every M5 series. Best-effort — ignore if unsupported.
    for (const pragma of ["SET preserve_insertion_order=false", "SET memory_limit='3.6GB'"]) {
      try {
        await conn.query(pragma);
      } catch (_) {
        /* option not available on this build */
      }
    }
  }

  // Sidebar (dashboard list) + app-shell controls
  await loadFileSamples();
  renderSidebar();
  $("side-new").onclick = () => loadDash("", "");
  $("save").onclick = saveDash;
  $("mode-edit").onclick = () => setMode("edit");
  $("mode-view").onclick = () => setMode("view");
  $("side-explore").onclick = () => setMode(bodyMode() === "explore" ? "edit" : "explore");
  $("cat-refresh").onclick = loadCatalog;
  $("cat-search").oninput = () => filterCatalog($("cat-search").value);
  $("xq-run").onclick = runExplore;
  $("xq-sql").addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      runExplore();
    }
  });
  // URL params — for embedding / theming:
  //   ?embed=1        hide the header + sidebar + toolbar (dashboard only)
  //   ?primary=RRGGBB brand accent colour (UI + chart primary)
  //   ?theme=dark|light the viewer's light/dark choice (a dashboard whose
  //                   ::THEME follows the viewer switches; JSON is never read
  //                   from the URL)
  //   ?dashboard=Name load a saved/example dashboard by name
  //   ?mode=edit      open with the SQL editor (default: the dashboard view)
  //   #sql=<base64>   inline SQL (from Share)
  const params = new URLSearchParams(location.search);
  const primary = params.get("primary");
  if (primary && /^#?[0-9a-fA-F]{6}$/.test(primary)) {
    const hex = primary.replace(/^#/, "");
    for (const v of ["--ui-accent", "--ui-accent2", "--ax-accent", "--ax-accent2"])
      document.documentElement.style.setProperty(v, "#" + hex);
    dpPrimary = hex; // chart primary colour (passed to the wasm renderer)
  }
  // Served mode: `anofox-visualization serve --dashboards` injects window.__served
  // = { id, sql } for a fixed, server-owned dashboard. It's view-only (embed) and
  // the server gates /query, so the client can render everything but only run the
  // server's registered queries.
  const served = window.__served || null;
  if (served) servedMode = true;
  if (params.get("embed") === "1" || params.has("embed") || served) {
    document.body.classList.add("embed");
  }
  // Deep-link: activate the tab named in ?tab= on first render (loadDash resets
  // this only when switching dashboards, so it survives the initial run()).
  const tabParam = getTabParam();
  if (tabParam) dpTab = tabParam;
  // Persistent cross-dashboard nav (served mode, 2+ dashboards).
  renderServedNav(served);
  // Opt-in chart zoom (off by default; ?zoom=1 enables).
  if (params.get("zoom") === "1") document.body.classList.add("dp-zoom");
  // Start on the dashboard (View); ✎ Edit — or ?mode=edit — opens the editor.
  setMode(params.get("mode") === "edit" ? "edit" : "view");
  const servedSql = served && typeof served.sql === "string" ? served.sql : null;
  const hashSql = decodeHashSql();
  const wantDash = params.get("dashboard");
  const savedItems = dashStore().items;
  if (servedSql != null) {
    $("sql").value = servedSql;
    $("dash-name").value = served.title || "Dashboard";
  } else if (hashSql) {
    $("sql").value = hashSql;
    $("dash-name").value = "Shared";
  } else if (wantDash && (SAMPLES[wantDash] || savedItems[wantDash])) {
    curDash = wantDash;
    $("sql").value = SAMPLES[wantDash] || savedItems[wantDash].sql;
    $("dash-name").value = wantDash;
    markActive();
  } else {
    const first = Object.keys(SAMPLES)[0];
    curDash = first;
    $("sql").value = SAMPLES[first];
    $("dash-name").value = first;
    markActive();
  }
  $("share").onclick = shareLink;
  $("dlhtml").onclick = downloadHtml;
  // 🌙: the viewer's light/dark choice — the builder chrome follows it, and so
  // does the dashboard when its theme follows the viewer (mode "auto"). The
  // panels re-render through wasm with the dark theme.
  document.body.classList.toggle("dark", viewerPrefersDark());
  $("dark").onclick = () => {
    const dark = !document.body.classList.contains("dark");
    try {
      localStorage.setItem("anofox-viz-dark", dark ? "1" : "0");
    } catch (_) {}
    if (new URLSearchParams(location.search).has("theme")) {
      const u = new URL(location.href);
      u.searchParams.delete("theme");
      history.replaceState(null, "", u);
    }
    document.body.classList.toggle("dark", dark);
    run(false);
  };
  if (typeof matchMedia === "function") {
    matchMedia("(prefers-color-scheme: dark)").addEventListener?.("change", () => {
      let stored = null;
      try {
        stored = localStorage.getItem("anofox-viz-dark");
      } catch (_) {}
      if (stored == null && !new URLSearchParams(location.search).has("theme")) run(false);
    });
  }
  $("refresh").onchange = () => {
    clearInterval(dpTimer);
    const s = parseInt($("refresh").value);
    if (s > 0) dpTimer = setInterval(run, s * 1000);
  };


  // MotherDuck connect dialog + auto-connect from a stored token.
  $("md").onclick = mdOpen;
  $("md-cancel").onclick = () => ($("md-dialog").hidden = true);
  $("md-connect").onclick = mdDoConnect;
  $("md-disconnect").onclick = mdDisconnect;
  const saved = mdSaved();
  if (saved && saved.token) {
    try {
      await mdConnect(saved.token, saved.db);
      mdMark(true, saved.db);
    } catch (e) {
      mdMark(false);
    }
  }

  $("run").disabled = false;
  $("run").onclick = () => run();

  // SQL editor: highlight overlay + scroll sync
  $("sql").addEventListener("input", syncHL);
  $("sql").addEventListener("scroll", () => {
    $("hl").scrollTop = $("sql").scrollTop;
    $("hl").scrollLeft = $("sql").scrollLeft;
  });
  syncHL();
  // Movable editor / dashboard boundary (persisted)
  let ew = parseInt(localStorage.getItem("dp_editor_w") || "400");
  document.documentElement.style.setProperty("--editor-w", ew + "px");
  $("splitter").addEventListener("mousedown", (e) => {
    e.preventDefault();
    $("splitter").classList.add("dragging");
    const startX = e.clientX,
      startW = ew;
    const move = (ev) => {
      ew = Math.max(260, Math.min(920, startW + ev.clientX - startX));
      document.documentElement.style.setProperty("--editor-w", ew + "px");
    };
    const up = () => {
      $("splitter").classList.remove("dragging");
      localStorage.setItem("dp_editor_w", ew);
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  });

  status(backend === "live" ? "live DuckDB · ready" : "DuckDB-Wasm · ready");
  run();
}

// Resizable sidebar (issue #59): the handle on its right edge is a focusable
// separator — drag it, or ←/→ (Shift: bigger steps), Home/End for the bounds,
// double-click (or Enter) resets. The width is per viewer (localStorage) and only applies
// above the phone breakpoint (index.html pins 238px there).
const SIDE_W = { def: 238, min: 180, max: 480, step: 16, key: "dp_side_w" };
function clampSideW(w) {
  const n = Math.round(Number(w));
  return Number.isFinite(n) && n > 0 ? Math.max(SIDE_W.min, Math.min(SIDE_W.max, n)) : SIDE_W.def;
}
// Phones (index.html's max-width:820px shell): the sidebar is an off-canvas
// drawer behind ☰, closed by default. While it is open the rest of the page
// is inert and Tab stays inside it; Esc, the scrim or picking an entry closes
// it. Wider screens: ☰ collapses / restores the sidebar in place.
const SIDE_DRAWER = "(max-width: 820px)";
function initSideDrawer() {
  const side = $("sidebar"),
    btn = $("side-toggle");
  const rest = [document.querySelector("header"), document.querySelector(".main")];
  const isOpen = () => document.body.classList.contains("side-open");
  const open = (on, refocus = true) => {
    document.body.classList.toggle("side-open", on);
    btn.setAttribute("aria-expanded", String(on));
    rest.forEach((el) => el && (el.inert = on));
    if (on) side.querySelector("button")?.focus();
    else if (refocus) btn.focus();
  };
  const drawer = matchMedia(SIDE_DRAWER);
  const sync = () => {
    if (isOpen()) open(false, false);
    btn.setAttribute("aria-expanded", String(drawer.matches ? false : !document.body.classList.contains("side-collapsed")));
  };
  drawer.addEventListener("change", sync);
  sync();
  btn.onclick = () => {
    if (drawer.matches) open(!isOpen());
    else {
      document.body.classList.toggle("side-collapsed");
      sync();
    }
  };
  $("side-scrim").onclick = () => open(false);
  side.addEventListener("click", (e) => {
    if (isOpen() && e.target.closest(".side-item, #side-new, #side-explore") && !e.target.closest(".s-del")) open(false, false);
  });
  document.addEventListener(
    "keydown",
    (e) => {
      if (!isOpen()) return;
      if (e.key === "Escape") {
        e.preventDefault();
        open(false);
      } else if (e.key === "Tab") {
        const f = [...side.querySelectorAll("button, a[href], [tabindex='0']")].filter((el) => el.offsetParent);
        if (!f.length) return;
        const [first, last] = [f[0], f[f.length - 1]];
        if (!side.contains(document.activeElement) || (e.shiftKey ? document.activeElement === first : document.activeElement === last)) {
          e.preventDefault();
          (e.shiftKey ? last : first).focus();
        }
      }
    },
    true
  );
}

function initSideResizer() {
  const h = $("side-resizer");
  if (!h) return;
  let w = SIDE_W.def;
  try {
    w = clampSideW(localStorage.getItem(SIDE_W.key) || SIDE_W.def);
  } catch (_) {
    /* storage blocked: default width */
  }
  const apply = (nw, save) => {
    w = clampSideW(nw);
    document.documentElement.style.setProperty("--side-w", w + "px");
    h.setAttribute("aria-valuenow", w);
    h.setAttribute("aria-valuetext", `${w} pixels`);
    if (!save) return;
    try {
      localStorage.setItem(SIDE_W.key, w);
    } catch (_) {
      /* storage blocked: width lasts for this page only */
    }
    // Charts scale with their panel; let anything sized to the window re-measure.
    window.dispatchEvent(new Event("resize"));
  };
  apply(w, false);
  h.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    h.setPointerCapture(e.pointerId);
    h.classList.add("dragging");
    const startX = e.clientX,
      startW = w;
    const move = (ev) => apply(startW + ev.clientX - startX, false);
    const up = () => {
      h.classList.remove("dragging");
      h.removeEventListener("pointermove", move);
      h.removeEventListener("pointerup", up);
      h.removeEventListener("pointercancel", up);
      apply(w, true);
    };
    h.addEventListener("pointermove", move);
    h.addEventListener("pointerup", up);
    h.addEventListener("pointercancel", up);
  });
  h.addEventListener("dblclick", () => apply(SIDE_W.def, true));
  h.addEventListener("keydown", (e) => {
    const big = e.shiftKey ? 4 : 1;
    const to = {
      ArrowLeft: w - SIDE_W.step * big,
      ArrowRight: w + SIDE_W.step * big,
      Home: SIDE_W.min,
      End: SIDE_W.max,
      Enter: SIDE_W.def,
    }[e.key];
    if (to === undefined) return;
    e.preventDefault();
    apply(to, true);
  });
}

// ---------- app shell: sidebar, dashboards, modes, data exploration ----------
const DASH_KEY = "dp_dashboards";
let curDash = null;
// Store: { items: { name: {sql, group} }, groups: [names], collapsed: {group:bool} }.
function dashStore() {
  let raw;
  try {
    raw = JSON.parse(localStorage.getItem(DASH_KEY) || "null");
  } catch (_) {
    raw = null;
  }
  if (!raw) return { items: {}, groups: [], collapsed: {} };
  if (!raw.items) {
    // migrate the old flat { name: sql } shape
    const items = {};
    for (const [n, sql] of Object.entries(raw)) if (typeof sql === "string") items[n] = { sql, group: "" };
    return { items, groups: [], collapsed: {} };
  }
  raw.items ||= {};
  raw.groups ||= [];
  raw.collapsed ||= {};
  return raw;
}
const dashSaveStore = (s) => localStorage.setItem(DASH_KEY, JSON.stringify(s));

function renderSidebar() {
  const nav = $("side-nav");
  nav.innerHTML = "";
  const store = dashStore();
  const names = Object.keys(store.items);

  // "My dashboards" header with a + group button (drop here to ungroup)
  const hdr = document.createElement("div");
  hdr.className = "side-section side-section-row";
  const lbl = document.createElement("span");
  lbl.textContent = "My dashboards";
  const add = document.createElement("button");
  add.className = "side-mini";
  add.textContent = "+ group";
  add.title = "new group";
  add.onclick = createGroup;
  hdr.append(lbl, add);
  hdr.ondragover = (e) => {
    e.preventDefault();
    hdr.classList.add("drop");
  };
  hdr.ondragleave = () => hdr.classList.remove("drop");
  hdr.ondrop = (e) => {
    e.preventDefault();
    hdr.classList.remove("drop");
    moveDash(e.dataTransfer.getData("text/plain"), "");
  };
  nav.appendChild(hdr);

  // bucket dashboards by group
  const byGroup = {};
  for (const g of store.groups) byGroup[g] = [];
  const ungrouped = [];
  for (const n of names) {
    const g = store.items[n].group;
    if (g && byGroup[g]) byGroup[g].push(n);
    else ungrouped.push(n);
  }
  for (const g of store.groups) {
    nav.appendChild(groupHeader(g, store));
    if (!store.collapsed[g])
      for (const n of byGroup[g]) {
        const it = sideItem(n, store.items[n].sql, true);
        it.classList.add("in-group");
        nav.appendChild(it);
      }
  }
  for (const n of ungrouped) nav.appendChild(sideItem(n, store.items[n].sql, true));

  // Examples — visually separated from your dashboards, collectively collapsible.
  const exc = exCollapsed();
  const master = document.createElement("div");
  master.className = "side-section side-section-toggle side-master";
  const mc = document.createElement("span");
  mc.className = "ex-caret";
  mc.textContent = exc.__all__ ? "▸" : "▾";
  const ml = document.createElement("span");
  ml.textContent = "Examples";
  master.append(mc, ml);
  master.onclick = () => toggleEx("__all__");
  nav.appendChild(master);
  // Group headers are static labels — only the two masters collapse.
  const groupItems = (g) => {
    const hdr = document.createElement("div");
    hdr.className = "side-section side-sub";
    hdr.textContent = g.group;
    nav.appendChild(hdr);
    for (const [n, sql] of Object.entries(g.items)) {
      const it = sideItem(n, sql, false);
      it.classList.add("side-ex-item"); // indent examples under their group header
      nav.appendChild(it);
    }
  };
  if (!exc.__all__) for (const g of SAMPLE_GROUPS) if (!g.ext) groupItems(g);
  // The anofox extension showcases — a separate area below the core tour.
  const extGroups = SAMPLE_GROUPS.filter((g) => g.ext);
  if (extGroups.length) {
    const xm = document.createElement("div");
    xm.className = "side-section side-section-toggle side-master";
    const xc = document.createElement("span");
    xc.className = "ex-caret";
    xc.textContent = exc.__ext__ ? "▸" : "▾";
    const xl = document.createElement("span");
    xl.textContent = "anofox extensions";
    xm.title = "Showcases of the anofox DuckDB extensions (live in the browser, or stored results computed natively)";
    xm.append(xc, xl);
    xm.onclick = () => toggleEx("__ext__");
    nav.appendChild(xm);
    if (!exc.__ext__) for (const g of extGroups) groupItems(g);
  }
  markActive();
}
const EXC_KEY = "dp_ex_collapsed";
function exCollapsed() {
  try {
    return JSON.parse(localStorage.getItem(EXC_KEY) || "{}");
  } catch (_) {
    return {};
  }
}
function toggleEx(group) {
  const c = exCollapsed();
  c[group] = !c[group];
  localStorage.setItem(EXC_KEY, JSON.stringify(c));
  renderSidebar();
}
function sideSection(t) {
  const d = document.createElement("div");
  d.className = "side-section";
  d.textContent = t;
  return d;
}
function groupHeader(g, store) {
  const h = document.createElement("div");
  h.className = "side-group" + (store.collapsed[g] ? " collapsed" : "");
  const caret = document.createElement("span");
  caret.className = "g-caret";
  caret.textContent = store.collapsed[g] ? "▸" : "▾";
  const nm = document.createElement("span");
  nm.className = "g-name";
  nm.textContent = g;
  const ren = document.createElement("button");
  ren.className = "g-btn";
  ren.textContent = "✎";
  ren.title = "rename group";
  const del = document.createElement("button");
  del.className = "g-btn";
  del.textContent = "✕";
  del.title = "delete group";
  h.append(caret, nm, ren, del);
  h.onclick = (e) => {
    if (e.target === ren || e.target === del) return;
    const s = dashStore();
    s.collapsed[g] = !s.collapsed[g];
    dashSaveStore(s);
    renderSidebar();
  };
  ren.onclick = (e) => {
    e.stopPropagation();
    renameGroup(g);
  };
  del.onclick = (e) => {
    e.stopPropagation();
    deleteGroup(g);
  };
  h.ondragover = (e) => {
    e.preventDefault();
    h.classList.add("drop");
  };
  h.ondragleave = () => h.classList.remove("drop");
  h.ondrop = (e) => {
    e.preventDefault();
    h.classList.remove("drop");
    moveDash(e.dataTransfer.getData("text/plain"), g);
  };
  return h;
}
function sideItem(name, sql, deletable) {
  const b = document.createElement("button");
  b.className = "side-item";
  b.dataset.name = name;
  if (deletable) {
    b.draggable = true;
    b.ondragstart = (e) => {
      e.dataTransfer.setData("text/plain", name);
      e.dataTransfer.effectAllowed = "move";
    };
  }
  const s = document.createElement("span");
  s.className = "s-name";
  s.textContent = name;
  b.title = name; // long names are ellipsised in the sidebar
  b.appendChild(s);
  if (deletable) {
    const del = document.createElement("span");
    del.className = "s-del";
    del.textContent = "✕";
    del.title = "delete";
    del.onclick = (e) => {
      e.stopPropagation();
      delDash(name);
    };
    b.appendChild(del);
  }
  b.onclick = () => loadDash(name, sql);
  return b;
}
function markActive() {
  document.querySelectorAll(".side-item").forEach((el) => el.classList.toggle("active", el.dataset.name === curDash));
}
function createGroup() {
  const g = (prompt("New group name") || "").trim();
  if (!g) return;
  const s = dashStore();
  if (!s.groups.includes(g)) s.groups.push(g);
  dashSaveStore(s);
  renderSidebar();
}
function renameGroup(old) {
  const g = (prompt("Rename group", old) || "").trim();
  if (!g || g === old) return;
  const s = dashStore();
  s.groups = s.groups.map((x) => (x === old ? g : x));
  for (const n in s.items) if (s.items[n].group === old) s.items[n].group = g;
  if (s.collapsed[old] !== undefined) {
    s.collapsed[g] = s.collapsed[old];
    delete s.collapsed[old];
  }
  dashSaveStore(s);
  renderSidebar();
}
function deleteGroup(g) {
  const s = dashStore();
  s.groups = s.groups.filter((x) => x !== g);
  for (const n in s.items) if (s.items[n].group === g) s.items[n].group = "";
  delete s.collapsed[g];
  dashSaveStore(s);
  renderSidebar();
}
function moveDash(name, group) {
  const s = dashStore();
  if (s.items[name]) {
    s.items[name].group = group;
    dashSaveStore(s);
    renderSidebar();
  }
}
function loadDash(name, sql) {
  curDash = name || null;
  // A different dashboard starts clean — don't carry tab/filter/page state over.
  dpTab = null;
  dpTableUI = {};
  dpSubTab = {};
  dpFilter = "";
  dpSelected = null;
  dpXf = {};
  dpUrlSeed = {};
  dpVarDefault = {};
  dpPeriodUrl = {};
  dpRange = {};
  dpDrill = {};
  dpPage = {};
  dpSort = {};
  dpPaged = {};
  dpFilterText = {};
  $("sql").value = sql;
  $("dash-name").value = name || "";
  syncHL();
  if (bodyMode() === "explore") setMode("edit");
  markActive();
  run();
}
function saveDash() {
  const name = ($("dash-name").value || "").trim();
  if (!name) {
    $("dash-name").focus();
    return status("name it first");
  }
  const s = dashStore();
  const group = s.items[name] ? s.items[name].group : "";
  s.items[name] = { sql: $("sql").value, group };
  dashSaveStore(s);
  curDash = name;
  renderSidebar();
  status("saved ✓");
}
function delDash(name) {
  const s = dashStore();
  delete s.items[name];
  dashSaveStore(s);
  if (curDash === name) curDash = null;
  renderSidebar();
}

// ---- Edit / View / Explore modes ----
function bodyMode() {
  const c = document.body.classList;
  return c.contains("mode-explore") ? "explore" : c.contains("mode-view") ? "view" : "edit";
}
function setMode(m) {
  document.body.classList.remove("mode-view", "mode-explore");
  if (m === "view") document.body.classList.add("mode-view");
  else if (m === "explore") document.body.classList.add("mode-explore");
  $("mode-edit").classList.toggle("active", m === "edit");
  $("mode-view").classList.toggle("active", m === "view");
  $("side-explore").classList.toggle("active", m === "explore");
  document.querySelectorAll(".dash .dp-title[data-title]").forEach(paintHeadTitle);
  if (m === "explore") loadCatalog();
}

// ---- Data exploration: catalog browser + table preview + column stats ----
const qid = (s) => `"${String(s).replace(/"/g, '""')}"`;
const fqn = (db, sc, t) => `${qid(db)}.${qid(sc)}.${qid(t)}`;
async function loadCatalog() {
  const tree = $("cat-tree");
  tree.innerHTML = '<div class="cat-empty">Loading…</div>';
  let rows;
  try {
    rows = JSON.parse(
      await runSql(
        "SELECT database_name, schema_name, table_name FROM duckdb_tables() " +
          "UNION ALL SELECT database_name, schema_name, view_name FROM duckdb_views() " +
          "WHERE NOT internal ORDER BY 1,2,3"
      )
    );
  } catch (_) {
    try {
      rows = JSON.parse(
        await runSql(
          "SELECT table_catalog AS database_name, table_schema AS schema_name, table_name FROM information_schema.tables ORDER BY 1,2,3"
        )
      );
    } catch (e) {
      tree.innerHTML = `<div class="cat-empty">${escapeHtml(String(e))}</div>`;
      return;
    }
  }
  if (!rows.length) {
    tree.innerHTML = '<div class="cat-empty">No tables yet. Create one in Edit mode, or connect MotherDuck.</div>';
    return;
  }
  const groups = {};
  for (const r of rows) {
    const db = r.database_name,
      sc = r.schema_name,
      t = r.table_name;
    (groups[db] ??= {})[sc] ??= [];
    groups[db][sc].push(t);
  }
  tree.innerHTML = "";
  for (const db of Object.keys(groups)) {
    const dn = document.createElement("div");
    dn.className = "cat-node cat-db";
    dn.textContent = "🗄 " + db;
    tree.appendChild(dn);
    for (const sc of Object.keys(groups[db])) {
      const sn = document.createElement("div");
      sn.className = "cat-node cat-schema";
      sn.textContent = sc;
      tree.appendChild(sn);
      for (const t of groups[db][sc]) {
        const tn = document.createElement("div");
        tn.className = "cat-node cat-table";
        tn.textContent = t;
        tn.dataset.fq = fqn(db, sc, t);
        tn.onclick = () => previewTable(db, sc, t, tn);
        tree.appendChild(tn);
      }
    }
  }
}
function filterCatalog(q) {
  q = q.trim().toLowerCase();
  const tree = $("cat-tree");
  tree.querySelectorAll(".cat-table").forEach((tn) => {
    tn.style.display = !q || tn.textContent.toLowerCase().includes(q) ? "" : "none";
  });
  tree.querySelectorAll(".cat-schema").forEach((sn) => {
    let any = false;
    for (let el = sn.nextElementSibling; el && el.classList.contains("cat-table"); el = el.nextElementSibling)
      if (el.style.display !== "none") any = true;
    sn.style.display = any ? "" : "none";
  });
  tree.querySelectorAll(".cat-db").forEach((dn) => {
    let any = false;
    for (let el = dn.nextElementSibling; el && !el.classList.contains("cat-db"); el = el.nextElementSibling)
      if (el.classList.contains("cat-table") && el.style.display !== "none") any = true;
    dn.style.display = any ? "" : "none";
  });
}
// Run whatever is in the explore SQL editor and show the result table.
async function runExplore() {
  $("xq-detail").innerHTML = ""; // a manual query clears the table-stats card
  await renderExploreResult($("xq-sql").value.trim());
}
async function renderExploreResult(sql) {
  const info = $("xq-info"),
    res = $("xq-results");
  if (!sql) {
    res.innerHTML = '<div class="explore-empty">Pick a table on the left, or write a query above and Run.</div>';
    info.textContent = "";
    return;
  }
  info.textContent = "Running…";
  res.innerHTML = "";
  try {
    const rows = JSON.parse(await runSql(sql));
    info.textContent = `${rows.length.toLocaleString()} row${rows.length === 1 ? "" : "s"}`;
    res.appendChild(renderTable(rows));
  } catch (e) {
    info.textContent = "";
    res.innerHTML = `<div class="err">${escapeHtml(String(e))}</div>`;
  }
}
async function previewTable(db, sc, t, node) {
  document.querySelectorAll(".cat-table").forEach((el) => el.classList.toggle("active", el === node));
  const fq = fqn(db, sc, t);
  const path = `${escapeHtml(db)}.${escapeHtml(sc)}`;
  $("xq-sql").value = `SELECT * FROM ${fq} LIMIT 100`;
  const detail = $("xq-detail");
  detail.innerHTML = '<div class="explore-empty">Loading…</div>';
  renderExploreResult(`SELECT * FROM ${fq} LIMIT 100`);
  try {
    const stats = JSON.parse(await runSql(`SUMMARIZE FROM ${fq}`));
    let total = null;
    try {
      total = Number(JSON.parse(await runSql(`SELECT count(*) AS n FROM ${fq}`))[0].n);
    } catch (_) {}
    detail.innerHTML = "";
    const h = document.createElement("div");
    h.className = "explore-head";
    const meta = `${total != null ? total.toLocaleString() + " rows" : ""} · ${stats.length} columns`;
    h.innerHTML = `<div><h3>${escapeHtml(t)}</h3><div class="explore-sub">${path} — ${meta}</div></div>`;
    const open = document.createElement("button");
    open.className = "btn2";
    open.textContent = "＋ New dashboard from this table";
    open.onclick = () =>
      openTableAsDashboard(
        t,
        fq,
        stats.map((r) => r.column_name)
      );
    h.appendChild(open);
    detail.appendChild(h);
    const sec = document.createElement("div");
    sec.className = "explore-sec";
    sec.textContent = `Columns (${stats.length})`;
    detail.appendChild(sec);
    detail.appendChild(renderTable(stats));
  } catch (e) {
    detail.innerHTML = `<div class="err">${escapeHtml(String(e))}</div>`;
  }
}
// Explore → build: prewrite a paged dashboard querying the picked table.
function openTableAsDashboard(name, fq, colNames) {
  const cols = (colNames || []).filter(Boolean);
  let sql;
  if (cols.length) {
    const items = cols.map((c, i) => (i === cols.length - 1 ? `${qid(c)} ::PAGED` : qid(c))).join(", ");
    sql = `-- Paged view of ${fq}\nSELECT ${items} FROM ${fq};`;
  } else {
    sql = `SELECT * FROM ${fq} LIMIT 100 ::TABLE;`;
  }
  setMode("edit");
  loadDash(name, sql);
  $("dash-name").value = name;
}

// ---------- MotherDuck ----------
const MD_KEY = "dp_md";
function mdSaved() {
  try {
    return JSON.parse(localStorage.getItem(MD_KEY) || "null");
  } catch (_) {
    return null;
  }
}
// Attach an md: database. Token/db are substituted here (never in the dashboard
// SQL), so they stay out of the share link and exported HTML.
async function mdConnect(token, db) {
  const q = (s) => `'${String(s).replace(/'/g, "''")}'`;
  for (const stmt of ["INSTALL motherduck", "LOAD motherduck"]) {
    try {
      await runSql(stmt);
    } catch (_) {
      /* autoloaded on some builds */
    }
  }
  await runSql(`SET motherduck_token=${q(token)}`);
  try {
    await runSql(db ? `ATTACH ${q("md:" + db)}` : "ATTACH 'md:'");
  } catch (e) {
    if (!/already attached|already exists/i.test(String(e))) throw e;
  }
}
function mdMark(ok, db) {
  const btn = $("md");
  btn.classList.toggle("connected", ok);
  btn.textContent = ok ? `☁ ${db || "MotherDuck"} ✓` : "☁ MotherDuck";
}
function mdOpen() {
  const s = mdSaved() || {};
  $("md-token").value = s.token || "";
  $("md-db").value = s.db || "";
  const st = $("md-status");
  st.textContent = "";
  st.className = "md-status";
  $("md-dialog").hidden = false;
  $("md-token").focus();
}
async function mdDoConnect() {
  const token = $("md-token").value.trim();
  const db = $("md-db").value.trim();
  const st = $("md-status");
  if (!token) {
    st.textContent = "Please paste a token.";
    st.className = "md-status err";
    return;
  }
  st.textContent = "Connecting…";
  st.className = "md-status";
  try {
    await mdConnect(token, db);
    localStorage.setItem(MD_KEY, JSON.stringify({ token, db }));
    mdMark(true, db);
    st.textContent = "Connected ✓";
    st.className = "md-status ok";
    setTimeout(() => ($("md-dialog").hidden = true), 700);
    run();
  } catch (e) {
    st.textContent = "Failed: " + String(e).replace(/^Error:\s*/, "");
    st.className = "md-status err";
  }
}
async function mdDisconnect() {
  const s = mdSaved();
  localStorage.removeItem(MD_KEY);
  if (s && s.db) {
    try {
      await runSql(`DETACH ${s.db.replace(/[^A-Za-z0-9_]/g, "")}`); // detach by name
    } catch (_) {}
  }
  mdMark(false);
  const st = $("md-status");
  st.textContent = "Disconnected — reload the page for a fully clean session.";
  st.className = "md-status";
}

const role = (s, name) => s.roles.some((r) => r[1] === name);
// Role sets come from the Rust role registry (wasm `roles_json()`), so the
// browser never drifts from the planner. The literals are only a fallback for
// an older wasm build without that export.
let INPUTS = ["DROPDOWN", "NUMBER", "DATE", "TEXT", "MULTISELECT", "DATERANGE", "PERIOD", "BUTTONS", "TOGGLE"];
let METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];
let DIRECTIVES = ["COLUMNS", "GROUP", "ENDGROUP", "SPAN", "HEIGHT", "ROWSELECT", "EMPTY", "TOTALS", "COLGROUP", "FLAG", "HIGHLIGHT", "SUBTOTAL", "TAB", "SUBTAB", "PLACEHOLDER"];
let TFMT = ["MONEY", "PERCENT", "COMPACT", "METRIC", "TREND", "COLORSCALE", "COLORSCALE_HIGH_GOOD", "COLORSCALE_DIVERGING", "BADGE", "SPARKLINE", "PLAIN"];
let TEXT_SIZES = ["TEXT_SMALL", "TEXT_MEDIUM", "TEXT_LARGE"];
// Chart kinds whose panel honours a zoom window (plain cartesian charts).
let ZOOMABLE = [
  "BARCHART",
  "BARCHART_STACKED",
  "BARCHART_PERCENT",
  "BARCHART_STACKED_PERCENT",
  "LINECHART",
  "LINECHART_PERCENT",
  "STEP",
  "SMOOTH",
  "AREACHART",
  "AREACHART_STACKED",
  "SCATTER",
  "BUBBLE",
  "JITTER",
  "BOXPLOT",
  "VIOLIN",
];
function loadRoleSets() {
  try {
    const sets = JSON.parse(roles_json()).sets;
    INPUTS = sets.inputs;
    METRICS = sets.metrics;
    DIRECTIVES = sets.directives;
    TFMT = sets.table_formats;
    TEXT_SIZES = sets.text_sizes;
    if (sets.zoomable) ZOOMABLE = sets.zoomable;
    if (sets.chrome) CHROME = sets.chrome;
  } catch (_) {}
}
const inputKind = (s) => INPUTS.find((k) => role(s, k));
const isInput = (s) => !!inputKind(s);
// A ::DIAGRAM's KPI format is its nodes' value line, not a KPI tile.
const metricRole = (s) => !role(s, "DIAGRAM") && s.roles.find((r) => METRICS.includes(r[1]));
const isHeading = (s) => s.roles.length === 1 && s.roles[0][1] === "LABEL";
// A lone ::THRESHOLDS statement is the next table's rules (beside other roles
// it colours that KPI / gauge / chart).
const directive = (s) =>
  DIRECTIVES.find((d) => role(s, d)) || (s.roles.length === 1 && s.roles[0][1] === "THRESHOLDS" ? "THRESHOLDS" : undefined);
// Dashboard chrome directives (::THEME, ::LOCALE, ::DASHBOARD_TITLE, ::BRAND,
// ::LOGO, ::FAVICON) — collected in the pre-pass, before any panel renders.
let CHROME = ["THEME", "LOCALE", "DASHBOARD_TITLE", "BRAND", "LOGO", "FAVICON", "ASOF"];
const chromeRole = (s) => CHROME.find((d) => role(s, d));

// ---- theme (src/theme.rs is the source of truth; plan T1/T4/T12) ----
// dpTheme.spec: the dashboard's ::THEME (or ""), dpTheme.chrome: the other
// chrome directives, dpTheme.set: the resolved theme JSON from wasm, and
// dpTheme.variant: the variant on screen ("light" | "dark").
let dpTheme = { spec: "", chrome: {}, set: null, variant: "light", key: "" };
// The viewer's light/dark preference: ?theme=dark|light, else the 🌙 toggle
// (remembered in this browser), else the OS setting.
function viewerPrefersDark() {
  const q = new URLSearchParams(location.search).get("theme");
  if (q === "dark" || q === "light") return q === "dark";
  try {
    const v = localStorage.getItem("anofox-viz-dark");
    if (v === "1" || v === "0") return v === "1";
  } catch (_) {}
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
}
// Resolve the dashboard theme (cached by spec+chrome) and apply its tokens to
// the page. Returns an error message, or "".
function applyDashboardTheme() {
  const key = dpTheme.spec + "\u0000" + JSON.stringify(dpTheme.chrome);
  let err = "";
  if (key !== dpTheme.key || !dpTheme.set) {
    let set = null;
    try {
      set = JSON.parse(resolve_dashboard_theme(dpTheme.spec, JSON.stringify(dpTheme.chrome)));
    } catch (e) {
      set = { ok: false, error: String(e) };
    }
    if (!set.ok) {
      err = `::THEME / chrome directive: ${set.error}`;
      set = JSON.parse(resolve_dashboard_theme("", "{}"));
    }
    dpTheme.set = set;
    dpTheme.key = key;
  }
  const set = dpTheme.set;
  const dark = viewerPrefersDark();
  dpTheme.variant = set.mode === "auto" ? (dark ? "dark" : "light") : set.mode;
  document.body.classList.toggle("dark", dark);
  const v = set[dpTheme.variant];
  const el = document.getElementById("ax-theme");
  if (el && el.textContent !== v.css) el.textContent = v.css;
  document.documentElement.dataset.axVariant = dpTheme.variant;
  // Author branding: <title>, favicon (the builder keeps its product header).
  const b = v.brand || {};
  if (b.title) document.title = b.title + (b.title_suffix || "");
  let icon = document.querySelector("link[rel=icon]");
  if (b.favicon) {
    const href = safeUrl(b.favicon, { image: true });
    if (href) {
      if (!icon) {
        icon = document.createElement("link");
        icon.rel = "icon";
        document.head.appendChild(icon);
      }
      icon.href = href;
    }
  } else if (icon) icon.remove();
  return err;
}
function defaultThemeSpec() {
  const sv = window.__served;
  const t = (sv && sv.theme) || window.__anofoxTheme || "";
  return typeof t === "string" ? t : JSON.stringify(t);
}
// The wasm render context: "" / "rrggbb" for the default look (byte-identical
// to earlier releases), else {"primary","theme","variant","locale"}.
function renderCtx() {
  const set = dpTheme.set;
  const loc = dashLocale();
  const plain = !dpTheme.spec && dpTheme.variant === "light" && !loc;
  if (plain) return dpPrimary || "";
  const ctx = { theme: dpTheme.spec || "", variant: dpTheme.variant };
  if (loc) ctx.locale = loc;
  if (dpPrimary) ctx.primary = dpPrimary;
  return JSON.stringify(ctx);
}
// The dashboard locale (::LOCALE or the theme's locale token), "" = en-US.
function dashLocale() {
  // ?locale=de-DE: a viewer override (not in served/locked mode).
  const q = !servedMode && new URLSearchParams(location.search).get("locale");
  if (q && /^[A-Za-z]{2}([-_][A-Za-z]{2})?$/.test(q)) return q;
  const set = dpTheme.set;
  const v = set && set[dpTheme.variant];
  return (v && v.locale) || "";
}
// A theme token's current value (e.g. "scale-lo"), read from the page.
function axToken(name, fallback = "") {
  const v = getComputedStyle(document.documentElement).getPropertyValue("--ax-" + name).trim();
  return v || fallback;
}
// The dashboard header (::DASHBOARD_TITLE / ::BRAND / ::LOGO), or null.
// ---- ::ASOF freshness chip (src/freshness.rs has the same rules) ----
let dpAsof = null; // {at: epoch ms | null, stale: seconds | 0}
// A timestamp cell → epoch ms: a number is epoch ms (DuckDB-Wasm), text is
// ISO-ish ('2026-10-16 14:02:00[+02]'); text without an offset is UTC.
function asofParse(v) {
  if (v == null || v === "") return null;
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  const m = String(v)
    .trim()
    .match(/^(\d{4}-\d{2}-\d{2})(?:[ T](\d{2}:\d{2})(:\d{2}(?:\.\d+)?)?)?\s*(Z|UTC|[+-]\d{2}(?::?\d{2})?)?$/);
  if (!m) return null;
  let off = m[4] || "Z";
  if (off === "UTC") off = "Z";
  else if (/^[+-]\d{2}$/.test(off)) off += ":00";
  else if (/^[+-]\d{4}$/.test(off)) off = off.slice(0, 3) + ":" + off.slice(3);
  const t = Date.parse(`${m[1]}T${m[2] || "00:00"}${m[3] || ""}${off}`);
  return Number.isNaN(t) ? null : t;
}
// 'stale=24h' / '90m' / '2d' / '1w' → seconds (0 = never stale).
function asofStale(v) {
  const m = String(v ?? "").match(/^\s*(?:stale\s*=?\s*)?(\d+)\s*([smhdw])\s*$/i);
  return m ? Number(m[1]) * { s: 1, m: 60, h: 3600, d: 86400, w: 604800 }[m[2].toLowerCase()] : 0;
}
function asofRelative(sec) {
  sec = Math.max(0, sec);
  if (sec < 60) return "just now";
  if (sec < 3600) return `${Math.floor(sec / 60)} min ago`;
  if (sec < 172800) return `${Math.floor(sec / 3600)} h ago`;
  return `${Math.floor(sec / 86400)} d ago`;
}
// Fill a chip: "Data as of 14:02 (2 h ago)" in the viewer's zone and the
// dashboard locale (a date is added when it isn't today); amber once stale.
function paintAsof(el, a) {
  const d = new Date(a.at);
  const now = Date.now();
  const age = (now - a.at) / 1000;
  const loc = dashLocale() || undefined;
  let t;
  try {
    t = d.toLocaleTimeString(loc, { hour: "2-digit", minute: "2-digit" });
    if (d.toDateString() !== new Date(now).toDateString()) t = d.toLocaleDateString(loc) + " " + t;
  } catch (_) {
    t = d.toISOString().slice(0, 16).replace("T", " ");
  }
  el.textContent = `Data as of ${t} (${asofRelative(age)})`;
  el.title = d.toString();
  el.classList.toggle("stale", a.stale > 0 && age > a.stale);
}
let asofTimer = null;
function mkAsof() {
  if (!dpAsof || dpAsof.at == null) return null;
  const c = document.createElement("div");
  c.className = "dp-asof";
  c.setAttribute("role", "status");
  paintAsof(c, dpAsof);
  // keep the relative time current while the dashboard is open
  asofTimer ||= setInterval(() => {
    document.querySelectorAll(".dash .dp-asof").forEach((el) => dpAsof && dpAsof.at != null && paintAsof(el, dpAsof));
  }, 60000);
  return c;
}
function dashHeader() {
  const v = dpTheme.set && dpTheme.set[dpTheme.variant];
  const b = (v && v.brand) || {};
  const chip = mkAsof();
  // The builder's Edit mode edits the name and logo in place (#60); a served or
  // embedded dashboard (and View mode) shows them as they are.
  const editable = !servedMode && !document.body.classList.contains("embed");
  if (!b.title && !b.name && !b.logo && !editable) {
    if (!chip) return null;
    const h = document.createElement("div");
    h.className = "dp-head";
    h.appendChild(chip);
    return h;
  }
  const h = document.createElement("div"); // not <header>: the builder styles that
  h.className = "dp-head";
  const logo = b.logo && safeUrl(b.logo, { image: true });
  if (logo) {
    const img = document.createElement("img");
    img.className = "dp-logo";
    img.alt = "";
    img.src = logo;
    h.appendChild(img);
  }
  const txt = document.createElement("div");
  txt.className = "dp-head-text";
  if (b.name) {
    const n = document.createElement("div");
    n.className = "dp-brand";
    n.textContent = b.name;
    txt.appendChild(n);
  }
  if (editable) {
    const t = document.createElement("h1");
    t.className = "dp-title";
    t.dataset.title = b.title || "";
    paintHeadTitle(t);
    txt.appendChild(t);
  } else if (b.title) {
    const t = document.createElement("h1");
    t.className = "dp-title";
    t.textContent = b.title;
    txt.appendChild(t);
  }
  h.appendChild(txt);
  if (editable) {
    h.appendChild(mkLogoEdit(h, b.logo || ""));
    if (!b.title && !b.name && !logo && !chip) h.classList.add("dp-head-blank");
  }
  if (chip) h.appendChild(chip);
  return h;
}
// ---- editing the header in the builder (#60): plain SQL, so Share links, the
// HTML export and `dashboard` / `serve` keep the change ----
const LOGO_MAX_BYTES = 256 * 1024; // an uploaded logo is embedded as a data: URL
const LOGO_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml"];
// Write `value` as the dashboard's ::<roleName> statement, then re-run; `focusKey`
// names the header control to focus again once the new header is on screen.
function editChrome(roleName, value, focusKey) {
  const ta = $("sql");
  const next = setChromeSql(ta.value, roleName, value);
  if (next !== ta.value) {
    ta.value = next;
    ta.dispatchEvent(new Event("input"));
  }
  dpRefocus = focusKey;
  run();
}
// The header name: plain text in View mode; in Edit mode a button that turns
// into a text field (Enter / blur saves, Esc cancels). Called again on a mode
// switch (setMode).
function paintHeadTitle(h1) {
  const t = h1.dataset.title;
  if (bodyMode() !== "edit") {
    h1.textContent = t;
    h1.hidden = !t;
    return;
  }
  h1.hidden = false;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "dp-title-btn" + (t ? "" : " dp-title-add");
  btn.textContent = t || "Add a dashboard name";
  btn.title = "Edit the dashboard name (::DASHBOARD_TITLE)";
  if (t) btn.setAttribute("aria-label", `Dashboard name: ${t}. Edit`);
  btn.onclick = () => editHeadTitle(h1);
  h1.replaceChildren(btn);
  refocus("head:title", btn);
}
function editHeadTitle(h1) {
  const cur = h1.dataset.title;
  const inp = document.createElement("input");
  inp.className = "dp-title-input";
  inp.value = cur;
  inp.placeholder = "Dashboard name";
  inp.setAttribute("aria-label", "Dashboard name");
  inp.spellcheck = false;
  h1.replaceChildren(inp);
  inp.focus();
  inp.select();
  let done = false;
  const finish = (save) => {
    if (done) return;
    done = true;
    const v = inp.value.trim();
    if (save && v !== cur) return editChrome("DASHBOARD_TITLE", v, "head:title");
    paintHeadTitle(h1);
    if (!save) h1.firstChild.focus();
  };
  inp.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      finish(true);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      finish(false);
    }
  });
  inp.addEventListener("blur", () => finish(true));
}
// "Add logo" / "Change logo" and its form: an image URL or an uploaded file
// (embedded as a data:image/ URL up to LOGO_MAX_BYTES), checked like ::LOGO.
function mkLogoEdit(head, cur) {
  const wrap = document.createElement("div");
  wrap.className = "dp-hedit dp-logo-edit";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "dp-hbtn";
  btn.textContent = cur ? "Change logo" : "Add logo";
  btn.setAttribute("aria-expanded", "false");
  const form = document.createElement("form");
  form.className = "dp-logo-form";
  form.id = "dp-logo-form";
  form.hidden = true;
  form.setAttribute("aria-label", "Dashboard logo");
  btn.setAttribute("aria-controls", form.id);
  const field = (label, input) => {
    const l = document.createElement("label");
    l.append(label, input);
    return l;
  };
  const url = document.createElement("input");
  url.type = "text";
  url.spellcheck = false;
  url.placeholder = "https://… or logo.svg";
  url.value = cur.startsWith("data:") ? "" : cur;
  const file = document.createElement("input");
  file.type = "file";
  file.accept = LOGO_TYPES.join(",");
  const err = document.createElement("div");
  err.className = "dp-logo-err";
  err.setAttribute("role", "alert");
  const acts = document.createElement("div");
  acts.className = "dp-logo-acts";
  const mk = (text, type, cls) => {
    const b = document.createElement("button");
    b.type = type;
    b.className = cls;
    b.textContent = text;
    acts.appendChild(b);
    return b;
  };
  mk("Apply", "submit", "dp-hbtn dp-hbtn-main");
  if (cur) mk("Remove logo", "button", "dp-hbtn").onclick = () => editChrome("LOGO", "", "head:logo");
  const cancel = mk("Cancel", "button", "dp-hbtn");
  form.append(field("Image URL", url), field("or upload an image", file), err, acts);
  const toggle = (open) => {
    form.hidden = !open;
    btn.setAttribute("aria-expanded", String(open));
    head.classList.toggle("dp-head-editing", open);
    err.textContent = "";
    if (open) url.focus();
  };
  btn.onclick = () => toggle(form.hidden);
  cancel.onclick = () => {
    toggle(false);
    btn.focus();
  };
  form.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      cancel.onclick();
    }
  });
  form.onsubmit = async (e) => {
    e.preventDefault();
    let v = url.value.trim();
    const f = file.files && file.files[0];
    if (f) {
      if (!LOGO_TYPES.includes(f.type)) return (err.textContent = "Choose a PNG, JPEG, GIF, WebP or SVG image.");
      if (f.size > LOGO_MAX_BYTES)
        return (err.textContent = `The image is ${Math.ceil(f.size / 1024)} KB; logos are embedded up to ${LOGO_MAX_BYTES / 1024} KB.`);
      v = await new Promise((ok, no) => {
        const r = new FileReader();
        r.onload = () => ok(String(r.result));
        r.onerror = () => no(r.error);
        r.readAsDataURL(f);
      }).catch(() => "");
    }
    if (!v) return (err.textContent = "Enter an image URL or choose a file.");
    if (!chromeUrlOk(v)) return (err.textContent = "Not an allowed image URL: use a relative path, https: or data:image/.");
    editChrome("LOGO", v, "head:logo");
  };
  wrap.append(btn, form);
  refocus("head:logo", btn);
  return wrap;
}
let dpVars = {}; // DuckDB variable name -> selected value (persists across runs)
let dpVarNow = {}; // variable -> SQL literal set during this run (markPending)
let dpVarLast = {}; // … and at the end of the last completed run
let dpCols = 2; // default panels-per-row on the 12-column grid
let dpFilter = ""; // generic cross-filter: last clicked value, as getvariable('selected')
let dpXf = {}; // named cross-filters: column-name -> value, each getvariable('<name>')
let dpPage = {}; // ::PAGED tables: statement index -> current page (server-side)
let dpSort = {}; // ::PAGED tables: statement index -> {col, dir} (server-side sort)
let dpKbdActive = false; // one-shot: keep row highlight after a server page-change
let dpNav = null; // arrow-key controller for the table the pointer is over
let dpPageSize = {}; // ::PAGED tables: statement idx -> rows per page
let dpFilterText = {}; // ::PAGED tables: statement idx -> full-text filter term
let dpPaged = {}; // ::PAGED tables: statement idx -> {colEq, colQ, colRange, view, …} (column filters, Scroll view)
let dpTableUI = {}; // client ::TABLE: statement idx -> {sortCol,dir,page,pageSize,colQ} — survives a cross-filter re-run so sort/filter/page persist

// Arrow-key table navigation acts on whichever table the pointer is over
// (dpNav), so no clicking/tabbing is needed — and browsing rows doesn't trigger
// a re-run. Enter drills into the highlighted row (cross-filter). Ignored while
// typing in an input or the SQL editor.
document.addEventListener("keydown", (e) => {
  if (!dpNav) return;
  const el = document.activeElement;
  const tag = ((el && el.tagName) || "").toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select" || (el && el.isContentEditable)) return;
  switch (e.key) {
    case "ArrowDown": e.preventDefault(); dpNav.move(1); break;
    case "ArrowUp": e.preventDefault(); dpNav.move(-1); break;
    case "Home": e.preventDefault(); dpNav.move("home"); break;
    case "End": e.preventDefault(); dpNav.move("end"); break;
    case "ArrowRight": case "PageDown": e.preventDefault(); dpNav.page(1); break;
    case "ArrowLeft": case "PageUp": e.preventDefault(); dpNav.page(-1); break;
    case "Enter": e.preventDefault(); dpNav.drill(); break;
  }
});
let dpTab = null; // the active tab name (preserved across re-runs)
let dpSubTab = {}; // top-tab name -> active sub-tab name (nested tabs)

// Deep-linking: reflect the active tab in the URL (?tab=) so a reload or a
// shared link lands on the same page.
function getTabParam() {
  try {
    return new URLSearchParams(location.search).get("tab");
  } catch (e) {
    return null;
  }
}
function setTabParam(name) {
  try {
    const u = new URL(location.href);
    u.searchParams.set("tab", name);
    history.replaceState(null, "", u);
  } catch (e) {}
}

// Viewer state in the URL: `?v.<name>=<value>` seeds a variable on load — an
// input's value (checked against the input's own options / type first), the
// shared `selected`, or a named selection. A list repeats the key
// (`?v.channel=app&v.channel=web`; `?v.channel=` is an empty list). After every
// run the URL is rewritten (history.replaceState) with the inputs that differ
// from their defaults and the live selections, so a reload or a copied link
// restores the view. Only variable VALUES travel — never SQL — so a locked
// server binds them as typed literals like any other input.
const URL_VAR_RE = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
let dpUrlSeed = readUrlSeed(); // variable -> [values] from ?v.* (consumed by the first run)
let dpVarDefault = {}; // input variable -> its default (JSON text): only changes go into the URL
let dpPeriodUrl = {}; // ::PERIOD column -> {prefix, def} (def = its resolved default) for the URL
let dpSeedRerun = false; // a seeded table selection changed shape: re-run once after this run
let dpSeeded = new Set(); // named selections seeded from the URL whose shape a table still fixes
function readUrlSeed(search = location.search) {
  const seed = {};
  try {
    for (const [k, v] of new URLSearchParams(search)) {
      if (!k.startsWith("v.")) continue;
      const name = k.slice(2);
      // __claim_* (the login's claims) and anofox_data are set by a locked server only.
      if (!URL_VAR_RE.test(name) || /^(__claim_|anofox_data$)/i.test(name) || String(v).length > 1000) continue;
      if (!seed[name] && Object.keys(seed).length >= 64) continue;
      (seed[name] ||= []).length < 200 && seed[name].push(String(v));
    }
  } catch (_) {}
  return seed;
}
function isIsoDate(v) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(v)) return false;
  const d = new Date(v + "T00:00:00Z");
  return !isNaN(d) && d.toISOString().slice(0, 10) === v;
}
// A seeded input value if it passes the input's own validation, else undefined
// (the input keeps its default).
function validSeed(kind, vals, options) {
  const one = vals.length === 1 ? vals[0] : undefined;
  switch (kind) {
    case "DROPDOWN":
    case "BUTTONS":
      return options && options.includes(one) ? one : undefined;
    case "TOGGLE":
      return one !== undefined && /^(true|false|t|f|1|0|yes|no|on|off)$/i.test(one) ? toggleValue(one) : undefined;
    case "MULTISELECT": {
      const want = vals.filter((v) => v !== "");
      const ok = want.filter((v) => options && options.includes(v));
      return ok.length === want.length || ok.length ? [...new Set(ok)] : undefined;
    }
    case "NUMBER": {
      const n = Number(one);
      return one !== undefined && /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(one) && Math.abs(n) < 1e15 ? one : undefined;
    }
    case "DATE":
    case "DATERANGE":
      return one !== undefined && isIsoDate(one) ? one : undefined;
    default: // TEXT
      return one !== undefined && one.length <= 500 ? one : undefined;
  }
}
// A ::PERIOD's URL seed -> the control's state, or undefined. `v.p=30d` (a
// preset) or `v.p=2026-01-01..2026-03-31` (a custom range), plus optional
// `v.p_grain=week` and `v.p_compare=prev|yoy|none`. `resolve(state)` is
// period_resolve for this input's spec: the range must resolve without a new
// error, grain and comparison must be offered; an invalid part is dropped.
function validPeriodSeed(seed, prefix, resolve) {
  const one = (k) => (seed[k] && seed[k].length === 1 ? seed[k][0] : undefined);
  const base = resolve({});
  const nerr = (base.errors || []).length;
  const fits = (st) => {
    const r = resolve(st);
    return !!r.vars && (r.errors || []).length === nerr;
  };
  const st = {};
  const range = one(prefix);
  if (range !== undefined) {
    const m = range.match(/^(\d{4}-\d{2}-\d{2})\.\.(\d{4}-\d{2}-\d{2})$/);
    const part = m
      ? isIsoDate(m[1]) && isIsoDate(m[2])
        ? { preset: "custom", from: m[1], to: m[2] }
        : null
      : /^[a-z0-9_]{1,20}$/i.test(range) && range !== "custom"
        ? { preset: range }
        : null;
    if (part && fits(part)) Object.assign(st, part);
  }
  const grain = one(prefix + "_grain");
  if (grain !== undefined && (base.grains || []).includes(grain)) st.grain = grain;
  const cmp = one(prefix + "_compare");
  if (cmp !== undefined && (base.compares || []).some((c) => c.id === cmp)) st.compare = cmp;
  return Object.keys(st).length && fits(st) ? st : undefined;
}
// A ::PERIOD state as URL parameters (the inverse of validPeriodSeed): only what
// differs from the input's default; an explicit grain always (it stops the
// automatic one).
function periodUrlParams(prefix, st, def) {
  const out = [];
  if (!st || typeof st !== "object") return out;
  if (st.preset === "custom" && st.from && st.to) out.push([prefix, `${st.from}..${st.to}`]);
  else if (st.preset && st.preset !== def.preset) out.push([prefix, st.preset]);
  if (st.grain) out.push([prefix + "_grain", st.grain]);
  if (st.compare && st.compare !== def.compare) out.push([prefix + "_compare", st.compare]);
  return out;
}
// A ::TOGGLE value as "true" / "false" (its default may be a BOOLEAN, a number
// or text such as 'on'); unknown text is "false".
function toggleValue(v) {
  return /^(true|t|1|yes|on)$/i.test(String(v ?? "").trim()) ? "true" : "false";
}
// Record an input's default and apply its URL seed (first run only, while the
// variable is still unset). `names` = [var] or a ::DATERANGE's [from, to].
function seedInputFromUrl(kind, names, rows) {
  for (const name of names) {
    const listed = kind === "DROPDOWN" || kind === "BUTTONS" || kind === "MULTISELECT";
    const options = listed ? rows.map((r) => String(r[name])) : null;
    dpVarDefault[name] = JSON.stringify(
      kind === "MULTISELECT"
        ? options
        : listed
          ? options[0]
          : kind === "TOGGLE"
            ? toggleValue(rows[0][name])
            : String(rows[0][name] ?? "")
    );
    const vals = dpUrlSeed[name];
    if (!vals) continue;
    delete dpUrlSeed[name];
    const v = dpVars[name] === undefined ? validSeed(kind, vals, options) : undefined;
    if (v !== undefined) dpVars[name] = v;
  }
}
// Seeds left after the input pre-pass are selections: `selected` (the shared
// click filter) or a named one (one value = text, several = a list; a table's
// ::ROWSELECT mode fixes the shape when it renders). Returns true if any applied.
function applySelectionSeeds() {
  let any = false;
  for (const [name, vals] of Object.entries(dpUrlSeed)) {
    delete dpUrlSeed[name];
    if (vals.some((v) => v.length > 500)) continue;
    if (name === "selected") {
      dpFilter = vals[0] || "";
      dpSelected = dpFilter || null;
    } else {
      dpXf[name] = vals.length > 1 ? vals : vals[0];
      dpSeeded.add(name);
    }
    any = true;
  }
  return any;
}
// A seeded named selection whose table turned out to be 'multi' (or 'single')
// takes that shape; the dashboard re-runs once with it.
function coerceSeededSelection(name, mode) {
  if (!dpSeeded.delete(name)) return;
  const v = dpXf[name];
  if (mode === "multi" && typeof v === "string") dpXf[name] = v ? [v] : [];
  else if (mode === "single" && Array.isArray(v)) dpXf[name] = v[0] ?? "";
  else return;
  dpSeedRerun = true;
}
// The cross-filter variables — the generic `selected` (last click) plus any
// NAMED cross-filters (each table emits getvariable('<its first column>') so two
// tables give two independent live selections). Unset named vars read as NULL,
// so targets guard with COALESCE(getvariable('name'),'').
async function setSelectionVars() {
  try {
    await runSql(`SET VARIABLE selected = '${dpFilter.replace(/'/g, "''")}'`);
    const q = (x) => `'${String(x).replace(/'/g, "''")}'`;
    for (const [k, v] of Object.entries(dpXf)) {
      if (URL_VAR_RE.test(k)) {
        // a multi-select table keeps a list: getvariable('<column>') is VARCHAR[]
        const lit = Array.isArray(v) ? (v.length ? `[${v.map(q).join(", ")}]` : "[]::VARCHAR[]") : q(v);
        await runSql(`SET VARIABLE ${k} = ${lit}`);
      }
    }
  } catch (e) {
    /* ignore */
  }
}
// The viewer state as `v.*` query parameters (inputs that differ from their
// default, the live selections), merged into the current URL.
function urlStateParams() {
  const out = [];
  const add = (k, v) => (Array.isArray(v) ? (v.length ? v : [""]) : [v]).forEach((x) => out.push(["v." + k, String(x)]));
  for (const [k, def] of Object.entries(dpVarDefault)) {
    if (dpVars[k] !== undefined && JSON.stringify(dpVars[k]) !== def) add(k, dpVars[k]);
  }
  for (const [col, p] of Object.entries(dpPeriodUrl)) {
    for (const [k, v] of periodUrlParams(p.prefix, dpVars[col], p.def)) out.push(["v." + k, v]);
  }
  if (dpFilter) add("selected", dpFilter);
  for (const [k, v] of Object.entries(dpXf)) {
    if (k in dpVarDefault || v == null || v === "" || (Array.isArray(v) && !v.length)) continue;
    add(k, v);
  }
  for (const [k, r] of Object.entries(dpRange)) {
    if (r) add(k + "_from", r.from), add(k + "_to", r.to);
  }
  for (const [p, path] of Object.entries(dpDrill)) if (path.length) add(p + "_path", path);
  return out;
}
function syncUrlState() {
  try {
    const u = new URL(location.href);
    for (const k of [...u.searchParams.keys()]) if (k.startsWith("v.")) u.searchParams.delete(k);
    for (const [k, v] of urlStateParams()) u.searchParams.append(k, v);
    if (u.href !== location.href) history.replaceState(null, "", u);
  } catch (_) {}
}

// ---- chart interactions: named selections (::SELECT_AS) ----
// `'region'::SELECT_AS` before a chart: a click sets getvariable('region') (''
// = nothing) instead of the shared `selected`; `'region multi'` keeps a list
// (VARCHAR[], [] = nothing) — a click selects one value, shift/ctrl-click adds
// or removes one. A brush (toolbox ▧) on a continuous / date x sets
// `region_from` / `region_to` (NULL = no range); on a discrete x in multi mode
// it selects the brushed values. Esc or a click on empty space clears.
let dpIa = {}; // statement index -> {select: {name, mode}} (this run's interaction directives)
let dpRange = {}; // named selection -> {from, to} brushed x range (numbers or ISO dates)
function parseSelectAs(v) {
  const m = String(v ?? "")
    .trim()
    .match(/^([A-Za-z_][A-Za-z0-9_]{0,59})(?:\s*[\s:,]\s*(single|multi))?$/i);
  return m ? { name: m[1], mode: (m[2] || "single").toLowerCase() } : null;
}
// Evaluate the interaction directives before any panel queries, so their
// variables exist (and URL seeds apply) when the panels run.
async function prepInteractions(stmts) {
  dpIa = {};
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    const d = s.setup ? null : directive(s);
    if (d !== "SELECT_AS" && d !== "DRILL") continue;
    try {
      const rows = JSON.parse(await runSql(s.sql));
      if (d === "DRILL") {
        const drill = parseDrill(rows[0] ? Object.values(rows[0])[0] : "");
        if (!drill) continue;
        dpIa[i] = { drill };
        seedDrill(drill);
        continue;
      }
      const sel = parseSelectAs(rows[0] ? Object.values(rows[0])[0] : "");
      if (!sel) continue;
      dpIa[i] = { select: sel };
      seedNamedSelection(sel);
      const cur = dpXf[sel.name];
      if (sel.mode === "multi" && !Array.isArray(cur)) dpXf[sel.name] = cur ? [cur] : [];
      if (sel.mode === "single" && typeof cur !== "string") dpXf[sel.name] = Array.isArray(cur) ? cur[0] ?? "" : "";
      dpRange[sel.name] ||= null;
    } catch (_) {}
  }
}
function seedNamedSelection(sel) {
  const take = (k) => {
    const v = dpUrlSeed[k];
    delete dpUrlSeed[k];
    return v && v.every((x) => x.length <= 500) ? v : null;
  };
  const v = take(sel.name);
  if (v) dpXf[sel.name] = sel.mode === "multi" ? v.filter((x) => x !== "") : v[0];
  const f = take(sel.name + "_from");
  const t = take(sel.name + "_to");
  const typed = (x) => (x && /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(x[0]) && Number.isFinite(+x[0]) ? +x[0] : x && isIsoDate(x[0].slice(0, 10)) ? x[0] : null);
  if (typed(f) !== null && typed(t) !== null) dpRange[sel.name] = { from: typed(f), to: typed(t) };
}
// ---- drill-down (::DRILL) and drill-through (::LINK) ----
// `'region > country > city'::DRILL` before a chart: the chart shows one level
// at a time. Its variables (prefix `drill`, or `geo` for 'geo: region > …'):
//   <p>_level  the level on screen ('country')   <p>_depth  0-based depth
//   <p>_path   the values drilled through, VARCHAR[] (['EU'])
//   region, country, city — the value chosen at each level above, else ''.
// A click on a mark goes one level deeper; the breadcrumb above the chart goes
// back. On the last level a click falls through to ::LINK / ::SELECT_AS /
// `selected`. The path is part of the URL state (?v.drill_path=EU).
let dpDrill = {}; // drill prefix -> path (array of values)
function parseDrill(v) {
  const m = String(v ?? "").match(/^\s*(?:([A-Za-z_][A-Za-z0-9_]{0,59})\s*:)?([^:]*)$/);
  if (!m) return null;
  const levels = m[2].split(">").map((x) => x.trim());
  if (levels.length < 2 || !levels.every((l) => /^[A-Za-z_][A-Za-z0-9_]{0,59}$/.test(l))) return null;
  return { prefix: m[1] || "drill", levels };
}
function seedDrill(drill) {
  const k = drill.prefix + "_path";
  const seed = dpUrlSeed[k];
  delete dpUrlSeed[k];
  if (seed && seed.every((x) => x.length <= 500)) dpDrill[drill.prefix] = seed.filter((x) => x !== "");
  // never deeper than the last level (the levels may have changed)
  dpDrill[drill.prefix] = (dpDrill[drill.prefix] || []).slice(0, drill.levels.length - 1);
}
// The drill variables of this run's ::DRILL charts.
async function setDrillVars() {
  const q = (x) => `'${String(x).replace(/'/g, "''")}'`;
  for (const ia of Object.values(dpIa)) {
    if (!ia.drill) continue;
    const { prefix, levels } = ia.drill;
    const path = dpDrill[prefix] || [];
    try {
      await runSql(`SET VARIABLE ${prefix}_level = ${q(levels[path.length])}`);
      await runSql(`SET VARIABLE ${prefix}_depth = ${path.length}`);
      await runSql(`SET VARIABLE ${prefix}_path = ${path.length ? `[${path.map(q).join(", ")}]` : "[]::VARCHAR[]"}`);
      for (let j = 0; j < levels.length; j++) await runSql(`SET VARIABLE ${levels[j]} = ${q(j < path.length ? path[j] : "")}`);
    } catch (_) {}
  }
}
// The breadcrumb above a ::DRILL chart: All › EU › Germany · city. Each crumb
// but the last goes back to that depth; the focus then moves to the new last
// crumb (the clicked one is gone) and the chart zooms out to the value left.
function drillCrumbs(drill) {
  const path = dpDrill[drill.prefix] || [];
  const nav = document.createElement("nav");
  nav.className = "dp-crumbs";
  nav.dataset.drill = drill.prefix;
  nav.setAttribute("aria-label", "Drill path");
  const crumb = (text, depth) => {
    if (depth === path.length) {
      const cur = document.createElement("span");
      cur.className = "dp-crumb-cur";
      cur.setAttribute("aria-current", "location");
      cur.tabIndex = -1; // a focus target after going back
      cur.textContent = text;
      return cur;
    }
    const b = document.createElement("button");
    b.className = "dp-crumb";
    b.textContent = text;
    b.title = depth ? `Back to ${text}` : "Back to the top level";
    b.onclick = (e) => {
      e.stopPropagation();
      dpDrill[drill.prefix] = path.slice(0, depth);
      const focus = () => document.querySelector(`.dash .dp-crumbs[data-drill="${drill.prefix}"] .dp-crumb-cur`)?.focus({ preventScroll: true });
      dpMotion = { drill: drill.prefix, key: path[depth], down: false, focus };
      run(false);
    };
    return b;
  };
  nav.appendChild(crumb("All", 0));
  path.forEach((v, j) => {
    const sep = document.createElement("span");
    sep.className = "dp-crumb-sep";
    sep.textContent = "›";
    nav.append(sep, crumb(v, j + 1));
  });
  const lvl = document.createElement("span");
  lvl.className = "dp-crumb-level";
  const last = path.length >= drill.levels.length - 1;
  lvl.textContent = last ? `by ${drill.levels[path.length]}` : `by ${drill.levels[path.length]} — click to drill into ${drill.levels[path.length + 1]}`;
  nav.appendChild(lvl);
  return nav;
}
// A click on a ::DRILL chart's mark: one level deeper. False on the last level
// (the click then does what it would without ::DRILL).
function drillInto(drill, key) {
  const path = dpDrill[drill.prefix] || [];
  if (path.length >= drill.levels.length - 1 || key === "") return false;
  dpDrill[drill.prefix] = [...path, key];
  const focus = () => document.querySelector(`.dash .dp-crumbs[data-drill="${drill.prefix}"] .dp-crumb-cur`)?.focus({ preventScroll: true });
  dpMotion = { drill: drill.prefix, key, down: true, focus };
  run(false);
  return true;
}
// Fill a ::LINK template: {value} = the clicked key, {x} / {series} on a chart,
// any {column} of a table row, the drill levels — each URL-encoded.
function fillLink(tpl, vars) {
  return String(tpl ?? "").replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (_, k) =>
    encodeURIComponent(vars[k] == null ? "" : String(vars[k]))
  );
}
// Follow a filled ::LINK: '#Tab' (optionally '#Tab?v.x=…') opens a tab of this
// dashboard, '?v.x=…' sets variables here (seeded like URL parameters — inputs
// are validated), a path ('/d/<id>?v.x=…') navigates, an http(s) URL opens in
// a new tab. Other schemes are refused (safeUrl).
function followLink(url) {
  const u = String(url ?? "").trim();
  if (u.startsWith("#") || u.startsWith("?")) {
    const qi = u.indexOf("?");
    const tab = u.startsWith("#") ? decodeURIComponent(qi < 0 ? u.slice(1) : u.slice(1, qi)) : null;
    const qs = qi < 0 ? "" : u.slice(qi);
    const seed = readUrlSeed(qs);
    for (const k of Object.keys(seed)) if (k in dpVarDefault) delete dpVars[k]; // re-seeded (validated) by the run
    for (const [col, p] of Object.entries(dpPeriodUrl)) {
      if ([p.prefix, p.prefix + "_grain", p.prefix + "_compare"].some((k) => k in seed)) delete dpVars[col];
    }
    Object.assign(dpUrlSeed, seed);
    const t = tab || new URLSearchParams(qs).get("tab");
    dpMotion = { link: true };
    if (t) {
      dpTab = t;
      setTabParam(t);
      openTabNow(t);
    }
    dpLinkFrom = null;
    run(false);
    return;
  }
  const safe = safeUrl(u);
  if (!safe) return status("link blocked: only #tab, ?v.…, paths and http(s) URLs are followed");
  if (/^https?:/i.test(safe) && !safe.startsWith(location.origin + "/")) window.open(safe, "_blank", "noopener");
  else location.href = safe;
}
// Drill-through to a tab: show it at once — its panels as they are, under the
// loading veil (no delay: they may still show the previous value) — and fade
// it in; the run then updates it in place. The focus goes to the tab's first
// panel title (announced; else its tab button), and Esc there goes back to
// the link's origin (dpLinkBack).
let dpLinkFrom = null; // the element a ::LINK was followed from: {stmt, series, x} (a mark) or {stmt, link} (a table link)
let dpLinkBack = null; // {from, to, origin}: the tab switch a drill-through made
const activePane = () => [...document.querySelectorAll(".dash > .tabwrap > .tabpane")].find((p) => p.style.display !== "none");
function openTabNow(name) {
  const origin = dpLinkFrom;
  dpLinkFrom = null;
  const btn = [...document.querySelectorAll(".dash > .tabbar > .tab-btn")].find((b) => b.textContent === name);
  if (!btn) return;
  const focus = () => {
    const pane = activePane();
    const t = pane?.querySelector(".panel-title") || pane?.querySelector(".group-title");
    if (!t) return document.querySelector(".dash > .tabbar > .tab-btn.active")?.focus({ preventScroll: true });
    if (!t.hasAttribute("tabindex")) t.tabIndex = -1;
    t.focus({ preventScroll: true });
  };
  dpMotion.focus = focus;
  if (btn.classList.contains("active")) return;
  const from = document.querySelector(".dash > .tabbar > .tab-btn.active")?.textContent;
  dpA11yFocus = null; // the focus leaves the chart: it is not handed back after the run
  btn.click();
  dpLinkBack = from ? { from, to: name, origin } : null;
  const pane = activePane();
  if (pane) {
    pane.classList.add("dp-switching");
    if (!dpAnim.still() && pane.animate) pane.animate([{ opacity: 0.25 }, { opacity: 1 }], { duration: 200, easing: "ease-out" });
  }
  focus();
}
// Esc on a drill-through's tab (not inside a chart, which has its own Esc, or
// a control): back to the tab it came from, focus on the mark or link followed.
document.addEventListener("keydown", (e) => {
  const b = dpLinkBack;
  if (e.key !== "Escape" || !b || e.defaultPrevented) return;
  const ae = document.activeElement;
  const cur = document.querySelector(".dash > .tabbar > .tab-btn.active");
  if (!cur || cur.textContent !== b.to) return (dpLinkBack = null);
  if (ae && ae !== document.body && (!activePane()?.contains(ae) || ae.closest(".panel-svg, input, select, textarea, details"))) return;
  const btn = [...document.querySelectorAll(".dash > .tabbar > .tab-btn")].find((x) => x.textContent === b.from);
  if (!btn) return;
  e.preventDefault();
  dpLinkBack = null;
  btn.click();
  const o = b.origin;
  const fig = o && document.querySelector(`.dash .panel[data-stmt="${o.stmt}"]`);
  let to = null;
  if (fig && o.link != null) to = fig.querySelectorAll(".dp-link")[o.link];
  else if (fig) {
    const svg = fig.querySelector(".panel-svg > svg");
    to = (svg && chartMarks(svg).find((m) => m.getAttribute("data-series") === o.series && (m.getAttribute("data-x") || "") === o.x)) || svg;
  }
  (to || btn).focus({ preventScroll: true });
});
// A chart mark's ::LINK: the template of the row the mark was drawn from
// (matched on x and series), filled with the mark's values and the drill path.
function chartLink(link, el, key, series) {
  let rows = [];
  try {
    rows = JSON.parse(link.rows);
  } catch (_) {}
  const col = (name) => link.roles.find((r) => r[1] === name);
  const xr = col("XAXIS") || col("DIAGRAM"); // a ::DIAGRAM node's x is its id
  const cr = col("CATEGORY");
  const x = el.getAttribute("data-x") || "";
  const ds = el.getAttribute("data-series");
  const str = (v) => (v == null ? "" : String(v).replace(/^"|"$/g, ""));
  const row =
    rows.find((r) => (!xr || str(r["c" + xr[0]]) === x) && (!cr || ds == null || str(r["c" + cr[0]]) === ds)) ||
    rows.find((r) => (xr && str(r["c" + xr[0]]) === key) || (cr && str(r["c" + cr[0]]) === key)) ||
    rows[0] ||
    {};
  const vars = { value: key, x: xr ? str(row["c" + xr[0]]) : x, series };
  for (const ia of Object.values(dpIa)) {
    if (ia.drill) (dpDrill[ia.drill.prefix] || []).forEach((v, j) => (vars[ia.drill.levels[j]] = v));
  }
  return fillLink(str(row["c" + link.idx]), vars);
}
// A table's ::LINK cell: a link built from the row ({value} = its first column).
function linkCell(tpl, row, key) {
  const vars = { value: key };
  for (const [k, v] of Object.entries(row)) vars[k] = v == null ? "" : String(v).replace(/^"|"$/g, "");
  const url = fillLink(tpl, vars);
  const a = document.createElement("a");
  a.className = "dp-link";
  const tab = url.startsWith("#") ? decodeURIComponent(url.slice(1).split("?")[0]) : "";
  const external = /^https?:/i.test(url) && !url.startsWith(location.origin + "/");
  a.textContent = tab ? `${tab} →` : external ? "Open ↗" : "Open →";
  a.href = url.startsWith("#") || url.startsWith("?") ? "#" : safeUrl(url) || "#";
  a.title = url;
  if (external) {
    a.target = "_blank";
    a.rel = "noopener";
  }
  a.onclick = (e) => {
    e.stopPropagation(); // not a row selection
    if (e.ctrlKey || e.metaKey || e.shiftKey || external) return; // the browser opens it
    e.preventDefault();
    const fig = a.closest(".panel[data-stmt]");
    dpLinkFrom = fig ? { stmt: fig.dataset.stmt, link: [...fig.querySelectorAll(".dp-link")].indexOf(a) } : null;
    followLink(url);
  };
  return a;
}

// The brushed ranges as variables: <name>_from / <name>_to (NULL when unset).
async function setRangeVars() {
  for (const [name, r] of Object.entries(dpRange)) {
    const lit = (v) => (v == null ? "NULL" : typeof v === "number" ? String(v) : `'${String(v).replace(/'/g, "''")}'`);
    try {
      await runSql(`SET VARIABLE ${name}_from = ${lit(r && r.from)}`);
      await runSql(`SET VARIABLE ${name}_to = ${lit(r && r.to)}`);
    } catch (_) {}
  }
}
// A click on a mark of a ::SELECT_AS chart. `add` (shift/ctrl/⌘) toggles one
// value in a multi selection; a plain click selects just that value (again: clear).
function clickNamedSelection(sel, key, add) {
  const cur = dpXf[sel.name];
  if (sel.mode !== "multi") dpXf[sel.name] = cur === key ? "" : key;
  else {
    const set = new Set(Array.isArray(cur) ? cur : []);
    if (add) set.has(key) ? set.delete(key) : set.add(key);
    else if (set.size === 1 && set.has(key)) set.clear();
    else {
      set.clear();
      set.add(key);
    }
    dpXf[sel.name] = [...set];
  }
  run(false);
}
// The shape of a chart's x from its marks' data-x: "num", "date" or null (discrete).
function xKindOf(xs) {
  if (!xs.length) return null;
  if (xs.every((x) => x !== "" && Number.isFinite(Number(x)))) return "num";
  if (xs.every((x) => /^\d{4}-\d{2}-\d{2}/.test(x))) return "date";
  return null;
}
const xVal = (x, kind) => (kind === "num" ? Number(x) : x);
// Clear every click / brush selection (Esc, a click on empty space). Returns
// true if there was anything to clear.
function clearSelections() {
  const anyNamed = Object.values(dpXf).some((v) => (Array.isArray(v) ? v.length : v));
  const anyRange = Object.values(dpRange).some((r) => r);
  if (!dpFilter && dpSelected === null && !anyNamed && !anyRange) return false;
  dpFilter = "";
  dpSelected = null;
  for (const k in dpXf) dpXf[k] = Array.isArray(dpXf[k]) ? [] : "";
  for (const k in dpRange) dpRange[k] = null;
  return true;
}
// After a render: dim the marks outside each named selection / brushed range
// and show what is selected (with a ✕ to clear) above the chart.
function decorateSelections() {
  document.querySelectorAll(".dash .panel").forEach((panel) => {
    const drill = panel._ia && panel._ia.drill;
    if (drill) {
      const was = panel.querySelector(".dp-crumbs");
      const nav = drillCrumbs(drill);
      if (!was || was.outerHTML !== nav.outerHTML) {
        was?.remove();
        const head = panel.querySelector(".panel-title");
        head ? head.after(nav) : panel.prepend(nav);
      }
    }
    if (panel._ia && panel._ia.link) panel.classList.add("dp-linked");
    const sel = panel._ia && panel._ia.select;
    if (!sel) return;
    panel.classList.add("dp-selectable");
    const cur = dpXf[sel.name];
    const keys = new Set(Array.isArray(cur) ? cur : cur ? [cur] : []);
    const r = dpRange[sel.name];
    const kind = r ? xKindOf([...marksOf(panel)].map((el) => el.getAttribute("data-x") || "")) : null;
    if (keys.size || (r && kind)) {
      filterMarks(panel, (el) => {
        if (keys.size && !keys.has(el.getAttribute("data-series"))) return false;
        if (!r || !kind) return true;
        const x = xVal(el.getAttribute("data-x") || "", kind);
        return x >= r.from && x <= r.to;
      });
    }
    // The brushed range stays visible as a translucent band over the marks it kept.
    const holder = panel.querySelector(".panel-svg");
    holder?.querySelector(".dp-range-band")?.remove();
    if (r && kind && holder) {
      const hr = holder.getBoundingClientRect();
      const xs = [...marksOf(panel)]
        .filter((el) => {
          const x = xVal(el.getAttribute("data-x") || "", kind);
          return x >= r.from && x <= r.to;
        })
        .map((el) => el.getBoundingClientRect())
        .flatMap((b) => [b.left, b.right]);
      if (xs.length && hr.width) {
        const band = document.createElement("div");
        band.className = "dp-range-band";
        band.style.left = Math.min(...xs) - hr.left - 4 + "px";
        band.style.width = Math.max(...xs) - Math.min(...xs) + 8 + "px";
        holder.appendChild(band);
      }
    }
    panel.querySelector(".dp-ia-bar")?.remove();
    const parts = [];
    if (keys.size) parts.push([`${sel.name}: ${[...keys].join(", ")}`, () => (dpXf[sel.name] = Array.isArray(cur) ? [] : "")]);
    if (r) parts.push([`${sel.name}: ${r.from} – ${r.to}`, () => (dpRange[sel.name] = null)]);
    if (!parts.length) return;
    const bar = document.createElement("div");
    bar.className = "dp-ia-bar";
    for (const [text, clear] of parts) {
      const chip = document.createElement("span");
      chip.className = "tbl-chip";
      chip.title = text;
      chip.append(text);
      const x = document.createElement("button");
      x.textContent = "×";
      x.title = "Clear this selection";
      x.setAttribute("aria-label", "Clear " + text);
      x.onclick = (e) => {
        e.stopPropagation();
        clear();
        run(false);
      };
      chip.appendChild(x);
      bar.appendChild(chip);
    }
    const anchor = panel.querySelector(".dp-crumbs") || panel.querySelector(".panel-title");
    anchor ? anchor.after(bar) : panel.prepend(bar);
  });
}

// The parts of a dashboard a re-run may swap one by one: panels and controls
// (outermost; a control's value lives in its DOM, so it is always replaced).
const SLOTS = ".panel[data-stmt], .control, .controls";
function slotsOf(roots) {
  const out = [];
  for (const r of roots) if (r) for (const el of r.querySelectorAll(SLOTS)) if (!el.parentElement.closest(SLOTS)) out.push(el);
  return out;
}
// A dashboard's layout without its slots and its open tab: two runs with the
// same shape differ only in panel content.
// (Serialised with the slots and the tab state taken out for a moment — no
// copy of the charts.)
function shapeOf(roots) {
  const slots = slotsOf(roots);
  const holes = slots.map((el) => {
    const i = document.createElement("i");
    el.replaceWith(i);
    return i;
  });
  const tabs = roots.flatMap((r) => (r ? [...r.querySelectorAll(".tab-btn, .tabpane, .subtabpane")] : []));
  const attr = (el) => (el.matches(".tab-btn") ? "class" : "style");
  const was = tabs.map((el) => el.getAttribute(attr(el)));
  tabs.forEach((el) => (el.matches(".tab-btn") ? el.classList.remove("active") : el.style.removeProperty("display")));
  const html = roots.map((r) => (r ? r.outerHTML : "")).join("");
  tabs.forEach((el, k) => (was[k] == null ? el.removeAttribute(attr(el)) : el.setAttribute(attr(el), was[k])));
  holes.forEach((i, k) => i.replaceWith(slots[k]));
  return html;
}
// Patch the dashboard on screen (`roots`) to the new one: keep each panel
// whose output is unchanged, replace the others (pairs in `swaps`), and open
// the new run's tabs. False (nothing touched) when the slots do not line up.
function patchSlots(roots, newRoots, swaps) {
  const a = slotsOf(roots);
  const b = slotsOf(newRoots);
  if (a.length !== b.length) return false;
  const tabState = (rs) => rs.flatMap((r) => (r ? [...r.querySelectorAll(".tab-btn, .tabpane, .subtabpane")] : []));
  const ta = tabState(roots);
  const tb = tabState(newRoots);
  if (ta.length !== tb.length) return false;
  ta.forEach((el, k) => {
    if (el.matches(".tab-btn")) el.classList.toggle("active", tb[k].classList.contains("active"));
    else el.style.display = tb[k].style.display;
  });
  a.forEach((o, k) => {
    const n = b[k];
    if (o.matches(".panel") && n.matches(".panel") && !n._async && o._sig === n._sig) {
      o.classList.remove("dp-pending");
      o.removeAttribute("aria-busy");
      return;
    }
    swaps.push([o, n]);
    o.replaceWith(n);
  });
  return true;
}

// Per-panel loading state (no full-page veil): while a run is in flight the
// panels on screen whose query reads a variable that changed (all of them on a
// fresh run) get a placeholder shimmer + aria-busy; the rest stay untouched.
// The first run, with nothing on screen yet, draws skeleton cards instead (a
// tabbed dashboard's panels live in .tabwrap, outside the grid).
const varsOf = (sql) => [...String(sql).matchAll(/getvariable\s*\(\s*'([^']+)'/gi)].map((m) => m[1]);
function markPending(grid, stmts, fresh) {
  const changed = new Set(
    [...new Set([...Object.keys(dpVarNow), ...Object.keys(dpVarLast)])].filter((k) => dpVarNow[k] !== dpVarLast[k])
  );
  for (const fig of document.querySelectorAll(".dash .panel[data-stmt]")) {
    const s = stmts[+fig.dataset.stmt];
    if (s && (fresh || varsOf(s.sql).some((v) => changed.has(v)))) {
      fig.classList.add("dp-pending");
      fig.setAttribute("aria-busy", "true");
    }
  }
  if (document.querySelector(".dash .panel")) return;
  // First run: one skeleton card per visible panel (up to 12), KPI tiles short.
  const skel = [];
  for (const s of stmts) {
    if (skel.length >= 12) break;
    if (s.setup || directive(s) || isInput(s) || chromeRole(s) || isHeading(s) || role(s, "SECTION")) continue;
    const el = document.createElement("div");
    const kpi = !!metricRole(s) && !role(s, "TABLE");
    el.className = "panel dp-skel" + (kpi ? " dp-skel-kpi" : "");
    el.setAttribute("aria-hidden", "true");
    el.style.gridColumn = `span ${kpi ? 3 : Math.max(1, Math.round(12 / dpCols))}`;
    el.innerHTML = "<i></i><i></i><i></i>";
    skel.push(el);
  }
  if (skel.length) {
    grid.replaceChildren(...skel);
    grid.setAttribute("aria-busy", "true");
  }
}

// Served mode: a persistent nav bar linking every sibling dashboard (/d/<id>),
// built from window.__served.nav. No-op unless there are 2+ dashboards.
function renderServedNav(served) {
  if (!served || !Array.isArray(served.nav) || served.nav.length < 2) return;
  if (document.querySelector(".served-nav")) return;
  const nav = document.createElement("nav");
  nav.className = "served-nav";
  for (const d of served.nav) {
    const a = document.createElement("a");
    a.href = "/d/" + encodeURIComponent(d.id);
    a.textContent = d.title || d.id;
    const active = d.id === served.id;
    if (active) a.className = "active"; // styled by web/theme-base.css (.served-nav)
    nav.appendChild(a);
  }
  const dash = document.querySelector(".dash");
  if (dash && dash.parentNode) dash.parentNode.insertBefore(nav, dash);
}
// ---- lazy tabs ----
// A ::TAB / ::SUBTAB pane's panels run their queries and render only when the
// pane is first shown: the run renders the visible pane, leaves a placeholder
// (a job) for every other panel, and lazyPump() runs the jobs of a pane once
// it is shown — and, in idle time, the light ones of the hidden panes, so a
// later tab switch is instant. Setup statements inside a tab's section are
// deferred too and run, in script order, before the first panel after them.
// Jobs run one at a time on dpLazy.tail; a new run voids the old jobs (gen).
// ?lazy=0 renders everything up front; ?prefetch=0 skips the idle prefetch.
const dpLazyParams = new URLSearchParams(location.search);
const dpLazyTabs = dpLazyParams.get("lazy") !== "0";
const dpLazyPrefetch = dpLazyParams.get("prefetch") !== "0";
const dpLazy = { gen: 0, tail: Promise.resolve(), cur: null, building: null, pumping: false, wake: null };
let dpSetupRan = new Map(); // setup SQL run since the last full run -> its varSig
let dpSetupEpoch = 0; // bumped by a setup statement that may change what a query read
let dpCreated = new Set(); // objects setup created since the last full run
const dpResults = new Map(); // panel query result cache (see panelSql)

const yieldTask = () => (globalThis.scheduler?.yield ? scheduler.yield() : new Promise((r) => setTimeout(r, 0)));
// Idle time (or a wake-up: a tab was shown, its panels go first).
const idleTask = () =>
  new Promise((r) => {
    dpLazy.wake = r;
    if (typeof requestIdleCallback === "function") requestIdleCallback(() => r(), { timeout: 1000 });
    else setTimeout(r, 30);
  }).finally(() => (dpLazy.wake = null));

// Shown unless the element or a pane around it is display:none (works on the
// detached grid a run builds as well).
function paneShown(el) {
  for (let e = el; e; e = e.parentElement) if (e.style && e.style.display === "none") return false;
  return true;
}

// The values of the variables a statement reads (getvariable), as one string.
const varSig = (sql) => varsOf(sql).map((v) => `${v}=${dpVarNow[v] ?? ""}`).join("\u0001");

// Creating a new table / view / macro, or loading an extension, changes no
// result a query already returned: the cache stays valid. Anything else (a
// re-run, a replaced object, an INSERT…) starts a new epoch.
const CREATE_RE = /^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:TEMP(?:ORARY)?\s+)?(?:TABLE|VIEW|MACRO|FUNCTION|SEQUENCE|TYPE)\s+(?:IF\s+NOT\s+EXISTS\s+)?([\w."]+)/i;
async function runSetup(sql) {
  const sig = varSig(sql);
  await runSql(sql);
  const code = sqlCode(sql);
  const made = CREATE_RE.exec(code);
  const name = made && made[1].toLowerCase();
  const additive = (made && !dpCreated.has(name)) || /^\s*(INSTALL|LOAD)\b/i.test(code);
  if (name) dpCreated.add(name);
  if (!additive || dpSetupRan.has(sql)) dpSetupEpoch++;
  dpSetupRan.set(sql, sig);
}

// Run the deferred setup statements that come before statement `upto` (all of
// them for Infinity), in order. A failing one shows its error card before `at`.
async function lazyFlushSetup(L, upto, at) {
  while (L.setup.length && L.setup[0] < upto) {
    const k = L.setup.shift();
    try {
      await runSetup(L.stmts[k].sql);
    } catch (e) {
      if (isOptionalMissing(L.stmts[k], e)) console.info(`skipped (optional input missing): ${L.stmts[k].requires.join(", ")}`);
      else if (at) at.before(mkErrorCard(e, L.stmts[k].sql, true));
    }
  }
}

// A panel's query, cached per (statement, the variables it reads, setup epoch)
// — a cross-filter re-run or a tab shown again skips the queries whose inputs
// did not change. A full run clears the cache. A query that fails on a missing
// table or function while setup is still deferred runs that setup and retries.
async function panelSql(sql) {
  const key = `${dpSetupEpoch}\u0000${varSig(sql)}\u0000${sql}`;
  if (dpResults.has(key)) {
    performance.mark("dp:cache-hit");
    return dpResults.get(key);
  }
  let rows;
  try {
    rows = await runSql(sql);
  } catch (e) {
    const L = dpLazy.building;
    if (!(L && L.setup.length && /does not exist|Catalog Error/i.test(String(e)))) throw e;
    await lazyFlushSetup(L, Infinity, null);
    return panelSql(sql);
  }
  if (rows.length < 4e6) {
    dpResults.set(key, rows);
    if (dpResults.size > 400) dpResults.delete(dpResults.keys().next().value);
  }
  return rows;
}

// Run one placeholder's statement (+ the ::TRACKs under it) and swap the
// rendered panel(s) in. `inline`: the run is still building (it wires and
// animates everything itself).
async function lazyRunJob(job, inline = false) {
  const L = job.run;
  if (job.done || L.gen !== dpLazy.gen) return;
  job.done = true;
  performance.mark("dp:lazy", { detail: job.i });
  await lazyFlushSetup(L, job.i, job.ph);
  const frag = document.createDocumentFragment();
  const base = { ...job.ctx, container: frag, curGrid: job.ctx.container === job.ctx.curGrid ? frag : null };
  const tag = (i) => frag.querySelectorAll(".panel:not([data-stmt])").forEach((p) => (p.dataset.stmt = i));
  const out = await L.panelBody(job.i, L.stmts[job.i], base);
  tag(job.i);
  for (const t of job.tracks) {
    const ts = L.stmts[t.i];
    if (out.lastChart) {
      try {
        addTrack(out.lastChart, { rows: await panelSql(ts.sql), roles: ts.roles, height: t.height });
      } catch (e) {
        frag.appendChild(mkErrorCard(e, ts.sql));
      }
    } else await L.panelBody(t.i, ts, { ...base, nextSpan: 0, nextHeight: t.height, nextRowSelect: "", nextTableOpts: {}, nextIa: {}, nextEmpty: "" });
    tag(t.i);
  }
  if (L.gen !== dpLazy.gen) return;
  const made = [...frag.querySelectorAll(".panel")];
  const pane = job.ph.closest(".tabpane, .subtabpane");
  job.ph.replaceWith(frag);
  if (pane && ![...pane.querySelectorAll(".dp-lazy")].some((p) => p.closest(".tabpane, .subtabpane") === pane)) pane.removeAttribute("aria-busy");
  if (inline) return;
  // Wired at once (hover, a11y summary, toolbox…): a panel is never on the
  // page without its summary.
  const t0 = performance.now();
  fitCharts(true); // drawn at their shown width, and watched (a hidden pane: once it shows)
  wireCharts();
  addExportButtons();
  performance.measure("dp:wire", { start: t0, detail: "lazy" });
  if (L.fresh) animateIn(made);
}

// Queries that load an extension or remote files: not prefetched in idle time
// (a click elsewhere would wait for them); they run when their tab is shown.
const LAZY_HEAVY_RE = /\bST_Read\b|\bspatial\b|\bts_\w+\b|\banofox_forecast\b|m5_monthly|'data\/|\b(INSTALL|LOAD|ATTACH)\b|read_(parquet|csv|json)\w*\s*\(\s*'(https?|s3):/i;
const lazyHeavy = (L, job) => [job.i, ...L.setup.filter((k) => k < job.i)].some((k) => LAZY_HEAVY_RE.test(L.stmts[k].sql) || STATS_RE.test(L.stmts[k].sql));

async function lazyPump() {
  if (dpLazy.pumping) return;
  dpLazy.pumping = true;
  try {
    for (;;) {
      const L = dpLazy.cur;
      if (!L || runActive || L.gen !== dpLazy.gen) break;
      const open = () => L.jobs.filter((j) => !j.done && j.ph.isConnected);
      let job = open().find((j) => paneShown(j.ph));
      if (!job && dpLazyPrefetch) {
        await idleTask();
        if (runActive || L.gen !== dpLazy.gen) break;
        job = open().find((j) => paneShown(j.ph)) || open().find((j) => !lazyHeavy(L, j));
      }
      if (!job) break;
      const step = dpLazy.tail.then(() => lazyRunJob(job));
      dpLazy.tail = step.catch((e) => console.error(e));
      await dpLazy.tail;
      await yieldTask();
    }
  } finally {
    dpLazy.pumping = false;
  }
}

// Every panel rendered (HTML export): run the jobs still waiting.
async function lazyFlushAll() {
  while (runActive) await runActive;
  const L = dpLazy.cur;
  if (!L) return;
  for (const job of L.jobs) {
    const step = dpLazy.tail.then(() => lazyRunJob(job, true));
    dpLazy.tail = step.catch((e) => console.error(e));
    await dpLazy.tail;
  }
  wireCharts();
  addExportButtons();
}

// A pane was just shown: play the entrance of its panels rendered while it
// was hidden, size its scrolling tables, and render its waiting panels.
function lazyShown(pane) {
  animateIn([...pane.querySelectorAll(".panel")].filter((p) => p._dpEnter));
  requestAnimationFrame(keyboardScrollers);
  dpLazy.wake?.();
  lazyPump();
}

let dpTimer = null; // auto-refresh interval handle
let dpPrimary = null; // optional brand colour (hex, no #) for chart primary

// `fresh` = a full run (Run button / load): execute every setup statement.
// A cross-filter re-run (selecting a row, an input, clearing) passes fresh=false
// so we SKIP setup that doesn't depend on the selection — e.g. an expensive
// CREATE OR REPLACE TABLE fc AS ts_forecast_by(...) is computed once, not on
// every row click. Setup that references getvariable() still re-runs.
//
// Runs never overlap: a run requested while one is in progress (two quick
// clicks, an input changed mid-run) is queued and coalesced into ONE follow-up
// run with the latest state (a full run wins over a cross-filter re-run), so two
// half-built dashboards can never race to replace each other.
let runActive = null; // the run in progress (a promise)
// A drill or drill-through click: {drill: prefix, key, down} or {link: true},
// plus `focus` — where the focus goes if the run takes it away. The next run
// animates its replaced charts from their old marks.
let dpMotion = null;
let runQueued = null; // null, or the `fresh` flag of the queued follow-up run
function run(fresh = true) {
  if (runActive) {
    runQueued = (runQueued ?? false) || fresh;
    return runActive;
  }
  runActive = (async () => {
    try {
      dpLazy.gen++; // the old dashboard's lazy jobs are void …
      await dpLazy.tail; // … once the one in flight (a single query) settles
      await runOnce(fresh);
    } finally {
      runActive = null;
      if (runQueued !== null) {
        const f = runQueued;
        runQueued = null;
        run(f);
      }
    }
  })();
  return runActive;
}
async function runOnce(fresh = true) {
  const tRun = performance.now();
  const grid = $("grid");
  const motion = dpMotion; // what started this run (a drill, a drill-through)
  dpMotion = null;
  document.body.classList.add("loading");
  status("running…");
  // Double-buffer: build the whole dashboard off-screen, then swap it in at the
  // end. The old dashboard stays visible during the (async) rebuild, so a
  // cross-filter re-run updates in place instead of blinking empty.
  const newGrid = document.createElement("div");
  newGrid.className = "grid";
  let stmts;
  try {
    stmts = JSON.parse(plan($("sql").value));
  } catch (e) {
    grid.replaceChildren();
    return showError(grid, String(e));
  }
  markPending(grid, stmts, fresh);
  if (fresh) {
    dpSetupRan = new Map();
    dpCreated = new Set();
    dpResults.clear();
  }
  // Lazy tabs: this run's deferred setup statements and panel jobs.
  const lazy = dpLazyTabs ? { gen: dpLazy.gen, fresh, stmts, setup: [], jobs: [], panelBody: null } : null;
  dpLazy.building = lazy;
  const tabAt = []; // statement -> the ::TAB whose section it is in (-1: before the first)
  for (let i = 0, t = -1; i < stmts.length; i++) tabAt.push(directive(stmts[i]) === "TAB" ? (t = i) : t);

  await prepInteractions(stmts);
  await setSelectionVars();
  await setRangeVars();
  await setDrillVars();

  // Pre-pass: run setup + set each dropdown's DuckDB variable (before charts),
  // caching options for the render pass by statement index. Chrome directives
  // (::THEME, ::LOCALE, ::DASHBOARD_TITLE, …) are collected here so the theme
  // applies to every panel wherever the directive sits in the script.
  const dd = {};
  // Default theme: the server's (`serve --theme`, anofox_serve_dashboards
  // options.theme), else the built-in light look; a ::THEME overrides it.
  dpTheme.spec = defaultThemeSpec();
  dpTheme.chrome = {};
  dpAsof = null;
  const CHROME_KEY = { LOCALE: "locale", DASHBOARD_TITLE: "title", BRAND: "brand", LOGO: "logo", FAVICON: "favicon" };
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    try {
      if (!s.setup && chromeRole(s)) {
        const cr = chromeRole(s);
        const rows = JSON.parse(await runSql(s.sql));
        const idx = s.roles.find((r) => r[1] === cr)[0];
        let v = rows[0] ? rows[0]["c" + idx] ?? Object.values(rows[0])[0] : null;
        if (v != null && typeof v === "object") v = JSON.stringify(v);
        v = v == null ? "" : String(v);
        if (cr === "THEME") dpTheme.spec = v;
        else if (cr === "ASOF") {
          // ::ASOF: the data's timestamp (+ an optional 'stale=24h' cell)
          const opt = Object.values(rows[0] || {}).find((c) => typeof c === "string" && /^\s*stale/.test(c));
          dpAsof = { at: asofParse(rows[0] ? rows[0]["c" + idx] ?? Object.values(rows[0])[0] : null), stale: asofStale(opt) };
        } else dpTheme.chrome[CHROME_KEY[cr]] = v;
        continue;
      }
      if (s.setup) {
        // On a cross-filter re-run, skip setup whose variables did not change
        // since it last ran (selection-independent setup ran on the last full
        // run) so clicks don't recompute forecasts etc.
        if (!fresh && dpSetupRan.has(s.sql) && dpSetupRan.get(s.sql) === varSig(s.sql)) continue;
        // Inside a tab's section: runs before the first panel after it does.
        if (lazy && tabAt[i] >= 0 && !/^\s*(SET|USE|ATTACH|DETACH)\b/i.test(sqlCode(s.sql))) {
          lazy.setup.push(i);
          continue;
        }
        await runSetup(s.sql);
      } else if (isInput(s)) {
        const kind = inputKind(s);
        const rows = JSON.parse(await panelSql(s.sql));
        if (!rows.length) continue;
        // Optional ::LABEL column → the control's caption (default: the variable
        // name). A ::DATERANGE keeps its columns as they are (from, to, label).
        const lr = s.roles.find((r) => r[1] === "LABEL");
        const caption =
          kind === "DATERANGE"
            ? String(Object.values(rows[0])[2] ?? "")
            : lr
              ? String(rows[0]["c" + lr[0]] ?? "")
              : "";
        if (kind === "PERIOD") {
          // ::PERIOD: the spec string's column (its alias = the variable
          // prefix); the viewer's choice lives in dpVars[col] as an object.
          const keys = Object.keys(rows[0]);
          const col = keys[s.roles.find((r) => r[1] === "PERIOD")[0]] ?? keys[0];
          const spec = String(rows[0][col] ?? "");
          const def = periodResolve(spec, col, {});
          const prefix = def.prefix || col;
          dpPeriodUrl[col] = { prefix, def };
          if (dpVars[col] === undefined) {
            const seeded = validPeriodSeed(dpUrlSeed, prefix, (st) => periodResolve(spec, col, st));
            if (seeded) dpVars[col] = seeded;
          }
          for (const k of [prefix, prefix + "_grain", prefix + "_compare"]) delete dpUrlSeed[k];
          const st = dpVars[col];
          const res = periodResolve(spec, col, st && typeof st === "object" && !Array.isArray(st) ? st : {});
          if (!res.vars) throw new Error("::PERIOD: " + (res.errors || []).join("; "));
          dd[i] = { kind, varname: col, res, caption };
          for (const q of periodSets(res.prefix, res.vars)) await runSql(q);
          continue;
        }
        if (kind === "DATERANGE") {
          const keys = Object.keys(rows[0]);
          const fk = keys[0];
          const tk = keys[1] || keys[0];
          seedInputFromUrl(kind, [fk, tk], rows);
          if (dpVars[fk] === undefined) dpVars[fk] = String(rows[0][fk] ?? "");
          if (dpVars[tk] === undefined) dpVars[tk] = String(rows[0][tk] ?? "");
          dd[i] = { kind, varnames: [fk, tk], caption };
          await runSql(`SET VARIABLE ${fk} = '${String(dpVars[fk]).replace(/'/g, "''")}'`);
          await runSql(`SET VARIABLE ${tk} = '${String(dpVars[tk]).replace(/'/g, "''")}'`);
          continue;
        }
        const varname = Object.keys(rows[0])[0];
        seedInputFromUrl(kind, [varname], rows);
        let lit;
        if (kind === "DROPDOWN" || kind === "BUTTONS") {
          const options = rows.map((r) => String(r[varname]));
          if (dpVars[varname] === undefined || !options.includes(dpVars[varname])) dpVars[varname] = options[0];
          // Optional ::HINT column → a hint shown next to each option.
          const hr = s.roles.find((r) => r[1] === "HINT");
          const hints = hr ? rows.map((r) => String(r["c" + hr[0]] ?? "")) : null;
          dd[i] = { kind, varname, options, hints, caption };
          lit = `'${String(dpVars[varname]).replace(/'/g, "''")}'`;
        } else if (kind === "TOGGLE") {
          // A BOOLEAN variable; the query's value is the default.
          if (dpVars[varname] === undefined) dpVars[varname] = toggleValue(rows[0][varname]);
          dd[i] = { kind, varname, caption };
          lit = dpVars[varname] === "true" ? "TRUE" : "FALSE";
        } else if (kind === "MULTISELECT") {
          const options = rows.map((r) => String(r[varname]));
          if (!Array.isArray(dpVars[varname])) dpVars[varname] = options.slice(); // default: all
          dpVars[varname] = dpVars[varname].filter((v) => options.includes(v));
          dd[i] = { kind, varname, options, caption };
          lit = "[" + dpVars[varname].map((v) => `'${String(v).replace(/'/g, "''")}'`).join(",") + "]";
        } else {
          // number / date / text: the query's value is the default
          if (dpVars[varname] === undefined) dpVars[varname] = String(rows[0][varname] ?? "");
          dd[i] = { kind, varname, caption };
          const v = String(dpVars[varname]);
          lit = kind === "NUMBER" ? v || "0" : `'${v.replace(/'/g, "''")}'`; // number unquoted
        }
        await runSql(`SET VARIABLE ${varname} = ${lit}`);
      }
    } catch (e) {
      if (isOptionalMissing(s, e)) console.info(`skipped (optional input missing): ${s.requires.join(", ")}`);
      else newGrid.appendChild(mkErrorCard(e, s.sql, true));
    }
  }

  // URL seeds no input claimed are selections (`v.selected`, named ones).
  if (applySelectionSeeds()) await setSelectionVars();
  markPending(grid, stmts, fresh); // now the changed variables are known

  // The theme (and its light/dark variant) for this run: CSS tokens on the
  // page, the wasm render context for every panel.
  const themeErr = applyDashboardTheme();
  if (themeErr) showError(newGrid, themeErr);
  const head = dashHeader();
  if (head) newGrid.appendChild(head);

  // Render pass: place controls / headings / charts in document order into the
  // current container (the grid, or an open ::GROUP box). ::COLUMNS sets the
  // grid columns; ::SPAN widens the next panel.
  let container = newGrid;
  let curGrid = newGrid; // the active surface (main grid / tab pane / sub-tab pane)
  let tabBar = null;
  let tabWrap = null;
  let curPane = null; // the current top-level ::TAB pane (nesting host for ::SUBTAB)
  let curTopName = null;
  let subBar = null;
  let subWrap = null;
  let nextSpan = 0;
  let nextHeight = 0; // ::HEIGHT → the next panel's height in px (optional)
  let nextRowSelect = ""; // ::ROWSELECT → the next table's row selection ('single' | 'multi')
  let nextTableOpts = {}; // ::TOTALS / ::COLGROUP / ::FLAG / ::HIGHLIGHT / ::SUBTOTAL → the next table's options
  let nextIa = {}; // ::SELECT_AS → the next chart's interactions (see dpIa)
  let nextEmpty = ""; // ::EMPTY → the next panel's message for an empty result
  let lastChart = null; // the holder of the chart a ::TRACK right after it goes under
  let defaultSpan = Math.max(1, Math.round(12 / dpCols)); // 12-col bootstrap default
  let panels = 0;
  // Auto-pack a top-level run of >=2 consecutive KPI tiles (::METRIC/::COMPACT/
  // ::MONEY/::PERCENT, no ::TABLE) into one compact strip, so a plain dashboard
  // gets a metric band without an explicit ::GROUP. Never fires inside a
  // ::GROUP/tab that already boxes the tiles.
  const isKpi = (st) => !st.setup && !directive(st) && !isInput(st) && !!metricRole(st) && !role(st, "TABLE");
  const kpiRun = (from) => {
    let n = 0;
    for (let j = from; j < stmts.length; j++) {
      if (stmts[j].setup) continue;
      if (isKpi(stmts[j])) n++;
      else break;
    }
    return n;
  };
  let autoGroup = null; // open synthetic ::GROUP body element, or null
  const firstValue = async (s) => {
    // A literal directive (SELECT 'Map'::TAB, SELECT 6::SPAN) needs no query.
    const lit = /^\s*SELECT\s+(?:(-?\d+(?:\.\d+)?)|'((?:[^']|'')*)')\s+AS\s+c0\s*;?\s*$/i.exec(s.sql);
    if (lit) return lit[2] !== undefined ? lit[2].replace(/''/g, "'") : Number(lit[1]);
    const rows = JSON.parse(await runSql(s.sql));
    return rows[0] ? Object.values(rows[0])[0] : null;
  };
  // One panel statement (a contract plot, a ::PAGED table, or a chart / table /
  // KPI / text card…): run its query and append its panel(s) to ctx.container.
  // The render loop calls it in place; a statement inside a hidden ::TAB pane
  // runs later, as a lazy job, with the loop state it would have seen (ctx).
  // Returns the chart holder a following ::TRACK attaches to, and the count.
  const panelBody = async (i, s, ctx) => {
    let { container, curGrid, nextSpan, nextHeight, nextRowSelect, nextTableOpts, nextIa, nextEmpty } = ctx;
    const { defaultSpan, trackHost } = ctx;
    let lastChart = null;
    let panels = 0;
    try {
      if (s.contract) {
        // A contract plot call (SELECT anofox_plot_terms(t) FROM … t): the planner
        // rewrote it to return the rows (+ named args) as JSON; render each
        // result row (one per GROUP BY key) with the contract renderer.
        const rows = JSON.parse(await panelSql(s.sql));
        const span = Math.min(12, nextSpan || defaultSpan);
        const title = titleOf(s, JSON.stringify(rows.slice(0, 1)));
        const mk = (caption) => {
          const fig = document.createElement("figure");
          fig.className = "panel contract-panel";
          if (container === curGrid) fig.style.gridColumn = `span ${span}`;
          if (nextHeight) fig.style.minHeight = nextHeight + "px";
          if (caption) fig.appendChild(mkTitle(caption));
          return fig;
        };
        if (!rows.length) {
          const fig = mk(title);
          fig.appendChild(mkNoData(nextEmpty));
          container.appendChild(fig);
          panels++;
        }
        for (const r of rows) {
          const fig = mk(contractCaption(title, s.contract, r));
          const ph = nextHeight || contractHeight(s.contract.macro);
          const holder = document.createElement("div");
          holder.className = "panel-svg";
          const rw = Math.max(300, span * 100);
          const data = contractCell(r, s.contract.rows, "[]");
          const args = contractCell(r, s.contract.args, "{}");
          holder.innerHTML = render_contract(s.contract.macro, data, args, rw, ph, renderCtx());
          const warn = holder.querySelector("svg") && holder.querySelector("svg").getAttribute("data-warnings");
          if (warn) console.warn(`${s.contract.macro}: ${warn}`);
          fig.appendChild(holder);
          // Data view (toolbox) lists the rows the plot was drawn from.
          fig._dp = { rows: data, roles: [], rw, ph, isMap: false, contract: { macro: s.contract.macro, args } };
          container.appendChild(fig);
          panels++;
        }
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
      } else if (role(s, "PAGED")) {
        // A SQL-driven table: only one page of rows (Scroll view: one block at
        // a time) is fetched, plus one statistics row (count, sums, means, ranges)
        // — sorting, the search and the per-column filters all run in DuckDB, so
        // it works the same over a huge Parquet in S3 or MotherDuck. Every query
        // is the same `page` request everywhere: a locked server builds its SQL
        // with bound parameters; the browser and the authoring server run the
        // identical query from wasm page_sql().
        const span = Math.min(12, nextSpan || defaultSpan);
        const fig = document.createElement("figure");
        fig.className = "panel";
        fig._async = true; // its rows load after the swap: a re-run never keeps it
        if (container === curGrid) fig.style.gridColumn = `span ${span}`;
        const fmtByIdx = {};
        for (const [ix, r] of s.roles) if (TFMT.includes(r)) fmtByIdx[ix] = r;
        const titleRole = s.roles.find((r) => r[1] === "TITLE");
        const titleIdx = titleRole ? titleRole[0] : -1;
        const base = s.sql;
        const idx = i;
        const rowSelect = nextRowSelect;
        const tableOpts = nextTableOpts;
        const emptyMsg = nextEmpty;
        const titleHolder = document.createElement("div");
        const holder = document.createElement("div");
        fig.append(titleHolder, holder);
        const filterInput = document.createElement("input");
        filterInput.className = "tbl-search";
        filterInput.type = "search";
        filterInput.placeholder = "Search all rows…";
        filterInput.value = dpFilterText[idx] || "";
        // Every reload rebuilds the table and its toolbar, where the search box
        // lives; detaching it drops the focus. Typing in it marks it as in use,
        // so a rebuild hands the focus back (unless the user moved elsewhere).
        let lastTyped = 0;
        filterInput.addEventListener("input", () => (lastTyped = Date.now()));
        filterInput.addEventListener("focus", () => (lastTyped = Date.now())); // clicked in while a reload runs
        const st = (dpPaged[idx] ||= { colEq: {}, colQ: {}, colRange: {}, view: "pages" });
        // The column filters as request filters (op + value; values bound / escaped).
        const colFilters = () => {
          const out = [];
          for (const [col, v] of Object.entries(st.colEq)) out.push({ col, op: "eq", value: String(v) });
          for (const [col, v] of Object.entries(st.colQ)) {
            const m = String(v).match(/^([<>]=?)\s*(-?\d+(?:\.\d+)?)$/);
            if (m) out.push({ col, op: { ">": "gt", ">=": "ge", "<": "lt", "<=": "le" }[m[1]], value: m[2] });
            else out.push({ col, op: "contains", value: String(v) });
          }
          for (const [col, [lo, hi]] of Object.entries(st.colRange)) {
            if (lo != null) out.push({ col, op: "ge", value: String(lo) });
            if (hi != null) out.push({ col, op: "le", value: String(hi) });
          }
          return out;
        };
        // A newer load (page, sort, search…) supersedes the running one; so does
        // a newer Scroll-view block.
        const inst = ++servedTables;
        const pageReq = async (extra, withFilters = true, kind) => {
          const page = { ...extra };
          if (withFilters) {
            const q = (dpFilterText[idx] || "").trim();
            if (q) page.filter = q;
            const fl = colFilters();
            if (fl.length) page.filters = fl;
          }
          if (servedMode) return JSON.parse(await runSql(base, { base, ...page }, kind && kind + inst));
          const sql = page_sql(base, JSON.stringify(page));
          if (sql.startsWith("{")) throw new Error(JSON.parse(sql).error);
          return JSON.parse(await runSql(sql));
        };
        let stats = null; // the statistics row of the current filters (n = row count)
        let unfiltered = null; // the row count without search / column filters
        const sortPart = () => {
          const so = dpSort[idx];
          return so && so.col ? { sort: so.col, desc: so.dir < 0 } : {};
        };
        const load = async () => {
          const pageSize = dpPageSize[idx] || 10;
          const page = dpPage[idx] || 0;
          const so = dpSort[idx];
          const scroll = st.view === "scroll";
          let rows = [];
          try {
            rows = await pageReq({ limit: scroll ? 100 : pageSize, offset: scroll ? 0 : page * pageSize, ...sortPart() }, true, "load");
            if (!stats) {
              // the column names come from a page; a search with no hits still
              // keeps the last known columns
              const cols = rows.length ? Object.keys(rows[0]).filter((_, k) => k !== titleIdx) : st.cols || [];
              st.cols = cols;
              stats = cols.length ? (await pageReq({ stats: cols.slice(0, 64) }, true, "load"))[0] || { n: 0 } : (await pageReq({ count: true }, true, "load"))[0];
              stats._cols = cols.slice(0, 64);
            }
            const filtering = (dpFilterText[idx] || "").trim() || colFilters().length;
            if (unfiltered == null || !filtering)
              unfiltered = filtering ? Number((await pageReq({ count: true }, false, "load"))[0].n) : Number(stats.n);
          } catch (e) {
            if (isAbort(e)) return; // a newer load renders
            holder.innerHTML = "";
            if (isOptionalMissing(s, e)) holder.appendChild(mkNotAvailable(s));
            else holder.appendChild(mkErrorBody(e, base));
            fig.classList.add(isOptionalMissing(s, e) ? "dp-na" : "dp-errcard");
            return;
          }
          titleHolder.innerHTML = "";
          holder.innerHTML = "";
          if (emptyMsg && !unfiltered) {
            holder.appendChild(mkNoData(emptyMsg));
            return;
          }
          let sk = -1;
          let caption = "";
          if (titleIdx >= 0 && rows.length) {
            const tv = Object.values(rows[0])[titleIdx];
            if (tv != null) titleHolder.appendChild(mkTitle((caption = String(tv).replace(/^"|"$/g, ""))));
            sk = titleIdx;
          }
          const reload = () => {
            stats = null;
            dpPage[idx] = 0;
            load();
          };
          const server = {
            search: filterInput,
            total: Number(stats.n) || 0,
            unfiltered,
            stats,
            statCols: stats._cols,
            colState: st,
            view: st.view,
            page,
            pageSize,
            sortCol: so ? so.col : null,
            sortDir: so ? so.dir : 1,
            onPage: (p) => {
              dpPage[idx] = Math.max(0, p);
              load();
            },
            onSort: (col) => {
              const cur = dpSort[idx];
              dpSort[idx] = { col, dir: cur && cur.col === col ? -cur.dir : 1 };
              dpPage[idx] = 0;
              load();
            },
            onPageSize: (n) => {
              dpPageSize[idx] = n;
              dpPage[idx] = 0;
              load();
            },
            onView: (v) => {
              st.view = v;
              dpPage[idx] = 0;
              load();
            },
            onColFilters: reload,
            distinct: (col) => pageReq({ distinct: col }).then((rs) => rs.map((r) => r.v)),
            // the Stats strip's histograms / top values, once per filter state
            profile: () => (stats._prof ||= pageReq({ profile: stats._cols })),
            fetchRows: (offset, limit) => pageReq({ limit, offset, ...sortPart() }, true, "rows"),
          };
          holder.appendChild(renderTable(rows, sk, fmtByIdx, server, null, { select: rowSelect, caption, ...tableOpts }));
          const lost = !document.activeElement || document.activeElement === document.body;
          if (Date.now() - lastTyped < 4000 && lost) {
            filterInput.focus();
            const n = filterInput.value.length;
            filterInput.setSelectionRange(n, n);
          }
        };
        let filterTimer = null;
        filterInput.oninput = () => {
          clearTimeout(filterTimer);
          filterTimer = setTimeout(() => {
            dpFilterText[idx] = filterInput.value;
            stats = null; // the search changes the count and the statistics
            dpPage[idx] = 0;
            load();
          }, 180);
        };
        await load();
        container.appendChild(fig);
        panels++;
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
      } else {
        const rowsJson = await panelSql(s.sql);
        if (trackHost) {
          // A thin strip under the chart before it, on its x axis (same panel).
          addTrack(trackHost, { rows: rowsJson, roles: s.roles, height: nextHeight || 0 });
          return { lastChart: null, panels: 0 };
        }
        const span = Math.min(12, nextSpan || defaultSpan);
        const boxH = nextHeight;
        const made = []; // this statement's panels (an ::INFO decorates the first)
        const mkPanel = () => {
          const fig = document.createElement("figure");
          fig.className = "panel";
          made.push(fig);
          if (container === curGrid) fig.style.gridColumn = `span ${span}`;
          if (boxH) fig.style.minHeight = boxH + "px";
          return fig;
        };
        const firstCell = () => {
          const r = JSON.parse(rowsJson)[0];
          return r ? String(Object.values(r)[0] ?? "").replace(/^"|"$/g, "") : "";
        };
        if (isHeading(s) || role(s, "SECTION")) {
          // A lone ::LABEL or a ::SECTION: a full-width heading (own row).
          const rows = JSON.parse(rowsJson);
          const sr = s.roles.find((r) => r[1] === "SECTION");
          const h = document.createElement("h2");
          h.className = "section";
          const r0 = rows[0];
          h.textContent = r0 ? String((sr ? r0["c" + sr[0]] : Object.values(r0)[0]) ?? "") : "";
          container.appendChild(h);
        } else if (role(s, "RELOAD")) {
          // Auto-refresh every N seconds, driven from SQL.
          const secs = parseFloat(firstCell()) || 0;
          clearInterval(dpTimer);
          if (secs > 0) dpTimer = setInterval(run, secs * 1000);
          if ($("refresh")) $("refresh").value = [0, 5, 15, 30, 60].includes(secs) ? String(secs) : "0";
        } else if (role(s, "HEADER_IMAGE")) {
          const img = document.createElement("img");
          img.className = "header-image";
          const src = safeUrl(firstCell(), { image: true });
          if (src) {
            img.src = src;
            container.appendChild(img);
          }
        } else if (role(s, "FOOTER_LINK")) {
          const rows = JSON.parse(rowsJson);
          const vals = rows[0] ? Object.values(rows[0]).map((v) => String(v ?? "")) : [""];
          const a = document.createElement("a");
          a.className = "footer-link";
          a.href = safeUrl(vals[0].replace(/^"|"$/g, "")) || "#";
          a.textContent = (vals[1] || vals[0]).replace(/^"|"$/g, "");
          a.target = "_blank";
          a.rel = "noopener";
          container.appendChild(a);
        } else if (role(s, "DOWNLOAD_CSV") || role(s, "DOWNLOAD_XLSX") || role(s, "DOWNLOAD_PDF")) {
          const rows = JSON.parse(rowsJson);
          container.appendChild(mkDownload(s, rows));
        } else if (role(s, "MARKDOWN")) {
          const fig = mkPanel();
          fig.classList.add("md-box");
          const tr = s.roles.find((r) => r[1] === "TITLE");
          const r0 = JSON.parse(rowsJson)[0] || {};
          if (tr && r0["c" + tr[0]]) fig.appendChild(mkTitle(String(r0["c" + tr[0]])));
          const mdr = s.roles.find((r) => r[1] === "MARKDOWN");
          // ::CALLOUT 'note' | 'warning' | 'success': a tinted box with an icon.
          const cr = s.roles.find((r) => r[1] === "CALLOUT");
          if (cr) {
            const kind = calloutKind(r0["c" + cr[0]]);
            fig.classList.add("dp-callout", "dp-callout-" + kind);
            fig.setAttribute("role", "note"); // static text: never an assertive alert
            fig.dataset.icon = { note: "ℹ", warning: "⚠", success: "✓" }[kind];
          }
          const body = document.createElement("div");
          body.className = "md-body";
          body.innerHTML = markdownWithValues(mdr ? r0["c" + mdr[0]] : firstCell(), r0, mdValue);
          typesetMath(body);
          fig.appendChild(body);
          container.appendChild(fig);
          panels++;
        } else if (textSizeOf(s)) {
          const fig = mkPanel();
          fig.classList.add("textcard", "text-" + textSizeOf(s));
          const lr = s.roles.find((r) => r[1] === "LABEL");
          const r0 = JSON.parse(rowsJson)[0] || {};
          fig.innerHTML =
            `<div class="text-value">${escapeHtml(firstCell())}</div>` +
            (lr ? `<div class="metric-cap">${escapeHtml(r0["c" + lr[0]])}</div>` : "");
          container.appendChild(fig);
          panels++;
        } else if (role(s, "TABLE")) {
          let rows = JSON.parse(rowsJson);
          const fig = mkPanel();
          // An ::INFO column is the ⓘ text, not a cell: drop it (by key).
          let troles = s.roles;
          const infoR = troles.find((r) => r[1] === "INFO");
          if (infoR && rows.length) {
            const k = Object.keys(rows[0])[infoR[0]];
            const shift = (ri) => (ri[0] > infoR[0] ? [ri[0] - 1, ri[1]] : ri);
            rows = rows.map(({ [k]: _, ...rest }) => rest);
            troles = troles.filter((r) => r !== infoR).map(shift);
          }
          const tr = troles.find((r) => r[1] === "TITLE");
          let skip = -1;
          let caption = "";
          if (tr && rows.length) {
            skip = tr[0];
            const tv = Object.values(rows[0])[skip];
            if (tv != null) fig.appendChild(mkTitle((caption = String(tv).replace(/^"|"$/g, ""))));
          }
          // Per-column formatting (::MONEY/::PERCENT/::COMPACT/::METRIC number
          // formats, ::TREND arrows, ::COLORSCALE heatmap cells, ::BADGE pills,
          // ::SPARKLINE mini charts), keyed by output column index.
          const fmtByIdx = {};
          for (const [idx, r] of troles) if (TFMT.includes(r)) fmtByIdx[idx] = r;
          if (!rows.length && nextEmpty) fig.appendChild(mkNoData(nextEmpty));
          else fig.appendChild(renderTable(rows, skip, fmtByIdx, null, i, { select: nextRowSelect, caption, ...nextTableOpts }));
          container.appendChild(fig);
          panels++;
        } else if (metricRole(s)) {
          const r0 = JSON.parse(rowsJson)[0] || {};
          const mr = metricRole(s);
          const lr = s.roles.find((r) => r[1] === "LABEL");
          // ::DELTA: up is good; ::DELTA_LOWER_GOOD: down is good (churn, cost,
          // errors). ::DELTA_FORMAT / ::DELTA_LABEL shape the change line;
          // ::SPARKLINE (a list) draws a trend, ::GOAL/::TARGET a progress bar.
          // The texts come from Rust (wasm kpi_parts = the headless tile).
          const dr = s.roles.find((r) => r[1] === "DELTA" || r[1] === "DELTA_LOWER_GOOD");
          const cell = (tok) => {
            const r = s.roles.find((x) => x[1] === tok);
            return r ? r0["c" + r[0]] : null;
          };
          const fig = mkPanel();
          fig.classList.add("metric");
          let parts = {};
          try {
            parts = JSON.parse(
              kpi_parts(
                JSON.stringify({
                  value: r0["c" + mr[0]],
                  prev: dr ? r0["c" + dr[0]] : null,
                  goal: cell("REFLINE"),
                  fmt: mr[1],
                  delta_format: String(cell("DELTA_FORMAT") ?? ""),
                  lower_good: !!dr && dr[1] === "DELTA_LOWER_GOOD",
                  locale: dashLocale() || "",
                }),
              ),
            );
          } catch (_) {}
          let deltaHtml = "";
          const d = parts.delta;
          if (d) {
            const lab = cell("DELTA_LABEL");
            deltaHtml =
              `<div class="metric-delta ${d.good ? "good" : "bad"}" title="${d.good ? "better" : "worse"} than the comparison">` +
              `<span class="metric-delta-chg">${d.arrow} ${escapeHtml(d.text)}</span>` +
              (lab != null && String(lab).trim() ? `<span class="metric-delta-label">${escapeHtml(lab)}</span>` : "") +
              `</div>`;
          }
          const g = parts.goal;
          const goalHtml = g
            ? `<div class="metric-goal${g.reached ? " reached" : ""}"><div class="metric-goal-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(g.frac * 100)}" aria-label="${escapeHtml(g.text)}">` +
              `<span style="width:${Math.max(0, Math.min(1, g.frac)) * 100}%"></span></div><div class="metric-goal-text">${escapeHtml(g.text)}</div></div>`
            : "";
          // ::THRESHOLDS: the value takes its status colour (good / warn / bad)
          const thr = s.roles.find((r) => r[1] === "THRESHOLDS");
          const st = thr && thresholdRule((parseThresholds(r0["c" + thr[0]])[0] || {}).rules, r0["c" + mr[0]]);
          const stAttr = st ? ` th-${st.status}" title="${escapeHtml(`${st.status}: ${st.text}`)}` : "";
          fig.innerHTML =
            `<div class="metric-value${stAttr}">${escapeHtml(fmtNum(r0["c" + mr[0]], mr[1]))}</div>` +
            deltaHtml +
            `<div class="metric-cap">${escapeHtml(lr ? r0["c" + lr[0]] : "")}</div>` +
            kpiSpark(cell("SPARKLINE")) +
            goalHtml;
          container.appendChild(fig);
          panels++;
        } else {
          const fig = mkPanel();
          // Interactions (::SELECT_AS / ::DRILL before it, a ::LINK column) —
          // set before the no-data check so a drilled-empty chart keeps its breadcrumb.
          const lr = s.roles.find((r) => r[1] === "LINK");
          fig._ia = lr ? { ...nextIa, link: { idx: lr[0], rows: rowsJson, roles: s.roles } } : nextIa;
          const t = titleOf(s, rowsJson);
          if (t) fig.appendChild(mkTitle(t));
          // The drill breadcrumb is part of the panel from the start: added only
          // after the swap, the chart would sit one row higher for a layout,
          // and scroll anchoring would move the page.
          if (fig._ia.drill) fig.appendChild(drillCrumbs(fig._ia.drill));
          // A selection that filters everything out renders a clean note, not a
          // broken/empty chart box.
          if (!JSON.parse(rowsJson).length) {
            fig.appendChild(mkNoData(nextEmpty));
          } else {
            const ph = boxH || (role(s, "SPARKLINE") ? 90 : 300); // ::HEIGHT, else default
            const isMap = s.roles.some((r) => r[1] === "MAP");
            const holder = document.createElement("div");
            holder.className = "panel-svg";
            // Render width proportional to the panel's span, so displayed height
            // (= panel width × height / render width) is the same for any span at
            // a given ::HEIGHT — a full-width and a 1/3-width chart line up.
            const rw = isMap || role(s, "SPARKLINE") ? 460 : Math.max(300, span * 100);
            renderChart(holder, rowsJson, s.roles, rw, ph);
            // ggplot-rs build warnings (dropped rows, skipped layers) ride on the
            // SVG root as data-warnings — surface them for debugging.
            const warn = holder.querySelector("svg") && holder.querySelector("svg").getAttribute("data-warnings");
            if (warn) console.warn(`panel ${t ? `"${t}"` : s.sql.slice(0, 60)}: ${warn}`);
            fig.appendChild(holder);
            // Stash the panel's data/roles so the toolbox (data view, chart-type
            // toggle) can reach them without re-querying.
            fig._dp = { rows: rowsJson, roles: s.roles, rw, ph, isMap };
            // Maps + continuous cartesian charts are scroll-to-zoom / drag-to-pan
            // (double-click resets).
            if (isMap) attachMapZoom(holder, rowsJson, s.roles, ph);
            // Chart zoom/pan is OFF by default; enable per-view with ?zoom=1.
            else if (document.body.classList.contains("dp-zoom"))
              attachCartZoom(holder, rowsJson, s.roles, ph); // self-skips discrete x
            if (!isMap) lastChart = holder;
          }
          container.appendChild(fig);
          panels++;
        }
        const ir = s.roles.find((r) => r[1] === "INFO");
        if (ir && made.length) addInfo(made[0], cellAt(rowsJson, ir[0]));
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
      }
    } catch (e) {
      // A failing panel becomes a card in its own grid slot — the others keep
      // rendering. An optional input (`-- @requires`) that is missing reads
      // "Not available" instead.
      const card = isOptionalMissing(s, e) ? mkNotAvailable(s) : mkErrorCard(e, s.sql);
      if (container === curGrid) card.style.gridColumn = `span ${Math.min(12, nextSpan || defaultSpan)}`;
      container.appendChild(card);
    }
    return { lastChart, panels };
  };
  if (lazy) lazy.panelBody = panelBody;
  let lastYield = performance.now();
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    if (s.setup) continue;
    const d = directive(s);
    // A ::TRACK (optionally after a ::HEIGHT) follows its chart directly.
    const trackHost = role(s, "TRACK") ? lastChart : null;
    if (d !== "HEIGHT" && !role(s, "TRACK")) lastChart = null;
    // Close an open auto-strip the moment a non-KPI statement appears; open one
    // when a fresh top-level run of >=2 KPI tiles begins.
    if (autoGroup && !isKpi(s)) {
      container = curGrid;
      autoGroup = null;
    }
    if (!autoGroup && container === curGrid && isKpi(s) && kpiRun(i) >= 2) {
      const box = document.createElement("section");
      box.className = "group";
      const body = document.createElement("div");
      body.className = "group-body";
      box.appendChild(body);
      curGrid.appendChild(box);
      container = body;
      autoGroup = body;
    }
    try {
      if (chromeRole(s)) {
        // collected in the pre-pass
      } else if (d === "COLUMNS") {
        const n = parseInt(await firstValue(s));
        if (n > 0) {
          dpCols = n;
          defaultSpan = Math.max(1, Math.round(12 / n));
        }
      } else if (d === "GROUP") {
        const title = await firstValue(s);
        const box = document.createElement("section");
        box.className = "group";
        if (title) {
          const t = document.createElement("div");
          t.className = "group-title";
          t.textContent = title;
          box.appendChild(t);
        }
        const body = document.createElement("div");
        body.className = "group-body";
        box.appendChild(body);
        curGrid.appendChild(box);
        container = body;
      } else if (d === "ENDGROUP") {
        container = curGrid;
      } else if (d === "SPAN") {
        nextSpan = parseInt(await firstValue(s)) || 0;
      } else if (d === "HEIGHT") {
        nextHeight = parseInt(await firstValue(s)) || 0;
      } else if (d === "ROWSELECT") {
        nextRowSelect = String((await firstValue(s)) ?? "")
          .replace(/^"|"$/g, "")
          .toLowerCase();
      } else if (d === "EMPTY") {
        nextEmpty = String((await firstValue(s)) ?? "").replace(/^"|"$/g, "");
      } else if (d === "SELECT_AS" || d === "DRILL") {
        Object.assign(nextIa, dpIa[i] || {});
      } else if (["TOTALS", "COLGROUP", "FLAG", "HIGHLIGHT", "SUBTOTAL", "THRESHOLDS"].includes(d)) {
        nextTableOpts[d.toLowerCase()] = String((await firstValue(s)) ?? "").replace(/^"|"$/g, "");
      } else if (d === "TAB") {
        const name = String((await firstValue(s)) ?? "Tab");
        if (!tabBar) {
          tabBar = document.createElement("div");
          tabBar.className = "tabbar";
          tabWrap = document.createElement("div");
          tabWrap.className = "tabwrap";
        }
        const pane = document.createElement("div");
        pane.className = "grid tabpane";
        pane.style.display = "none";
        tabWrap.appendChild(pane);
        const btn = document.createElement("button");
        btn.className = "tab-btn";
        btn.textContent = name;
        btn.onclick = (e) => {
          e.stopPropagation();
          dpTab = name; // remember, so a cross-filter re-run keeps this tab
          setTabParam(name); // deep-link: reflect the active tab in the URL
          tabWrap.querySelectorAll(".tabpane").forEach((p) => (p.style.display = "none"));
          tabBar.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
          pane.style.display = "";
          btn.classList.add("active");
          lazyShown(pane);
        };
        tabBar.appendChild(btn);
        if (dpTab === name || (dpTab === null && tabBar.children.length === 1)) {
          pane.style.display = "";
          btn.classList.add("active");
        }
        curGrid = pane;
        container = pane;
        curPane = pane; // nesting host for ::SUBTAB
        curTopName = name;
        subBar = null; // start fresh sub-tabs for this top tab
        subWrap = null;
      } else if (d === "SUBTAB") {
        const name = String((await firstValue(s)) ?? "Tab");
        const host = curPane || curGrid; // nest inside the current top-level tab
        if (!subBar) {
          subBar = document.createElement("div");
          subBar.className = "subtabbar";
          subWrap = document.createElement("div");
          subWrap.className = "subtabwrap";
          host.append(subBar, subWrap);
        }
        const spane = document.createElement("div");
        spane.className = "grid subtabpane";
        spane.style.display = "none";
        subWrap.appendChild(spane);
        const sbtn = document.createElement("button");
        sbtn.className = "tab-btn subtab-btn";
        sbtn.textContent = name;
        const topName = curTopName || "";
        // Capture THIS sub-tab's bar/wrap — `subBar`/`subWrap` are shared loop
        // variables that get reassigned for later top tabs, so the closure must
        // not read them directly (that made a sub-tab operate on the last tab's
        // bar: siblings stayed active + other tabs went blank).
        const myBar = subBar;
        const myWrap = subWrap;
        sbtn.onclick = (e) => {
          e.stopPropagation();
          dpSubTab[topName] = name;
          myWrap.querySelectorAll(".subtabpane").forEach((p) => (p.style.display = "none"));
          myBar.querySelectorAll(".subtab-btn").forEach((b) => b.classList.remove("active"));
          spane.style.display = "";
          sbtn.classList.add("active");
          lazyShown(spane);
        };
        subBar.appendChild(sbtn);
        const activeSub = dpSubTab[topName];
        if (activeSub === name || (activeSub == null && subBar.children.length === 1)) {
          spane.style.display = "";
          sbtn.classList.add("active");
        }
        curGrid = spane;
        container = spane;
      } else if (d === "PLACEHOLDER") {
        const span = Math.min(12, nextSpan || defaultSpan);
        const ph = document.createElement("div");
        ph.className = "panel placeholder";
        if (container === curGrid) ph.style.gridColumn = `span ${span}`;
        container.appendChild(ph);
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
      } else if (isInput(s)) {
        if (dd[i]) container.appendChild(makeControl(dd[i], container === curGrid));
      } else if (trackHost && trackHost.lazy) {
        // a ::TRACK under a chart that renders later: it renders with it
        trackHost.lazy.tracks.push({ i, height: nextHeight || 0 });
        nextHeight = 0;
        continue;
      } else if (lazy && curGrid !== newGrid && !role(s, "RELOAD")) {
        // Inside a ::TAB / ::SUBTAB pane: a placeholder now; the query runs and
        // the panel renders when the pane is shown (lazyRun).
        const span = Math.min(12, nextSpan || defaultSpan);
        const ph = document.createElement("div");
        const kpi = !!metricRole(s) && !role(s, "TABLE");
        ph.className = "panel dp-lazy" + (kpi ? " dp-skel-kpi" : "");
        ph.setAttribute("aria-hidden", "true");
        if (container === curGrid) ph.style.gridColumn = `span ${span}`;
        if (nextHeight) ph.style.minHeight = nextHeight + "px";
        ph.innerHTML = "<i></i><i></i><i></i>";
        ph._async = true; // a re-run's patch (patchSlots) never keeps an old placeholder in its place
        container.appendChild(ph);
        curGrid.setAttribute("aria-busy", "true");
        const job = { i, ph, run: lazy, tracks: [], ctx: { container, curGrid, nextSpan, nextHeight, nextRowSelect, nextTableOpts, nextIa, nextEmpty, defaultSpan, trackHost: null } };
        lazy.jobs.push(job);
        lastChart = { lazy: job };
        panels++;
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
      } else {
        const out = await panelBody(i, s, { container, curGrid, nextSpan, nextHeight, nextRowSelect, nextTableOpts, nextIa, nextEmpty, defaultSpan, trackHost });
        if (out.lastChart) lastChart = out.lastChart;
        panels += out.panels;
        nextSpan = 0;
        nextHeight = 0;
        nextRowSelect = "";
        nextTableOpts = {};
        nextIa = {};
        nextEmpty = "";
        if (performance.now() - lastYield > 40) {
          await yieldTask(); // keep the page responsive between panels
          lastYield = performance.now();
        }
      }
    } catch (e) {
      // A failing panel becomes a card in its own grid slot — the others keep
      // rendering. An optional input (`-- @requires`) that is missing reads
      // "Not available" instead.
      const card = isOptionalMissing(s, e) ? mkNotAvailable(s) : mkErrorCard(e, s.sql);
      if (container === curGrid) card.style.gridColumn = `span ${Math.min(12, nextSpan || defaultSpan)}`;
      container.appendChild(card);
      nextSpan = 0;
      nextHeight = 0;
      nextRowSelect = "";
      nextTableOpts = {};
      nextIa = {};
      nextEmpty = "";
    } finally {
      // Tag this statement's panels (markPending finds them on the next run).
      for (const root of [newGrid, tabWrap])
        root?.querySelectorAll(".panel:not([data-stmt])").forEach((p) => (p.dataset.stmt = i));
    }
  }
  // If the remembered tab no longer exists, fall back to the first.
  if (tabBar && !tabBar.querySelector(".tab-btn.active")) {
    tabWrap.querySelector(".tabpane").style.display = "";
    tabBar.querySelector(".tab-btn").classList.add("active");
  }
  // Same fallback for nested sub-tabs: a remembered sub-tab that no longer
  // exists (edited SQL, or a stale name carried from another dashboard) would
  // otherwise leave the sub-pane blank. Activate the first sub-tab in that case.
  (tabWrap || newGrid).querySelectorAll(".subtabbar").forEach((bar) => {
    if (bar.querySelector(".subtab-btn.active")) return;
    const firstBtn = bar.querySelector(".subtab-btn");
    const wrap = bar.nextElementSibling; // subtabwrap follows subtabbar
    const firstPane = wrap && wrap.querySelector(".subtabpane");
    if (firstBtn) firstBtn.classList.add("active");
    if (firstPane) firstPane.style.display = "";
  });
  // Lazy tabs: render the panels of the panes on show now (off-screen, like
  // the rest); the hidden panes' jobs wait for lazyPump().
  if (lazy) {
    for (const job of lazy.jobs) {
      if (!paneShown(job.ph)) continue;
      await lazyRunJob(job, true);
      if (performance.now() - lastYield > 40) {
        await yieldTask();
        lastYield = performance.now();
      }
    }
    lazy.jobs = lazy.jobs.filter((j) => !j.done);
  }
  // A re-run keeps each chart's ::STORY step (a full run starts at the overview).
  if (fresh) dpStorySteps = {};
  else
    document.querySelectorAll(".dash .panel[data-stmt]").forEach((p) => {
      if (p._story) dpStorySteps[p.dataset.stmt] = p._story.step();
    });
  // A chart (or one of its marks) holding the keyboard focus hands it to its
  // replacement — however the run started (a click, an input, ::REFRESH).
  const ae = document.activeElement;
  const afig = ae?.closest?.(".dash .panel[data-stmt]");
  if (afig && ae.closest(".panel-svg > svg"))
    dpA11yFocus = ae.matches(".dp-hit") ? markKey(afig, ae) : { stmt: afig.dataset.stmt, chart: true };
  // Swap in the new dashboard in one synchronous step (no flash). A re-run of
  // the same layout patches it in place: a panel whose output is unchanged
  // keeps its DOM (scroll, focus, hover state), only the changed ones and the
  // controls are replaced. Anything else replaces the whole content.
  const dash = document.querySelector(".dash");
  const newRoots = [newGrid, tabBar, tabWrap];
  for (const p of slotsOf(newRoots)) if (p.matches(".panel")) p._sig = p.outerHTML;
  const shape = shapeOf(newRoots);
  const swaps = []; // [old, new] panels this run replaced
  const oldRoots = [grid, dash.querySelector(":scope > .tabbar"), dash.querySelector(":scope > .tabwrap")];
  if (fresh || dash._shape !== shape || !patchSlots(oldRoots, newRoots, swaps)) {
    const old = new Map();
    for (const p of document.querySelectorAll(".dash .panel[data-stmt]")) if (!old.has(p.dataset.stmt)) old.set(p.dataset.stmt, p);
    grid.replaceChildren(...newGrid.childNodes);
    dash.querySelectorAll(".tabbar,.tabwrap").forEach((e) => e.remove());
    if (tabBar) dash.append(tabBar, tabWrap);
    if (!fresh) for (const p of slotsOf(newRoots)) if (old.has(p.dataset.stmt)) swaps.push([old.get(p.dataset.stmt), p]);
  }
  dash._shape = shape;
  performance.mark("dp:swap");
  dpLazy.cur = lazy;
  // A replaced chart moves from its old marks: its 'tween'::TRANSITION, and
  // every chart on a drill / drill-through (the drilled one zooms).
  const tweens = [];
  for (const [o, n] of swaps) {
    const svg = n.querySelector(".panel-svg > svg");
    const was = o.querySelector(".panel-svg > svg");
    if (!svg || !was || (!motion && svg.getAttribute("data-transition") !== "tween")) continue;
    const drill = motion && motion.drill && n._ia && n._ia.drill && n._ia.drill.prefix === motion.drill ? motion : null;
    tweens.push([svg, dpAnim.geom(was), drill]);
  }
  // Accessibility: swap every chart for its data table (state survives runs).
  if (!dash.querySelector(".dp-a11y-bar")) {
    const bar = document.createElement("div");
    bar.className = "dp-a11y-bar";
    const b = document.createElement("button");
    b.type = "button";
    b.className = "dp-astable-btn";
    b.textContent = "▦ Charts as tables";
    b.title = "Show every chart as its data table";
    b.setAttribute("aria-pressed", String(dpAsTables));
    b.onclick = toggleAsTables;
    bar.appendChild(b);
    dash.prepend(bar);
  }
  // A "Powered by DataZoo GmbH" credit at the foot of every dashboard.
  dash.querySelector(".dp-credit")?.remove();
  const credit = document.createElement("div");
  credit.className = "dp-credit";
  const creditLink = document.createElement("a");
  creditLink.href = "https://data-zoo.de";
  creditLink.target = "_blank";
  creditLink.rel = "noopener noreferrer";
  creditLink.textContent = "DataZoo GmbH";
  credit.append("Powered by ", creditLink);
  dash.append(credit);
  fitCharts(true);
  const tWire = performance.now();
  wireCharts();
  performance.measure("dp:wire", { start: tWire });
  for (const [svg, old, drill] of tweens) drill ? dpAnim.zoom(svg, old, drill.key, drill.down) : dpAnim.tween(svg, old);
  // A drill / drill-through hands the focus on (a crumb, the opened tab) when
  // the run removed the element that had it.
  if (motion && motion.focus && (!document.activeElement || document.activeElement === document.body)) motion.focus();
  dash.querySelectorAll(".dp-switching").forEach((e) => e.classList.remove("dp-switching"));
  addExportButtons();
  syncUrlState();
  if (dpSeedRerun) {
    dpSeedRerun = false;
    runQueued = runQueued ?? false;
  }
  if (fresh) animateIn(); // entrance animation on load/Run only — cross-filter stays instant
  status(`${panels} panel${panels === 1 ? "" : "s"}`);
  grid.removeAttribute("aria-busy");
  dpVarLast = { ...dpVarNow };
  document.body.classList.remove("loading");
  performance.measure("dp:run", { start: tRun, detail: fresh ? "fresh" : "rerun" });
  if (lazy && lazy.jobs.length) setTimeout(lazyPump); // after the queued follow-up run, if any
}

// Entrance animation on a fresh render: panels fade+rise in and each chart
// plays its ::ANIMATE mode (web/anim.js — only data marks move, never grid
// lines). Skipped for cross-filter re-runs so interactions feel instant;
// prefers-reduced-motion shows the final state at once.
// A panel in a hidden tab pane plays its entrance when the pane is shown
// (lazyShown); `panels` defaults to every panel on the page.
function animateIn(panels = document.querySelectorAll(".dash .panel")) {
  if (dpAnim.still()) return;
  const go = [];
  panels.forEach((panel) => {
    if (panel.classList.contains("dp-lazy")) return;
    panel._dpEnter = !paneShown(panel);
    if (panel._dpEnter) return;
    const svg = panel.querySelector(".panel-svg > svg");
    if (svg && svg.getAttribute("data-anim") === "none") return; // ::ANIMATE 'none' opts out entirely
    panel.classList.remove("dp-in");
    go.push([panel, svg]);
  });
  if (!go.length) return;
  void document.body.offsetWidth; // one reflow, so the animations restart
  for (const [panel, svg] of go) {
    panel.classList.add("dp-in");
    if (svg) dpAnim.enter(svg, svg.getAttribute("data-anim"));
  }
}

// Render a chart's SVG into its holder — the dashboard run and every in-place
// re-render (toolbox chart-type switch, zoom/pan) — with its entrance mode.
// A chart with ::TRACKs (holder._tracks) renders as one stack, so the strips
// keep its plot area and x window. Follow with wireCharts() once the holder
// is in the page.
function renderChart(holder, rows, roles, w, ph, zoom = "") {
  const t0 = performance.now();
  const tracks = holder._tracks || [];
  if (tracks.length) {
    let svgs = [];
    try {
      svgs = JSON.parse(render_stack(JSON.stringify({ chart: { rows, roles }, tracks }), w, ph, renderCtx(), zoom));
    } catch (_) {}
    holder.innerHTML = svgs[0] || "";
    if (!holder._trackBox) {
      holder._trackBox = document.createElement("div");
      holder._trackBox.className = "panel-tracks";
      holder.after(holder._trackBox);
    }
    holder._trackBox.innerHTML = svgs.slice(1).join("");
    holder._trackBox.querySelectorAll(":scope > svg").forEach((svg, k) => (svg._track = tracks[k]));
  } else holder.innerHTML = render_panel(rows, JSON.stringify(roles), w, ph, renderCtx(), zoom);
  holder._zoom = zoom; // a re-fit to a new width keeps the zoom window
  // A ::DIAGRAM on a phone keeps a readable size: drawn at most ~1.4× smaller
  // than its own size (data-fit), its panel scrolls sideways (theme-base.css).
  const dg = holder.querySelector(":scope > svg[data-fit]");
  if (dg && matchMedia("(max-width: 640px)").matches) {
    const want = Math.round(+dg.getAttribute("data-fit") * 0.72);
    if (want > w + 1) {
      holder.innerHTML = render_panel(rows, JSON.stringify(roles), want, ph, renderCtx(), zoom);
      const wide = holder.querySelector(":scope > svg");
      if (wide) wide.style.minWidth = want + "px";
    }
    const now = holder.querySelector(":scope > svg");
    if (now) now._dpAskW = w; // fitCharts compares against the width it asked for
  }
  markAnim(holder, roles);
  performance.measure("dp:render", { start: t0 });
}

// ::TRACK: add a strip under the chart in `holder` and redraw the stack.
function addTrack(holder, track) {
  (holder._tracks = holder._tracks || []).push(track);
  const dp = holder.closest(".panel")?._dp;
  if (dp) renderChart(holder, dp.rows, dp.drawn || dp.roles, fitWidth(holder, dp.rw), dp.ph);
}

// A track's name (its ::TRACK value), for summaries and table captions.
function trackName(track) {
  const r = track && track.roles.find((x) => x[1] === "TRACK");
  const row = r && (parseRowsLenient(track.rows) || [])[0];
  return row ? String(row["c" + r[0]] ?? "") : "";
}

// A click on a track mark shades its x span on the chart above (again: off) —
// "did the alarm precede the stockout?". The stack shares the plot area, so
// the span's SVG x is the same in both.
function toggleTrackShade(el) {
  const main = el.closest(".panel")?.querySelector(".panel-svg > svg");
  const plot = ((main && main.getAttribute("data-plot")) || "").split(/\s+/).map(Number);
  if (!main || plot.length !== 4) return;
  let x = +el.getAttribute("x"), w = +el.getAttribute("width");
  if (el.tagName === "circle") (w = 6), (x = +el.getAttribute("cx") - 3);
  if (!Number.isFinite(x) || !Number.isFinite(w)) return;
  const key = `${x.toFixed(1)}:${w.toFixed(1)}`;
  const old = main.querySelector(`rect.dp-track-shade[data-key="${key}"]`);
  if (old) return old.remove();
  const r = document.createElementNS("http://www.w3.org/2000/svg", "rect");
  for (const [k, v] of [["x", x], ["y", plot[1]], ["width", Math.max(w, 2)], ["height", plot[3]], ["fill", el.getAttribute("fill") || axToken("accent", "#888")], ["fill-opacity", "0.18"], ["data-key", key], ["aria-hidden", "true"]])
    r.setAttribute(k, v);
  r.setAttribute("class", "dp-track-shade");
  r.style.pointerEvents = "none";
  main.appendChild(r);
}

// The width a chart is drawn at. Desktop: its render width (span × 100), so
// charts of any span line up at a given ::HEIGHT. Phone layout (panels stacked
// full width, the theme's max-width:760px rule) and touch screens (a phone held
// sideways, a tablet): the width it is shown at, so its text keeps its size
// instead of shrinking with the SVG.
const FIT_TO_PANEL = "(max-width: 760px), (hover: none)";
function fitWidth(holder, rw) {
  const w = Math.round(holder.clientWidth);
  return w > 0 && matchMedia(FIT_TO_PANEL).matches ? Math.max(240, w) : rw;
}

// Redraw every chart whose fitted width changed (after a run, and debounced on
// resize / orientation change / a tab coming into view), and re-fold the
// toolboxes (foldTools). True when a chart was redrawn: follow with
// wireCharts(). `watch` (after a run) re-points the resize observer at the
// fresh charts.
let dpFitObs = null,
  dpFitTimer = 0;
function fitCharts(watch = false) {
  let changed = false;
  if (watch) dpFitObs?.disconnect();
  document.querySelectorAll(".dash .panel").forEach((panel) => {
    foldTools(panel);
    const dp = panel._dp;
    const holder = panel.querySelector(":scope > .panel-svg");
    if (!dp || !holder) return;
    if (watch) dpFitObs?.observe(holder);
    const svg = holder.querySelector(":scope > svg");
    const w = fitWidth(holder, dp.rw);
    // hidden (a tab not shown, charts as tables): fitted once it shows
    if (!svg || !holder.clientWidth || Math.abs(w - (svg._dpAskW ?? svg.viewBox.baseVal.width)) < 2) return;
    if (dp.contract) holder.innerHTML = render_contract(dp.contract.macro, dp.rows, dp.contract.args, w, dp.ph, renderCtx());
    else renderChart(holder, dp.rows, dp.drawn || dp.roles, w, dp.ph, holder._zoom || "");
    changed = true;
  });
  return changed;
}
// Fold a panel's toolbox behind its "⋯" button (.dp-narrow) where the full
// bar would cover the title: narrow panels, touch screens (no hover to hide
// it), and a title long enough to run under it. Elsewhere it stays the
// hover-only bar.
function foldTools(panel) {
  const bar = panel.querySelector(":scope > .dp-toolbox");
  if (!bar || !panel.clientWidth) return;
  let fold = matchMedia("(hover: none)").matches || panel.clientWidth < 480;
  const title = !fold && panel.querySelector(":scope > .panel-title");
  if (title) {
    panel.classList.remove("dp-narrow"); // measure the unfolded bar and title
    const b = bar.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(title);
    fold = [...range.getClientRects()].some((t) => t.right > b.left && t.top < b.bottom && t.bottom > b.top);
  }
  panel.classList.toggle("dp-narrow", fold);
}

if (typeof ResizeObserver !== "undefined")
  dpFitObs = new ResizeObserver(() => {
    clearTimeout(dpFitTimer);
    dpFitTimer = setTimeout(() => fitCharts() && wireCharts(), 150);
  });

// Wire freshly rendered charts: hover, selection, toolbox and accessibility
// (attachHover), then the ::STORY steppers.
function wireCharts() {
  attachHover();
  mountStories();
}

// ::STORY steppers (web/anim.js) under every chart that has steps. A chart
// re-rendered in place keeps its step; one still showing its stepper's caption
// is left as it is.
let dpStorySteps = {}; // statement index → the step to restore after a re-run
function mountStories() {
  document.querySelectorAll(".dash .panel").forEach((panel) => {
    const svg = panel.querySelector(".panel-svg > svg[data-story]");
    if (!svg) {
      if (panel._story) panel.querySelectorAll(".dp-story, .dp-story-cap, .dp-story-link").forEach((e) => e.remove());
      panel._story = null;
      return;
    }
    if (panel._story && svg.parentElement.querySelector(":scope > .dp-story-cap")) return;
    const step = panel._story ? panel._story.step() : dpStorySteps[panel.dataset.stmt] || 0;
    panel._story = dpAnim.story(panel, svg, step);
  });
}

// The entrance a chart plays, on its root <svg> as `data-anim` (the static
// export replays it from there): its ::ANIMATE, else by chart kind — lines
// draw, bars grow, points pop; other kinds only fade in with their panel.
function markAnim(holder, roles) {
  const svg = holder.querySelector(":scope > svg");
  if (!svg) return;
  let mode = svg.getAttribute("data-animate") || "auto";
  // a ::DIAGRAM plays its own entrance (stage by stage) for any mode but 'none'
  if (svg.hasAttribute("data-diagram") && mode !== "none") mode = "draw";
  if (mode === "auto") {
    const kinds = roles.map((r) => r[1]);
    if (kinds.some((k) => /^(LINECHART|LINECHART_PERCENT|STEP|SMOOTH|AREACHART|AREACHART_STACKED|DENSITY)$/.test(k)))
      mode = "draw";
    else if (kinds.some((k) => /^(BARCHART.*|HISTOGRAM|WATERFALL|SCATTER|BUBBLE|JITTER|QQ)$/.test(k))) mode = "grow";
    else mode = "fade";
  }
  svg.setAttribute("data-anim", mode);
}

// A labelled <select>; changing it re-runs the dashboard. `bar` wraps a
// stand-alone control in its own spanning row (grouped ones sit inline).
function finalizeControl(wrap, bar) {
  if (!bar) return wrap;
  const box = document.createElement("div");
  box.className = "controls";
  box.appendChild(wrap);
  return box;
}

// One document-level listener closes any open dropdown-multiselect when you
// click outside it. Installed once (queries the DOM live, so it covers widgets
// created on later re-runs too — no per-widget listener to leak).
let msAutoCloseInstalled = false;
function installMsAutoClose() {
  if (msAutoCloseInstalled) return;
  msAutoCloseInstalled = true;
  document.addEventListener("mousedown", (e) => {
    document.querySelectorAll("details.dp-ms[open]").forEach((d) => {
      if (!d.contains(e.target)) d.open = false;
    });
  });
}

// ::PERIOD — every date computation is Rust (wasm period_resolve, src/period.rs);
// JS keeps only the viewer's choice and binds the resolved variables.
function localToday() {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
function periodResolve(spec, column, state) {
  try {
    return JSON.parse(period_resolve(spec, JSON.stringify({ ...state, column, today: localToday() })));
  } catch (e) {
    return { ok: false, errors: [String(e)] };
  }
}
// The SET VARIABLE statements for a resolved period (unquoted names — the
// served mode captures them by that shape; NULL without a comparison).
function periodSets(prefix, vars) {
  return Object.entries(vars).map(
    ([k, v]) => `SET VARIABLE ${prefix}_${k} = ${v == null ? "NULL" : `'${String(v).replace(/'/g, "''")}'`}`
  );
}
function makePeriodControl(meta, bar) {
  const res = meta.res;
  const key = meta.varname;
  const commit = (patch) => {
    const cur = dpVars[key] && typeof dpVars[key] === "object" ? dpVars[key] : {};
    dpVars[key] = { ...cur, ...patch };
    run(false);
  };
  const cont = document.createElement("div");
  cont.className = "control dp-period";
  cont.setAttribute("role", "group");
  const capText = meta.caption || "Period";
  cont.setAttribute("aria-label", capText);
  const cap = document.createElement("span");
  cap.textContent = capText + ":";
  cont.appendChild(cap);

  const sel = document.createElement("select");
  sel.className = "dp-period-preset";
  sel.setAttribute("aria-label", "Date range");
  for (const p of res.presets) {
    const o = document.createElement("option");
    o.value = p.id;
    o.textContent = p.label;
    if (p.id === res.preset) o.selected = true;
    sel.appendChild(o);
  }
  // Switching to "Custom range" starts from the range on screen.
  sel.onchange = () => commit(sel.value === "custom" ? { preset: "custom", from: res.from, to: res.to } : { preset: sel.value });
  cont.appendChild(sel);

  if (res.preset === "custom") {
    const mk = (k, label) => {
      const inp = document.createElement("input");
      inp.type = "date";
      inp.value = res[k] || "";
      inp.setAttribute("aria-label", label);
      inp.onchange = () => {
        if (inp.value) commit({ preset: "custom", from: res.from, to: res.to, [k]: inp.value });
      };
      return inp;
    };
    const arrow = document.createElement("span");
    arrow.textContent = "→";
    arrow.className = "daterange-arrow";
    cont.append(mk("from", "From"), arrow, mk("to", "To"));
  }

  // The grain switch (grains=none in the spec hides it).
  if (res.grains && res.grains.length) {
    const seg = document.createElement("div");
    seg.className = "dp-seg";
    seg.setAttribute("role", "group");
    seg.setAttribute("aria-label", "Time grain");
    for (const g of res.grains) {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = g[0].toUpperCase() + g.slice(1);
      b.setAttribute("aria-pressed", String(g === res.grain));
      if (g === res.grain && res.grain_auto) b.title = "Picked for this range — click to keep it";
      b.onclick = () => commit({ grain: g });
      seg.appendChild(b);
    }
    cont.appendChild(seg);
  }

  const cmp = document.createElement("select");
  cmp.className = "dp-period-compare";
  cmp.setAttribute("aria-label", "Compare with");
  for (const c of res.compares) {
    const o = document.createElement("option");
    o.value = c.id;
    o.textContent = c.label;
    if (c.id === res.compare) o.selected = true;
    cmp.appendChild(o);
  }
  cmp.onchange = () => commit({ compare: cmp.value });
  cont.appendChild(cmp);

  const txt = document.createElement("span");
  txt.className = "dp-period-range";
  txt.setAttribute("aria-live", "polite");
  const b = document.createElement("b");
  b.textContent = res.vars.label;
  txt.appendChild(b);
  if (res.vars.prev_label) txt.appendChild(document.createTextNode(" " + res.vars.prev_label));
  cont.appendChild(txt);

  if (!res.ok && res.errors && res.errors.length) {
    const e = document.createElement("span");
    e.className = "dp-period-err";
    e.textContent = "⚠ " + res.errors[0];
    e.title = res.errors.join("\n");
    cont.appendChild(e);
  }
  return finalizeControl(cont, bar);
}

function makeControl(meta, bar) {
  if (meta.kind === "PERIOD") return makePeriodControl(meta, bar);
  const wrap = document.createElement("label");
  wrap.className = "control";
  wrap.textContent = (meta.caption || meta.varname || "date") + ":";
  if (meta.kind === "DATERANGE") {
    const mk = (k) => {
      const inp = document.createElement("input");
      inp.type = "date";
      inp.value = dpVars[k] || "";
      inp.onchange = () => {
        dpVars[k] = inp.value;
        run(false);
      };
      return inp;
    };
    wrap.appendChild(mk(meta.varnames[0]));
    const arrow = document.createElement("span");
    arrow.textContent = "→";
    arrow.className = "daterange-arrow";
    wrap.appendChild(arrow);
    wrap.appendChild(mk(meta.varnames[1]));
    return finalizeControl(wrap, bar);
  }
  if (meta.kind === "BUTTONS") return finalizeControl(mkButtons(meta), bar);
  if (meta.kind === "TOGGLE") {
    // A native checkbox with the switch role: Space toggles it, screen readers
    // announce on/off; the caption is its label.
    wrap.textContent = "";
    wrap.classList.add("dp-toggle");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.setAttribute("role", "switch");
    cb.checked = dpVars[meta.varname] === "true";
    refocus("toggle:" + meta.varname, cb);
    cb.onchange = () => {
      if (document.activeElement === cb) dpRefocus = "toggle:" + meta.varname;
      dpVars[meta.varname] = cb.checked ? "true" : "false";
      run(false);
    };
    const cap = document.createElement("span");
    cap.textContent = meta.caption || meta.varname;
    wrap.append(cb, cap);
    return finalizeControl(wrap, bar);
  }
  let input;
  if (meta.kind === "DROPDOWN") {
    input = document.createElement("select");
    input.setAttribute("aria-label", meta.caption || meta.varname);
    meta.options.forEach((o, k) => {
      const opt = document.createElement("option");
      opt.value = o;
      const hint = meta.hints && meta.hints[k];
      opt.textContent = hint ? `${o} — ${hint}` : o;
      if (o === dpVars[meta.varname]) opt.selected = true;
      input.appendChild(opt);
    });
    input.onchange = () => {
      dpVars[meta.varname] = input.value;
      run(false);
    };
  } else if (meta.kind === "MULTISELECT") {
    // Dropdown-style multiselect: a summary button opens a popover of
    // checkboxes (searchable when the list is long). Built in its own
    // `.control` container so the option <label>s aren't nested inside the
    // caption <label> (invalid + would hijack clicks).
    const options = meta.options;
    const sel = new Set(dpVars[meta.varname] || []);
    const cont = document.createElement("div");
    cont.className = "control";
    const cap = document.createElement("span");
    cap.textContent = (meta.caption || meta.varname || "") + ":";
    cont.appendChild(cap);

    const det = document.createElement("details");
    det.className = "dp-ms";
    const sum = document.createElement("summary");
    sum.className = "dp-ms-btn";
    det.appendChild(sum);
    const pop = document.createElement("div");
    pop.className = "dp-ms-pop";
    det.appendChild(pop);

    const summarize = () => {
      // name the picks while they fit; otherwise count them
      const picked = options.filter((o) => sel.has(o));
      const names = picked.join(", ");
      sum.textContent =
        sel.size === 0
          ? "None"
          : sel.size === options.length
            ? "All"
            : picked.length <= 3 && names.length <= 32
              ? names
              : `${sel.size} of ${options.length}`;
      sum.title = sel.size && sel.size < options.length ? names : "";
    };
    // Re-running rebuilds the controls (closing this popover), so we batch:
    // toggles just update `sel` + the summary, and we commit once on close.
    let dirty = false;
    det.addEventListener("toggle", () => {
      if (!det.open && dirty) {
        dirty = false;
        dpVars[meta.varname] = [...sel];
        run(false);
      }
    });

    const rows = [];
    // A search box appears once the list gets long, so many categories stay usable.
    if (options.length > 50) {
      const sb = document.createElement("input");
      sb.type = "search";
      sb.placeholder = `Search ${options.length} options…`;
      sb.className = "dp-ms-search";
      sb.oninput = () => {
        const q = sb.value.toLowerCase();
        for (const r of rows) r.row.style.display = r.o.toLowerCase().includes(q) ? "" : "none";
      };
      sb.onkeydown = (e) => e.stopPropagation(); // don't let space/enter toggle <details>
      pop.appendChild(sb);
    }
    const tools = document.createElement("div");
    tools.className = "dp-ms-tools";
    const allB = document.createElement("button");
    allB.type = "button";
    allB.textContent = "All";
    const noB = document.createElement("button");
    noB.type = "button";
    noB.textContent = "None";
    tools.append(allB, noB);
    pop.appendChild(tools);

    const list = document.createElement("div");
    list.className = "dp-ms-list";
    pop.appendChild(list);
    for (const o of options) {
      const row = document.createElement("label");
      row.className = "dp-ms-opt";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = sel.has(o);
      cb.onchange = () => {
        if (cb.checked) sel.add(o);
        else sel.delete(o);
        dirty = true;
        summarize();
      };
      const tx = document.createElement("span");
      tx.textContent = o;
      row.append(cb, tx);
      list.appendChild(row);
      rows.push({ o, row, cb });
    }
    allB.onclick = () => {
      for (const r of rows) {
        sel.add(r.o);
        r.cb.checked = true;
      }
      dirty = true;
      summarize();
    };
    noB.onclick = () => {
      sel.clear();
      for (const r of rows) r.cb.checked = false;
      dirty = true;
      summarize();
    };
    summarize();
    installMsAutoClose();
    cont.appendChild(det);
    return finalizeControl(cont, bar);
  } else {
    input = document.createElement("input");
    input.type = meta.kind === "NUMBER" ? "number" : meta.kind === "DATE" ? "date" : "text";
    input.value = dpVars[meta.varname] ?? "";
    input.onchange = () => {
      dpVars[meta.varname] = input.value;
      run(false);
    };
  }
  wrap.appendChild(input);
  return finalizeControl(wrap, bar);
}

// The control to focus again after the re-run a keyboard change triggers
// (re-running rebuilds the controls): "<kind>:<variable>".
let dpRefocus = null;
function refocus(key, el) {
  if (dpRefocus !== key) return;
  dpRefocus = null;
  // The new controls are attached once the whole run has rendered.
  let tries = 0;
  const go = () => (el.isConnected ? el.focus() : ++tries < 120 && requestAnimationFrame(go));
  requestAnimationFrame(go);
}
// A ::BUTTONS input: a segmented radio group (one pressed button). Arrow keys
// move between the options (roving tabindex), like a native radio group.
function mkButtons(meta) {
  const cont = document.createElement("div");
  cont.className = "control";
  const cap = document.createElement("span");
  cap.textContent = (meta.caption || meta.varname || "") + ":";
  cap.id = "dp-seg-" + meta.varname;
  const seg = document.createElement("div");
  seg.className = "dp-seg";
  seg.setAttribute("role", "radiogroup");
  seg.setAttribute("aria-labelledby", cap.id);
  const btns = meta.options.map((o, k) => {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = o;
    b.setAttribute("role", "radio");
    const on = o === dpVars[meta.varname];
    b.setAttribute("aria-checked", on ? "true" : "false");
    b.tabIndex = on ? 0 : -1;
    if (on) refocus("seg:" + meta.varname, b);
    const hint = meta.hints && meta.hints[k];
    if (hint) b.title = hint;
    b.onclick = () => {
      if (dpVars[meta.varname] === o) return;
      if (document.activeElement === b) dpRefocus = "seg:" + meta.varname;
      dpVars[meta.varname] = o;
      run(false);
    };
    return b;
  });
  seg.addEventListener("keydown", (e) => {
    const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
    if (!step) return;
    e.preventDefault();
    const at = btns.indexOf(document.activeElement);
    const nb = btns[(at + step + btns.length) % btns.length];
    dpRefocus = "seg:" + meta.varname;
    nb.click();
  });
  seg.append(...btns);
  cont.append(cap, seg);
  return cont;
}

// A contract panel's payload cell (`c{idx}`) as JSON text: DuckDB-Wasm returns
// the JSON column as a string, a native backend may return it nested.
function contractCell(row, idx, empty) {
  const v = row ? row["c" + idx] : null;
  if (v == null) return empty;
  return typeof v === "string" ? v : JSON.stringify(v);
}

// A contract panel's caption: the ::TITLE plus its GROUP BY key values (one
// plot per key) — "Coefficients — ols", or just either.
function contractCaption(title, contract, row) {
  const keys = (contract.groups || [])
    .map(([i]) => (row ? row["c" + i] : null))
    .filter((v) => v != null && v !== "")
    .map((v) => String(v).replace(/^"|"$/g, ""));
  const key = keys.join(" · ");
  if (title && key) return `${title} — ${key}`;
  return title || key || "";
}

// Default render height of a contract plot (::HEIGHT overrides): the 2×2
// diagnostics need room, single panels match the chart default.
function contractHeight(macro) {
  const H = { anofox_plot_diagnostics: 560, anofox_plot_summary: 360, anofox_plot_terms: 340 };
  return H[macro] || 320;
}

// A per-panel title bar from a ::TITLE column (constant across the result).
function titleOf(s, rowsJson) {
  const tr = s.roles.find((r) => r[1] === "TITLE");
  if (!tr) return null;
  let rows;
  try {
    rows = JSON.parse(rowsJson);
  } catch (_) {
    return null;
  }
  const v = rows[0] ? rows[0]["c" + tr[0]] : null;
  return v == null ? null : String(v).replace(/^"|"$/g, "");
}

// {{col}} / {{col:fmt}} in a ::MARKDOWN text → the statement's own row,
// formatted like a KPI. The Markdown is rendered (and sanitised) with inert
// private-use tokens in their place; each token in text then becomes the
// HTML-escaped value, one in a formula's TeX becomes TeX text, any other
// token inside a tag (a link URL, an attribute) is dropped — a value can
// never become markup, a TeX command or a URL. Unknown columns stay
// as written. `fmt(value, spec)` formats one value.
function markdownWithValues(src, row, fmt) {
  const vals = [];
  const keys = Object.keys(row || {});
  const md = String(src ?? "").replace(/\{\{\s*([A-Za-z_][\w ]*?)\s*(?::\s*([A-Za-z_]+)\s*)?\}\}/g, (m, name, spec) => {
    const key = keys.find((k) => k.toLowerCase() === name.toLowerCase());
    if (key === undefined) return m;
    vals.push(String(fmt(row[key], (spec || "").toLowerCase())));
    return `\uE000${vals.length - 1}\uE001`;
  });
  const html = renderMarkdown(md);
  if (!vals.length) return html;
  const tok = /\uE000(\d+)\uE001/g;
  // in a formula's TeX (data-tex): the value as \text{…}, TeX-escaped
  const tex = (part) =>
    part.replace(/( data-tex=")([^"]*)/, (m, a, t) => a + t.replace(tok, (_, k) => escapeHtml(texText(vals[+k] ?? ""))));
  return html.replace(/<[^>]*>|[^<]+/g, (part) =>
    part[0] === "<" ? tex(part).replace(tok, "") : part.replace(tok, (_, k) => escapeHtml(vals[+k] ?? ""))
  );
}
// A value as TeX text: \text{…} with TeX's special characters escaped.
function texText(v) {
  const sp = { "\\": "\\textbackslash{}", "^": "\\textasciicircum{}", "~": "\\textasciitilde{}" };
  return `\\text{${String(v).replace(/[\\{}$%&#_^~]/g, (c) => sp[c] ?? "\\" + c)}}`;
}
// One {{col:fmt}} value: money / percent / compact / metric (numbers, as the
// KPI roles), date, text; no spec = a number or ISO date in the dashboard
// locale (like a table cell), other text as is.
const MD_FMT = { money: "MONEY", currency: "MONEY", usd: "MONEY", eur: "MONEY", dollar: "MONEY", percent: "PERCENT", pct: "PERCENT", compact: "COMPACT", metric: "METRIC", number: "METRIC", num: "METRIC" };
function mdValue(v, spec) {
  if (v == null) return "–";
  if (MD_FMT[spec]) return fmtNum(v, MD_FMT[spec]);
  if (spec === "text") return String(v);
  const loc = dashLocale();
  if (spec === "date") {
    // DuckDB-Wasm may hand a DATE / TIMESTAMP over as epoch ms
    const iso = typeof v === "number" ? new Date(v).toISOString().slice(0, 10) : String(v).slice(0, 10);
    return loc ? format_date_locale(iso, loc) : iso;
  }
  if (typeof v === "number" || /^-?\d+(\.\d+)?$/.test(String(v))) return fmtNum(v, "METRIC");
  return loc ? format_date_locale(String(v), loc) : String(v);
}
// ::CALLOUT value → 'note' | 'warning' | 'success' (src/lint.rs CALLOUT_KINDS).
function calloutKind(v) {
  const k = String(v ?? "").trim().toLowerCase();
  if (k === "warning" || k === "caution" || k === "danger") return "warning";
  if (k === "success" || k === "tip") return "success";
  return "note";
}
// One cell of a result (by output position) as text.
function cellAt(rowsJson, idx) {
  let r0;
  try {
    r0 = JSON.parse(rowsJson)[0];
  } catch (_) {
    return "";
  }
  const v = r0 ? (r0["c" + idx] ?? Object.values(r0)[idx]) : null;
  return v == null ? "" : String(v).replace(/^"|"$/g, "");
}
// ::INFO: an ⓘ button on the panel's title bar (top-right corner without one)
// that shows the definition on hover, keyboard focus or click; Esc closes it.
// The tooltip is placed on <body> while open, so a tile's clipping or
// stacking never hides it.
let dpInfoSeq = 0;
function addInfo(fig, text) {
  if (!text.trim()) return;
  const id = "dp-info-" + ++dpInfoSeq;
  const wrap = document.createElement("span");
  wrap.className = "dp-info";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "dp-info-btn";
  btn.textContent = "ⓘ";
  btn.setAttribute("aria-label", "About this panel");
  btn.setAttribute("aria-describedby", id);
  btn.setAttribute("aria-expanded", "false");
  const pop = document.createElement("div");
  pop.className = "dp-info-pop";
  pop.id = id;
  pop.setAttribute("role", "tooltip");
  pop.textContent = text;
  pop.hidden = true;
  wrap.append(btn, pop);
  let pinned = false;
  const show = (on) => {
    btn.setAttribute("aria-expanded", on ? "true" : "false");
    wrap.classList.toggle("open", on);
    if (!on) {
      pop.hidden = true;
      if (pop.parentNode !== wrap) wrap.appendChild(pop); // keep it in the panel (aria, export)
      return;
    }
    document.body.appendChild(pop);
    pop.hidden = false;
    const r = btn.getBoundingClientRect();
    const w = pop.offsetWidth;
    const left = Math.max(8, Math.min(r.left - 6, innerWidth - w - 8));
    const below = r.bottom + 6 + pop.offsetHeight < innerHeight;
    pop.style.left = left + scrollX + "px";
    pop.style.top = (below ? r.bottom + 6 : r.top - 6 - pop.offsetHeight) + scrollY + "px";
  };
  btn.onmouseenter = () => show(true);
  btn.onmouseleave = () => pinned || show(false);
  btn.onfocus = () => show(true);
  btn.onblur = () => {
    pinned = false;
    show(false);
  };
  btn.onclick = (e) => {
    e.stopPropagation();
    pinned = !pinned;
    show(pinned);
  };
  btn.onkeydown = (e) => {
    if (e.key !== "Escape") return;
    pinned = false;
    show(false);
  };
  const title = fig.querySelector(".panel-title");
  if (title) title.appendChild(wrap);
  else {
    wrap.classList.add("corner");
    fig.appendChild(wrap);
  }
}

function mkTitle(text) {
  const c = document.createElement("figcaption");
  c.className = "panel-title";
  c.textContent = text;
  return c;
}

// An empty result: the author's ::EMPTY message, else a generic note.
function mkNoData(msg) {
  const d = document.createElement("div");
  d.className = "nodata";
  d.setAttribute("role", "status");
  d.textContent = msg || "No data for this selection";
  return d;
}

// A failed query: a short message, the details (error + SQL) behind a toggle.
// Stack frames / DuckDB stack traces are dropped — the viewer gets the cause.
function shortError(msg) {
  const line = errorDetail(msg)
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l && !/^LINE \d+:|^\^/.test(l));
  const t = (line || "Unknown error").replace(/^(Uncaught )?Error:\s*/, "");
  return t.length > 160 ? t.slice(0, 157) + "…" : t;
}
function errorDetail(msg) {
  return String(msg)
    .split(/\n\s*Stack Trace:/i)[0]
    .split("\n")
    .filter((l) => !/^\s+at\s/.test(l))
    .join("\n")
    .trim();
}
function mkErrorBody(e, sql) {
  const box = document.createElement("div");
  box.className = "dp-err";
  const head = document.createElement("div");
  head.className = "dp-err-head";
  head.textContent = "Couldn't load this panel";
  const msg = document.createElement("div");
  msg.className = "dp-err-msg";
  msg.textContent = shortError(e);
  const det = document.createElement("details");
  const sum = document.createElement("summary");
  sum.textContent = "Details";
  const pre = document.createElement("pre");
  pre.textContent = errorDetail(e) + (sql ? "\n\n" + sql : "");
  det.append(sum, pre);
  box.append(head, msg, det);
  return box;
}
function mkErrorCard(e, sql, wide = false) {
  const fig = document.createElement("figure");
  fig.className = "panel dp-errcard";
  if (wide) fig.style.gridColumn = "1/-1";
  fig.appendChild(mkErrorBody(e, sql));
  return fig;
}

// An optional input (`-- @requires <path>` above the statement) whose file /
// table is missing: the query failed with a missing-input error.
const isOptionalMissing = (s, e) => !!(s && s.requires && s.requires.length) && isMissingInputError(String(e));
function isMissingInputError(msg) {
  return /no files found|cannot open file|no such file|does not exist|http 404|couldn't fetch/i.test(String(msg));
}
function mkNotAvailable(s) {
  const fig = document.createElement("figure");
  fig.className = "panel dp-na";
  const d = document.createElement("div");
  d.className = "nodata";
  d.setAttribute("role", "status");
  const b = document.createElement("strong");
  b.textContent = "Not available";
  const why = document.createElement("span");
  why.textContent = "This run has no " + s.requires.map((p) => p.split("/").pop()).join(", ");
  d.append(b, why);
  fig.appendChild(d);
  return fig;
}

// ::THRESHOLDS rules, parsed by the wasm `threshold_rules` (src/thresholds.rs):
// [{column, rules: [{lo, hi, status, text}]}] — [] for a malformed spec
// (`dashboard --check` reports it).
function parseThresholds(spec) {
  try {
    const j = JSON.parse(threshold_rules(String(spec ?? "")));
    return j.ok ? j.groups : [];
  } catch (_) {
    return [];
  }
}
// The first rule a number meets (its status: good / warn / bad / neutral), or null.
function thresholdRule(rules, v) {
  const n = typeof v === "number" ? v : cleanNum(v);
  if (n == null || !isFinite(n)) return null;
  const ok = (b, below) => !b || (below ? (b[1] ? n <= b[0] : n < b[0]) : b[1] ? n >= b[0] : n > b[0]);
  return (rules || []).find((r) => ok(r.lo, false) && ok(r.hi, true)) || null;
}
const STATUS_ICON = { good: "●", warn: "▲", bad: "◆", neutral: "○" };

const cleanNum = (v) => {
  if (v == null) return null;
  if (typeof v === "number") return v;
  const n = parseFloat(String(v).replace(/^"|"$/g, ""));
  return isNaN(n) ? null : n;
};

// One per-column filter test: substring match, or a numeric comparison
// (>n / >=n / <n / <=n) when the column is numeric.
function matchCell(v, q, isNum) {
  const m = q.match(/^([<>]=?)\s*(-?\d[\d.]*)$/);
  if (m && isNum) {
    const n = cleanNum(v),
      t = parseFloat(m[2]);
    if (n == null || isNaN(t)) return false;
    return m[1] === ">" ? n > t : m[1] === ">=" ? n >= t : m[1] === "<" ? n < t : n <= t;
  }
  return String(v ?? "")
    .replace(/^"|"$/g, "")
    .toLowerCase()
    .includes(q.toLowerCase());
}

// A dual-handle range slider. Calls onChange(lo, hi) with nulls for an
// unbounded end (so a full-range slider = no filter). `init` = [lo, hi] or null.
function dualRange(min, max, init, onChange, fmt = null) {
  const wrap = document.createElement("div");
  wrap.className = "dp-range";
  wrap.onclick = (e) => e.stopPropagation();
  const trk = document.createElement("div");
  trk.className = "trk";
  const fil = document.createElement("div");
  fil.className = "fil";
  const out = document.createElement("div");
  out.className = "out";
  const lo = document.createElement("input");
  const hi = document.createElement("input");
  const span = max - min || 1;
  [lo, hi].forEach((inp) => {
    inp.type = "range";
    inp.min = min;
    inp.max = max;
    inp.step = span / 200 || 1;
  });
  lo.value = init && init[0] != null ? init[0] : min;
  hi.value = init && init[1] != null ? init[1] : max;
  const fmtv = fmt || ((v) => (Math.abs(v) >= 1000 ? Math.round(v).toLocaleString() : String(Math.round(v * 100) / 100)));
  const paint = () => {
    const a = +lo.value,
      b = +hi.value;
    const pa = ((a - min) / span) * 100,
      pb = ((b - min) / span) * 100;
    fil.style.left = pa + "%";
    fil.style.width = Math.max(0, pb - pa) + "%";
    out.textContent = `${fmtv(a)} – ${fmtv(b)}`;
  };
  const fire = () => onChange(+lo.value <= min ? null : +lo.value, +hi.value >= max ? null : +hi.value);
  lo.oninput = () => {
    if (+lo.value > +hi.value) lo.value = hi.value;
    paint();
    fire();
  };
  hi.oninput = () => {
    if (+hi.value < +lo.value) hi.value = lo.value;
    paint();
    fire();
  };
  wrap.append(trk, fil, lo, hi, out);
  paint();
  return wrap;
}

// A ::TABLE result → a sortable HTML table with in-cell bars + per-column
// formatting (fmtByIdx maps an output column index to a format role). Pass
// `server` = {total, page, pageSize, onPage, onSort, sortCol, sortDir} for
// SQL-driven pagination/sorting (::PAGED); otherwise it paginates client-side.
// A strict "is this a number" test for auto-detecting numeric columns (cleanNum
// alone would read an ISO date "2026-06-01" as 2026).
const NUM_RE = /^-?\d+(\.\d+)?([eE][+-]?\d+)?$/;
const isNumLike = (v) => (typeof v === "number" ? isFinite(v) : NUM_RE.test(String(v ?? "").replace(/^"|"$/g, "")));
const isBoolLike = (v) => v === true || v === false || v === "true" || v === "false";

// Rich, readable data table: per-column number formats, slim data bars, soft
// colour scales, a toolbar (row count, search, filter chips, Filters / Totals /
// Columns / Pages|Scroll / CSV), a totals row, column statistics on the header,
// opt-in row selection and a virtualised Scroll view.
//
// `server` = a ::PAGED table: rows are one page (or, in Scroll view, the first
// block) and sorting, the search, the per-column filters, the totals and the
// column statistics are all computed in SQL by the caller (server.stats,
// server.onColFilters, server.distinct, server.fetchRows, …). `opts.select` =
// the ::ROWSELECT mode: "single" | "multi" (anything else: rows aren't clickable).
// Readable header for a raw column name (snake_case / camelCase, as SQL emits
// it: `load_pct` → "Load %"); an `AS "Header"` alias is kept as it is. The
// shared Rust rule (wasm human_header), like every table formatting decision.
function humanHeader(c) {
  return human_header(String(c));
}
// A number in a table cell as its column profile (wasm table_profile) formats
// it: the column's unit (scaled; in the header or the cell), a KPI format, or
// fixed decimals; a true minus sign or accounting parentheses. The browser twin
// of the Rust table::number_text — shared vectors in tests/fixtures/table_vectors.json.
function tableNumText(n, p, f, loc, neg) {
  if (!isFinite(n)) return "–";
  const nf = (dec, group) =>
    ((tableNumText.nf ||= {})[`${loc}|${dec}|${group}`] ||= new Intl.NumberFormat(loc, {
      minimumFractionDigits: dec,
      maximumFractionDigits: dec,
      useGrouping: group,
      signDisplay: "negative",
    }));
  const minus = (t) => (neg === "parens" && /^-/.test(t) ? `(${t.slice(1)})` : t.replace(/^-/, "−"));
  const u = p.unit;
  if (u) {
    const num = nf(u.dec, true).format(Math.abs(n) / u.div);
    return minus(`${n < 0 ? "-" : ""}${u.inHead ? num : `${u.pre}${num}${u.word}${u.post}`}`);
  }
  if (METRICS.includes(f)) return minus(fmtNum(n, f));
  return minus(nf(p.dec, !p.label).format(n));
}
const ROW_H_COMFY = 37; // row height of the virtualised Scroll view (px), comfortable density
const ROW_H_COMPACT = 29; // … compact density
const SCROLL_BLOCK = 100; // rows per fetch of a ::PAGED table in Scroll view
// the aggregations a totals cell can show (::TOTALS, or the viewer's pick)
const AGGS = ["sum", "mean", "median", "min", "max", "count"];
const AGG_WORD = { sum: "sum", mean: "mean", median: "median", min: "minimum", max: "maximum", count: "count of values" };
const AGG_LABEL = { sum: "Sum", mean: "Mean", median: "Median", min: "Min", max: "Max", count: "Count" };
// The open totals menu closes on a click elsewhere or Esc (one listener for all tables).
let totMenuWired = false;
function wireTotMenuClose() {
  if (totMenuWired) return;
  totMenuWired = true;
  const close = () => document.querySelectorAll(".tot-menu").forEach((m) => m.closeMenu());
  document.addEventListener("mousedown", (e) => {
    if (!e.target.closest?.(".tot-menu, .tot-agg")) close();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });
  window.addEventListener("scroll", close, true); // it is placed at the label: don't let it drift
}
// Tables to re-render for print (every matching row of a client table, not one page)
const dpPrintHooks = new Set();
for (const ev of ["beforeprint", "afterprint"])
  addEventListener(ev, () => {
    for (const h of [...dpPrintHooks]) h(ev === "beforeprint");
  });
function renderTable(rows, skip = -1, fmtByIdx = {}, server = null, key = null, opts = {}) {
  const unquote = unq;
  if (!rows.length && !server) {
    const empty = document.createElement("div");
    empty.className = "tbl-empty";
    empty.textContent = "No rows";
    return empty;
  }
  const t = document.createElement("table");
  t.className = "dp-table";
  // UI state: a client table keeps it per panel across cross-filter re-runs
  // (dpTableUI); a ::PAGED table's lives with its caller (server.colState), which
  // also turns the column filters into SQL.
  const saved = server ? server.colState : key != null ? dpTableUI[key] || {} : {};
  const persist = () => {
    const st = { sortCol, dir, page, pageSize, colQ, colRange, colEq, agg: aggPick, q, hidden: [...hidden], showFilters, showTotals, showStats, view, compact };
    if (server) Object.assign(server.colState, st);
    else if (key != null) dpTableUI[key] = st;
  };
  // (a ::PAGED table shares these objects with its caller)
  const colQ = server ? (saved.colQ ||= {}) : { ...(saved.colQ || {}) }; // text / >n per column
  const colRange = server ? (saved.colRange ||= {}) : { ...(saved.colRange || {}) }; // [lo,hi] per numeric column
  const colEq = server ? (saved.colEq ||= {}) : { ...(saved.colEq || {}) }; // exact value per categorical column
  const aggPick = server ? (saved.agg ||= {}) : { ...(saved.agg || {}) }; // the viewer's totals aggregation per column
  let q = server ? "" : saved.q || ""; // client search (a ::PAGED table's is server.search)
  const hidden = new Set(saved.hidden || []);
  const allKeys = rows.length ? Object.keys(rows[0]) : server.statCols || [];
  const colFmt = {}; // column key -> format role
  for (const [idx, f] of Object.entries(fmtByIdx)) colFmt[allKeys[idx]] = f;
  const cols = allKeys.filter((_, i) => i !== skip);
  const loc = dashLocale() || NUM_LOCALE;
  const selMode = ["single", "multi"].includes(opts.select) ? opts.select : "none";
  // ::TOTALS / ::COLGROUP / ::FLAG / ::HIGHLIGHT / ::SUBTOTAL name columns by the raw name or
  // the header people read (case-insensitive)
  const colNamed = (name) => {
    const n = String(name).trim().toLowerCase();
    return cols.find((c) => c.toLowerCase() === n || humanHeader(c).toLowerCase() === n);
  };
  // ::TOTALS: one aggregation for every numeric column ('none' hides the row),
  // and / or per column ('Revenue: sum, Margin: mean')
  const totSpec = { all: null, col: {} };
  for (const part of String(opts.totals || "").split(",")) {
    const m = part.match(/^\s*(?:(.+?)\s*:\s*)?([a-z]+)\s*$/i);
    const a = m ? m[2].toLowerCase().replace(/^(avg|average)$/, "mean") : "";
    if (![...AGGS, "none"].includes(a)) continue;
    if (!m[1]) totSpec.all = a;
    else if (colNamed(m[1])) totSpec.col[colNamed(m[1])] = a;
  }
  const totalsOff = totSpec.all === "none" && !Object.values(totSpec.col).some((a) => a !== "none");
  // ::COLGROUP 'Backtest: mae, rmse | Holdout: mae_h': a label over adjacent columns
  const groupOf = {};
  for (const part of String(opts.colgroup || "").split("|")) {
    const k = part.indexOf(":");
    const label = part.slice(0, k).trim();
    if (k < 0 || !label) continue;
    for (const name of part.slice(k + 1).split(",")) if (colNamed(name)) groupOf[colNamed(name)] = label;
  }
  // ::FLAG 'mape > 0.2, status = breach': cells meeting a condition get a ⚠
  const flags = {};
  for (const part of String(opts.flag || "").split(",")) {
    const m = part.match(/^\s*(.+?)\s*(>=|<=|!=|<>|>|<|=)\s*(.+?)\s*$/);
    const c = m && colNamed(m[1]);
    if (c) (flags[c] ||= []).push({ op: m[2], w: m[3].replace(/^['"]|['"]$/g, ""), text: part.trim() });
  }
  const flagOf = (c, v) =>
    v != null &&
    (flags[c] || []).find(({ op, w }) => {
      const n = cleanNum(v);
      const x = Number(w);
      if (n != null && w !== "" && isFinite(x))
        return { ">": n > x, ">=": n >= x, "<": n < x, "<=": n <= x, "=": n === x, "!=": n !== x, "<>": n !== x }[op];
      const a = unquote(v).toLowerCase();
      return op === "=" ? a === w.toLowerCase() : op === "!=" || op === "<>" ? a !== w.toLowerCase() : false;
    });
  // ::THRESHOLDS 'Margin %: <10:bad; <15:warn; good | …': each named column's
  // cells take their status tint and shape (● good, ▲ warn, ◆ bad — never colour alone)
  const thresholds = {};
  for (const g of opts.thresholds ? parseThresholds(opts.thresholds) : []) {
    const c = g.column != null && colNamed(g.column);
    if (c) thresholds[c] = g.rules;
  }
  // ::HIGHLIGHT 'Revenue: top 3, Margin: bottom 2' ('top 3' alone: the first numeric column)
  const hlSpec = {};
  for (const part of String(opts.highlight || "").split(",")) {
    const m = part.match(/^\s*(?:(.+?)\s*:\s*)?(top|bottom)\s+(\d+)\s*$/i);
    if (m && (!m[1] || colNamed(m[1]))) hlSpec[m[1] ? colNamed(m[1]) : ""] = { top: m[2].toLowerCase() === "top", n: +m[3] };
  }
  // ::SUBTOTAL 'region': the rows grouped by that column, a subtotal row after
  // each group (client tables; a ::PAGED table has only the rows of one page)
  const subCol = (!server && opts.subtotal && colNamed(opts.subtotal)) || null;
  const SUB = Symbol("subtotal"); // a subtotal row in the Scroll view's row list
  const groupKey = (r) => unquote(r[subCol] ?? "");
  let subGroups = new Map(); // group key -> its matching rows

  // ── per-column profile: kind, range, decimals, unit, whether it gets data
  // bars — decided by the shared Rust table module (wasm table_profile), the
  // same rules the headless SVG table uses. A ::PAGED table profiles the whole
  // filtered result from its SQL statistics (n{i}, d{i}, s{i}, m{i}, lo{i},
  // hi{i}); a client table from its rows.
  const sstat = (c) => {
    if (!server || !server.stats || !server.statCols) return null;
    const j = server.statCols.indexOf(c);
    if (j < 0) return null;
    const g = (k) => (server.stats[k + j] == null ? null : Number(server.stats[k + j]));
    return { n: g("n"), d: g("d"), s: g("s"), m: g("m"), md: g("md"), lo: g("lo"), hi: g("hi") };
  };
  const profiles = JSON.parse(
    table_profile(
      JSON.stringify({
        locale: loc,
        many: rows.length > 1 || !!server,
        cols: cols.map((c, k) => {
          const st = sstat(c);
          return { name: c, fmt: colFmt[c] || "", first: k === 0, lo: st?.lo ?? null, hi: st?.hi ?? null, values: rows.map((r) => r[c] ?? null) };
        }),
      })
    )
  );
  const info = {};
  const unitOf = {};
  cols.forEach((c, k) => {
    const p = profiles[k];
    const vals = rows.map((r) => r[c]);
    const present = vals.filter((v) => v != null && unquote(v) !== "");
    const nums = present.map(cleanNum).filter((v) => v != null);
    const st = sstat(c);
    info[c] = {
      ...p,
      f: colFmt[c],
      empty: st ? (server.total || 0) - (st.n || 0) : vals.length - present.length,
      distinct: st && st.d != null ? st.d : new Set(present.map((v) => unquote(v))).size,
      mean: st ? st.m : nums.length ? nums.reduce((a, b) => a + b, 0) / nums.length : null,
      sum: st ? st.s : null,
      longText: !p.numeric && present.some((v) => String(v).length > 32),
    };
    if (p.unit) unitOf[c] = p.unit;
  });
  if (hlSpec[""]) {
    const c = cols.find((cc) => info[cc].numeric && !info[cc].label && cc !== cols[0]);
    if (c && !hlSpec[c]) hlSpec[c] = hlSpec[""];
    delete hlSpec[""];
  }
  // theme chrome.negatives ("minus" | "red" | "parens") and chrome.zero ("muted" | "dash")
  const negStyle = axToken("table-negatives", "minus");
  const zeroStyle = axToken("table-zero", "muted");
  // a true minus sign, or accounting parentheses
  const minus = (t) => (negStyle === "parens" && /^-/.test(t) ? `(${t.slice(1)})` : t.replace(/^-/, "−"));
  // text of one cell (display, search, totals, slider labels)
  const cellText = (v, c) => {
    if (v == null) return "";
    const i = info[c];
    if (i.numeric) {
      const n = cleanNum(v);
      if (n == null) return unquote(v);
      return tableNumText(n, i, i.f, loc, negStyle);
    }
    // a year among text (dates, names) reads as a label: never "2,019"
    if (/^\d{4}$/.test(unquote(v)) && +unquote(v) >= 1800 && +unquote(v) <= 2200) return unquote(v);
    return String(fmtCell(unquote(v)));
  };
  // the header people read (raw name in the tooltip), with the column's unit when the cells drop it
  const head = (c) => info[c].head;

  let sortCol = server ? server.sortCol : saved.sortCol ?? null;
  let dir = server ? server.sortDir || 1 : saved.dir ?? 1;
  let page = server ? server.page : saved.page || 0;
  let pageSize = server ? server.pageSize : saved.pageSize || 10;
  let view = server ? server.view || "pages" : saved.view || "pages";
  // density: compact rows by default for long tables
  let compact = saved.compact ?? (server ? server.unfiltered ?? server.total : rows.length) > 50;
  let ROW_H = compact ? ROW_H_COMPACT : ROW_H_COMFY;
  let showStats = !!saved.showStats; // the column-stats strip under the header
  let showFilters =
    !!saved.showFilters || Object.keys(colQ).length + Object.keys(colRange).length + Object.keys(colEq).length > 0;
  // Σ for amounts and counts; a column named like a rate / share / score is averaged (table_profile)
  const additive = (c) => info[c].additive;
  const hasAdditive = cols.some((c) => ["MONEY", "COMPACT"].includes(info[c].f));
  const nTotal = () => (server ? server.total : filtered.length);
  let showTotals =
    !totalsOff && (saved.showTotals ?? (opts.totals ? (server ? server.total : rows.length) >= 2 : (server ? server.total : rows.length) >= 3 && hasAdditive));
  const visCols = () => cols.filter((c) => !hidden.has(c));

  // ── filtering + sorting (client tables; a ::PAGED table's happen in SQL) ──
  let filtered = rows;
  const applyFilters = () => {
    if (server) return;
    const qs = Object.entries(colQ);
    const rs = Object.entries(colRange);
    const es = Object.entries(colEq);
    const needle = q.trim().toLowerCase();
    filtered =
      qs.length || rs.length || es.length || needle
        ? rows.filter(
            (r) =>
              qs.every(([cc, qq]) => matchCell(r[cc], qq, info[cc].numeric)) &&
              es.every(([cc, val]) => unquote(r[cc]) === val) &&
              rs.every(([cc, [lo, hi]]) => {
                const n = cleanNum(r[cc]);
                return n != null && (lo == null || n >= lo) && (hi == null || n <= hi);
              }) &&
              (!needle ||
                visCols().some((cc) => {
                  const v = r[cc];
                  return v != null && (unquote(v).toLowerCase().includes(needle) || cellText(v, cc).toLowerCase().includes(needle));
                }))
          )
        : rows;
  };
  applyFilters();
  let sortedCache = null; // the client table's filtered + sorted rows (Scroll view reads it)
  const sortedRows = () => {
    if (server) return rows;
    const data = filtered.slice();
    if (sortCol && info[sortCol]) {
      const num = info[sortCol].numeric;
      data.sort((a, b) => {
        const x = a[sortCol];
        const y = b[sortCol];
        if (x == null || y == null) return x == null ? (y == null ? 0 : 1) : -1; // empties last
        return num
          ? ((cleanNum(x) ?? 0) - (cleanNum(y) ?? 0)) * dir
          : unquote(x).localeCompare(unquote(y), undefined, { numeric: true }) * dir;
      });
    }
    if (!subCol) return data;
    // ::SUBTOTAL: each group together, the groups in order of their first row
    // (sorting by the group column orders the groups, any other sorts within them)
    const groups = new Map();
    for (const r of data) {
      const k = groupKey(r);
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(r);
    }
    return [...groups.values()].flat();
  };
  // after row j of the (grouped) rows: the key of the group it closes, else undefined
  const closesGroup = (data, j) =>
    subCol && (j === data.length - 1 || groupKey(data[j + 1]) !== groupKey(data[j])) ? groupKey(data[j]) : undefined;
  // A filter changed: a client table re-filters; a ::PAGED table asks its caller
  // to re-query (text input is debounced so each keystroke isn't a query).
  let colTimer = null;
  const filtersChanged = (debounce = false) => {
    persist();
    if (server) {
      clearTimeout(colTimer);
      colTimer = setTimeout(() => server.onColFilters(), debounce ? 300 : 0);
      return;
    }
    page = 0;
    applyFilters();
    persist();
    body();
    renderStatsRow();
    renderBar();
  };

  // ── toolbar ──
  const wrap = document.createElement("div");
  wrap.className = "table-wrap";
  const bar = document.createElement("div");
  bar.className = "tbl-bar";
  const barLeft = document.createElement("div");
  barLeft.className = "tbl-bar-l";
  const barRight = document.createElement("div");
  barRight.className = "tbl-bar-r";
  bar.append(barLeft, barRight);
  const mkBtn = (label, title, on, fn) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "tbl-btn" + (on ? " on" : "");
    b.textContent = label;
    b.title = title;
    b.onclick = (e) => {
      e.stopPropagation();
      fn(b);
    };
    return b;
  };
  const search = server ? server.search : document.createElement("input");
  if (!server) {
    search.type = "search";
    search.className = "tbl-search";
    search.placeholder = "Search…";
    search.value = q;
    search.onclick = (e) => e.stopPropagation();
    search.oninput = () => {
      q = search.value;
      filtersChanged();
    };
  }
  const chip = (text, clear, cls = "") => {
    const s = document.createElement("span");
    s.className = "tbl-chip" + cls;
    s.textContent = text;
    const x = document.createElement("button");
    x.type = "button";
    x.textContent = "×";
    x.title = "remove";
    x.onclick = (e) => {
      e.stopPropagation();
      clear();
    };
    s.appendChild(x);
    return s;
  };
  const numTxt = (n, c) => (n == null ? "…" : cellText(n, c));
  const filterChip = (text, clear) =>
    chip(text, () => {
      clear();
      renderFilterRow();
      filtersChanged();
    });
  function renderBar() {
    barLeft.innerHTML = "";
    const count = document.createElement("span");
    count.className = "tbl-count";
    const total = server ? server.unfiltered ?? server.total : rows.length;
    const shown = nTotal();
    const noun = total === 1 ? "row" : "rows";
    count.textContent =
      shown === total ? `${total.toLocaleString(loc)} ${noun}` : `${shown.toLocaleString(loc)} of ${total.toLocaleString(loc)} ${noun}`;
    barLeft.appendChild(count);
    for (const [c, v] of Object.entries(colEq)) barLeft.appendChild(filterChip(`${head(c)}: ${v}`, () => delete colEq[c]));
    for (const [c, v] of Object.entries(colQ))
      barLeft.appendChild(filterChip(`${head(c)} ${/^[<>]/.test(v) ? v : "∋ " + v}`, () => delete colQ[c]));
    for (const [c, [lo, hi]] of Object.entries(colRange))
      barLeft.appendChild(filterChip(`${head(c)}: ${numTxt(lo, c)} – ${numTxt(hi, c)}`, () => delete colRange[c]));
    if (!server && q.trim())
      barLeft.appendChild(
        filterChip(`“${q.trim()}”`, () => {
          q = "";
          search.value = "";
        })
      );
    // the multi-select selection, clearable in one click
    const k0 = cols[0];
    if (selMode === "multi" && Array.isArray(dpXf[k0]) && dpXf[k0].length)
      barLeft.appendChild(
        chip(
          `${dpXf[k0].length} selected`,
          () => {
            dpXf[k0] = [];
            run(false);
          },
          " sel"
        )
      );
    // rebuilding the toolbar detaches the search box: keep its focus and caret
    const typing = document.activeElement === search;
    const caret = typing ? search.selectionStart : null;
    barRight.innerHTML = "";
    const many = (server ? server.unfiltered ?? server.total : rows.length) > 10;
    if (server ? !!search : rows.length > 8) barRight.appendChild(search);
    if (typing && search.isConnected) {
      search.focus();
      if (caret != null) search.setSelectionRange(caret, caret);
    }
    if (many) {
      const nAct = Object.keys(colQ).length + Object.keys(colRange).length + Object.keys(colEq).length;
      barRight.appendChild(
        mkBtn(nAct ? `Filters · ${nAct}` : "Filters", "per-column filters", showFilters, () => {
          showFilters = !showFilters;
          persist();
          renderFilterRow();
          renderBar();
        })
      );
    }
    if (!totalsOff && cols.some((c) => info[c].numeric && !info[c].label) && nTotal() > 1)
      barRight.appendChild(
        mkBtn(
          "Σ Totals",
          `a totals row over all ${server ? "matching rows (computed in SQL)" : "matching rows"}: sums of amounts and counts, means of rates — click a total's aggregation to change it`,
          showTotals,
          () => {
            showTotals = !showTotals;
            persist();
            renderCols();
            renderTotals();
            renderBar();
          }
        )
      );
    if (nTotal() > 1)
      barRight.appendChild(
        mkBtn("Stats", "column statistics under the header: each number column's distribution, each text column's top values", showStats, () => {
          showStats = !showStats;
          persist();
          renderStatsRow();
          renderBar();
        })
      );
    if (cols.length > 4) barRight.appendChild(columnsMenu());
    if (many) {
      const seg = document.createElement("span");
      seg.className = "tbl-seg";
      for (const [v, label, tip] of [
        ["pages", "Pages", "page through the rows"],
        ["scroll", "Scroll", "scroll through every row (only the visible ones are drawn" + (server ? ", fetched in blocks)" : ")")],
      ])
        seg.appendChild(
          mkBtn(label, tip, view === v, () => {
            if (view === v) return;
            if (server) {
              server.onView(v);
              return;
            }
            view = v;
            page = 0;
            persist();
            setView();
            body();
            renderBar();
          })
        );
      barRight.appendChild(seg);
    }
    barRight.appendChild(
      mkBtn(compact ? "Compact" : "Comfortable", "row density: compact ↔ comfortable", false, () => {
        compact = !compact;
        ROW_H = compact ? ROW_H_COMPACT : ROW_H_COMFY;
        t.classList.toggle("compact", compact);
        persist();
        if (view === "scroll") body();
        renderBar();
      })
    );
    barRight.appendChild(exportMenu());
  }
  function columnsMenu() {
    const d = document.createElement("details");
    d.className = "tbl-cols";
    d.onclick = (e) => e.stopPropagation();
    const s = document.createElement("summary");
    s.className = "tbl-btn" + (hidden.size ? " on" : "");
    const lab = () => (hidden.size ? `Columns · ${cols.length - hidden.size}/${cols.length}` : "Columns");
    s.textContent = lab();
    s.title = "show / hide columns";
    const pop = document.createElement("div");
    pop.className = "tbl-cols-pop";
    cols.forEach((c, ix) => {
      const l = document.createElement("label");
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.checked = !hidden.has(c);
      cb.disabled = ix === 0; // the row key stays
      cb.onchange = () => {
        if (cb.checked) hidden.delete(c);
        else hidden.add(c);
        persist();
        renderHead();
        renderStatsRow();
        renderFilterRow();
        body();
        s.textContent = lab();
        s.classList.toggle("on", hidden.size > 0);
      };
      l.append(cb, document.createTextNode(head(c)));
      pop.appendChild(l);
    });
    d.append(s, pop);
    return d;
  }
  // ⤓ Export: CSV (raw values) or Excel (formatted like the table)
  function exportMenu() {
    const d = document.createElement("details");
    d.className = "tbl-cols tbl-export";
    d.onclick = (e) => e.stopPropagation();
    const s = document.createElement("summary");
    s.className = "tbl-btn";
    s.textContent = "⤓ Export";
    s.title = `download the ${server ? "loaded" : "matching"} rows`;
    const pop = document.createElement("div");
    pop.className = "tbl-cols-pop";
    for (const [b, label] of [
      [csvBtn(), "CSV — raw values"],
      [xlsxBtn(), "Excel — formatted like the table"],
    ]) {
      b.className = "tbl-mi";
      b.textContent = label;
      const go = b.onclick;
      b.onclick = (e) => {
        d.open = false;
        go(e);
      };
      pop.appendChild(b);
    }
    d.append(s, pop);
    return d;
  }
  // CSV of what the table shows: the matching rows (a ::PAGED table: the rows
  // loaded so far) and the visible columns, in table order.
  function csvBtn() {
    return mkBtn(
      "⤓ CSV",
      server ? "download the loaded rows as CSV" : "download the matching rows as CSV",
      false,
      () => {
        const data = server ? (view === "scroll" ? loadedRows() : rows) : sortedRows();
        download(new Blob([csvOfRows(data, visCols())], { type: "text/csv" }), "data.csv");
      }
    );
  }
  // Excel: the same rows as the CSV, but as the table reads — headers with their
  // units, the grouped header, each column's number format (units by scaling,
  // the raw value kept), dates as dates, subtotal and totals rows as live
  // SUBTOTAL() formulas (a ::PAGED table's totals: the SQL values), a frozen
  // header with a filter.
  const xlFmt = (c) => {
    const i = info[c];
    if (!i.numeric) return null;
    if (i.label) return "0";
    const neg = (pos) => (negStyle === "parens" ? `${pos};(${pos})` : negStyle === "red" ? `${pos};[Red]-${pos}` : pos);
    const dec = (n) => (n > 0 ? "." + "0".repeat(n) : "");
    const lit = (t) => (t ? `"${t.replace(/"/g, "")}"` : "");
    const u = unitOf[c];
    if (u) {
      const scale = { 1: "", 1e3: ",", 1e6: ",,", 1e9: ",,," }[u.div] ?? "";
      return neg(u.inHead ? `#,##0${dec(u.dec)}${scale}` : `${lit(u.pre)}#,##0${dec(u.dec)}${scale}${lit(u.word + u.post)}`);
    }
    if (i.f === "PERCENT") return neg('#,##0.0"%"');
    if (i.f === "TREND") return "+0.0;-0.0;0.0";
    if (i.f === "METRIC") return neg("#,##0.00");
    if (String(i.f).startsWith("COLORSCALE")) return neg("#,##0.0#");
    return neg(`#,##0${dec(i.dec)}`);
  };
  const xlDate = (t) => {
    const l = String(dashLocale() || "");
    const d = /^de|^ru|^pl/.test(l) ? "dd.mm.yyyy" : /^en-US/.test(l) ? "mm/dd/yyyy" : /^(en|fr|es|it|pt)/.test(l) ? "dd/mm/yyyy" : "yyyy-mm-dd";
    return /[ T]\d/.test(t) ? `${d} hh:mm` : d;
  };
  function xlsxBtn() {
    return mkBtn(
      "Excel",
      `download the ${server ? "loaded" : "matching"} rows as an Excel workbook, formatted like the table`,
      false,
      () => {
        const vc = visCols().filter((c) => info[c].f !== "SPARKLINE");
        const data = server ? (view === "scroll" ? loadedRows() : rows) : sortedRows();
        const fmts = Object.fromEntries(vc.map((c) => [c, xlFmt(c)]));
        const present = (c) => data.map((r) => r[c]).filter((v) => v != null && unquote(v) !== "");
        const isDate = Object.fromEntries(
          vc.map((c) => [c, !info[c].numeric && !info[c].bool && present(c).length > 0 && present(c).every((v) => excelDate(unquote(v)) != null)])
        );
        const out = [];
        const merges = [];
        if (Object.keys(groupOf).length) {
          const g = vc.map((c) => groupOf[c] ?? "");
          const row = g.map(() => null);
          for (let k = 0; k < g.length; ) {
            let e = k;
            while (e + 1 < g.length && g[e + 1] === g[k]) e++;
            if (g[k]) {
              row[k] = { v: g[k], style: "group" };
              if (e > k) merges.push(`${xlsxCol(k)}1:${xlsxCol(e)}1`);
            }
            k = e + 1;
          }
          out.push(row);
        }
        out.push(vc.map((c) => ({ v: head(c), style: "head" })));
        const hdr = out.length;
        const cellOf = (v, c) => {
          if (v == null || unquote(v) === "") return null;
          const i = info[c];
          if (i.bool) return v === true || v === "true";
          if (i.numeric) {
            const n = cleanNum(v);
            return n == null ? unquote(v) : { v: n, fmt: fmts[c] };
          }
          if (isDate[c]) return { v: excelDate(unquote(v)), fmt: xlDate(unquote(v)) };
          return unquote(v);
        };
        const FN = { sum: 109, mean: 101, count: 102, max: 104, min: 105 };
        const aggable = (c) => info[c].numeric && !info[c].label && info[c].f !== "BADGE" && aggOf(c) !== "none";
        // a subtotal / total cell: the value, and (client tables) the live formula
        const aggCell = (c, from, to, v, style, nested) => {
          const a = aggOf(c);
          const col = xlsxCol(vc.indexOf(c));
          const f = server ? null : a === "median" ? (nested ? null : `MEDIAN(${col}${from}:${col}${to})`) : `SUBTOTAL(${FN[a]},${col}${from}:${col}${to})`;
          // (a mean / median of whole numbers keeps one decimal, as in the table)
          const whole = (!info[c].f || info[c].f === "PLAIN") && info[c].dec === 0 && !info[c].label;
          const fmt = a === "count" ? "#,##0" : (a === "mean" || a === "median") && whole ? "#,##0.0" : fmts[c];
          return { v: v ?? "", f, fmt, style };
        };
        let gStart = out.length + 1;
        data.forEach((r, j) => {
          out.push(vc.map((c) => cellOf(r[c], c)));
          const k = closesGroup(data, j);
          if (k === undefined) return;
          const grp = subGroups.get(k) || [];
          const to = out.length;
          out.push(
            vc.map((c, ix) =>
              ix === 0 ? { v: `Subtotal ${k}`, style: "sub" } : aggable(c) ? aggCell(c, gStart, to, aggValue(c, aggOf(c), grp), "sub", false) : { v: "", style: "sub" }
            )
          );
          gStart = out.length + 1;
        });
        const last = Math.max(out.length, hdr + 1);
        if (showTotals && !totalsOff && data.length)
          out.push(
            vc.map((c, ix) =>
              ix === 0
                ? { v: server ? `Total (all ${nTotal().toLocaleString(loc)} rows)` : "Total", style: "tot" }
                : aggable(c)
                  ? aggCell(c, hdr + 1, last, aggValue(c, aggOf(c)), "tot", !!subCol)
                  : { v: "", style: "tot" }
            )
          );
        const name = (opts.caption && opts.caption !== "Data table" ? opts.caption : "Data").split(/\s+[—–-]\s+/)[0];
        const file = name.replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-|-$/g, "").toLowerCase().slice(0, 60) || "data";
        const bytes = xlsxOf({
          name,
          cols: vc.map((c) => ({ width: Math.min(60, Math.max(8, (natW?.[c] ?? 10) + 3, Math.ceil(chars(head(c)) / 2) + 3)) })),
          rows: out,
          freeze: hdr,
          filter: data.length ? `A${hdr}:${xlsxCol(vc.length - 1)}${last}` : null,
          merges,
        });
        download(new Blob([bytes], { type: XLSX_TYPE }), `${file}.xlsx`);
      }
    );
  }
  wrap.appendChild(bar);

  // ── head: label, sort icon, column statistics on hover ──
  const scroller = document.createElement("div");
  scroller.className = "tbl-scroll";
  scroller.appendChild(t);
  wrap.appendChild(scroller);
  const legend = document.createElement("div"); // what the totals are, under the table
  legend.className = "tot-legend";
  legend.hidden = true;
  wrap.appendChild(legend);
  // a caption names the table for assistive tech (the panel title shows above it)
  const cap = t.createCaption();
  cap.className = "tbl-cap";
  cap.textContent = opts.caption || "Data table";
  const thead = t.createTHead();
  const hr = thead.insertRow();
  let gRow = null; // the ::COLGROUP row above the header
  const statsTitle = (c) => {
    const i = info[c];
    const parts = [];
    if (i.numeric && !i.label && i.mean != null && !["SPARKLINE", "BADGE"].includes(i.f)) {
      parts.push(`min ${cellText(i.min, c)}`, `max ${cellText(i.max, c)}`, `mean ${cellText(i.mean, c)}`);
    } else if (i.f !== "SPARKLINE") parts.push(`${i.distinct.toLocaleString(loc)} distinct`);
    if (i.empty) parts.push(`${i.empty.toLocaleString(loc)} empty`);
    const scope = server ? " (all matching rows)" : "";
    const name = head(c) === c ? c : `${head(c)}  (${c})`;
    return `${name}\n${parts.join(" · ")}${scope}\nclick to sort`;
  };
  const alignRight = (c) => info[c].numeric && !["BADGE", "SPARKLINE"].includes(info[c].f);
  // ── natural column widths, in ch of the table font: the header (as wrapped
  // onto two lines) and the formatted cells, measured once per render over all
  // rows (a sample of long tables; a ::PAGED table: the loaded page) so paging
  // and scrolling never shift them. Long text is capped (it wraps); the slack of
  // a wide card is shared in proportion, never absorbed by one column.
  const PAD_CH = 3.5; // the cell's horizontal padding
  let natW = null;
  const chars = (x) => [...String(x)].length;
  const measure = () => {
    const step = Math.max(1, Math.ceil(rows.length / 400));
    const sample = rows.length > 400 ? rows.filter((_, k) => k < 200 || k % step === 0) : rows;
    natW = {};
    for (const c of cols) {
      const i = info[c];
      let w = 0;
      if (i.f === "SPARKLINE") w = 15;
      else
        for (const r of sample) {
          const v = r[c];
          if (v == null || unquote(v) === "") continue;
          if (i.bool) w = Math.max(w, 2);
          else if (i.f === "BADGE") w = Math.max(w, chars(unquote(v)) + 4); // dot + pill padding
          else if (i.f === "TREND") w = Math.max(w, chars(cellText(v, c)) + 3); // ▲ +
          else if (String(i.f).startsWith("COLORSCALE")) w = Math.max(w, chars(cellText(v, c)) + 3, 7);
          else w = Math.max(w, chars(cellText(v, c)));
        }
      if (!i.numeric) w = Math.min(w, i.longText ? 44 : 32); // long text wraps
      if (c === cols[0] && !i.numeric) w *= 1.08; // the key column is bold
      if (flags[c]) w += 2.5;
      if (hlSpec[c]) w += 3;
      if (subCol && c === cols[0])
        for (const r of sample) w = Math.max(w, chars("Subtotal ") * 0.9 + chars(groupKey(r)) * 1.05);
      natW[c] = w;
    }
  };
  function renderCols() {
    if (!natW) measure();
    t.querySelector("colgroup")?.remove();
    const ws = visCols().map((c) => {
      // the header wraps onto at most two lines (smaller type), plus the sort icon
      const h = head(c);
      const hw = Math.max(...h.split(/\s+/).map(chars), Math.ceil(chars(h) / 2)) * 0.9 + 2;
      let w = Math.max(natW[c], hw, 4);
      // a shown total: its aggregation tag and value
      const i = info[c];
      if (showTotals && !totalsOff && i.numeric && !i.label && i.f !== "SPARKLINE" && c !== cols[0]) {
        const a = aggOf(c);
        const v = a === "none" ? null : aggValue(c, a);
        if (v != null) w = Math.max(w, chars(aggText(v, c, a)) * 1.05 + chars(a) * 0.8 + 2);
      }
      return w + PAD_CH;
    });
    const total = ws.reduce((a, b) => a + b, 0);
    // the narrowest the table may get before it scrolls sideways: long text
    // (messages, descriptions) wraps down to ~24ch first
    const LONG_MIN = 24 + PAD_CH;
    const minTotal = visCols().reduce((acc, c, k) => acc + (info[c].longText ? Math.min(ws[k], LONG_MIN) : ws[k]), 0);
    const cg = document.createElement("colgroup");
    // with long text: the other columns keep their natural width and the long
    // ones share what is left (they wrap); else the slack is shared in proportion
    const long = visCols().map((c) => info[c].longText);
    const hasLong = long.some(Boolean);
    ws.forEach((w, k) => {
      const col = document.createElement("col");
      if (!hasLong) col.style.width = `${((100 * w) / total).toFixed(2)}%`;
      else if (!long[k]) col.style.width = `${w.toFixed(1)}ch`;
      cg.appendChild(col);
    });
    t.insertBefore(cg, t.tHead);
    // never narrower than the content (the card scrolls sideways instead), and
    // at most ~1.6× its natural width, so a few columns don't spread over a wide card
    t.style.minWidth = `${minTotal.toFixed(1)}ch`;
    wrap.style.maxWidth = `max(44rem, ${(total * 1.6 * 0.84).toFixed(1)}ch)`;
  }
  function renderHead() {
    renderCols();
    hr.innerHTML = "";
    gRow?.remove();
    gRow = null;
    if (Object.keys(groupOf).length) {
      gRow = thead.insertRow(0);
      gRow.className = "th-group";
      for (const c of visCols()) {
        const g = groupOf[c] ?? "";
        const last = gRow.lastElementChild;
        if (last && last.dataset.g === g) {
          last.colSpan += 1;
          continue;
        }
        const th = document.createElement("th");
        th.dataset.g = g;
        if (g) {
          th.scope = "colgroup";
          th.innerHTML = `<span class="th-g">${escapeHtml(g)}</span>`;
        }
        gRow.appendChild(th);
      }
    }
    for (const c of visCols()) {
      const th = document.createElement("th");
      th.title = statsTitle(c);
      th.scope = "col";
      if (alignRight(c)) th.classList.add("num");
      const active = c === sortCol;
      th.classList.toggle("sorted", active);
      if (active) th.setAttribute("aria-sort", dir > 0 ? "ascending" : "descending");
      th.innerHTML =
        `<span class="th-in"><span class="th-l">${escapeHtml(head(c))}</span>` +
        `<span class="th-sort">${active ? (dir > 0 ? "↑" : "↓") : "↕"}</span></span>`;
      th.onclick = (e) => {
        if (server) {
          e.stopPropagation();
          server.onSort(c);
          return;
        }
        dir = sortCol === c ? -dir : 1;
        sortCol = c;
        persist();
        renderHead();
        body();
      };
      hr.appendChild(th);
    }
  }

  // ── the column-stats strip (behind the Stats button): a 10-bin histogram of
  // each number column, the top 3 values (with their share) of the others —
  // over the matching rows; a ::PAGED table's come from SQL (server.profile) ──
  let sRow = null;
  const BINS = 10;
  const profileOf = (c, prof) => {
    const i = info[c];
    const num = i.numeric && !i.label;
    if (server) {
      const j = server.statCols ? server.statCols.indexOf(c) : -1;
      const st = sstat(c);
      if (j < 0 || !prof) return null;
      const mine = prof.filter((r) => Number(r.c) === j);
      const hist = new Array(BINS).fill(0);
      for (const r of mine) if (r.t === "h") hist[Math.min(BINS - 1, Number(r.k) || 0)] += Number(r.n);
      const top = mine.filter((r) => r.t === "t").map((r) => [unquote(r.v), Number(r.n)]);
      return { num, hist, lo: st?.lo, hi: st?.hi, top, n: st?.n ?? 0, distinct: st?.d ?? top.length };
    }
    const vals = filtered.map((r) => r[c]).filter((v) => v != null && unquote(v) !== "");
    if (num) {
      const xs = vals.map(cleanNum).filter((v) => v != null);
      const lo = xs.length ? Math.min(...xs) : null;
      const hi = xs.length ? Math.max(...xs) : null;
      const hist = new Array(BINS).fill(0);
      for (const x of xs) hist[hi > lo ? Math.min(BINS - 1, Math.floor(((x - lo) / (hi - lo)) * BINS)) : 0]++;
      return { num, hist, lo, hi, n: xs.length };
    }
    const cnt = new Map();
    for (const v of vals) cnt.set(unquote(v), (cnt.get(unquote(v)) || 0) + 1);
    const top = [...cnt].sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0]))).slice(0, 3);
    return { num, top, n: vals.length, distinct: cnt.size };
  };
  const pctTxt = (x) => new Intl.NumberFormat(loc, { style: "percent", maximumFractionDigits: x < 0.1 ? 1 : 0 }).format(x);
  function statCell(c, p) {
    const i = info[c];
    if (!p || i.f === "SPARKLINE") return "";
    const empty = (server ? server.total || 0 : filtered.length) - p.n; // (a number column: values that aren't numbers too)
    const foot = empty > 0 ? `<span class="st-empty">${empty.toLocaleString(loc)} empty</span>` : "";
    if (p.num) {
      const mx = Math.max(...p.hist, 1);
      const w = 100 / BINS;
      const step = p.hi > p.lo ? (p.hi - p.lo) / BINS : 0;
      const bars = p.hist
        .map((n, k) => {
          const h = n ? Math.max(1.5, (22 * n) / mx) : 0;
          const range = step ? `${cellText(p.lo + k * step, c)} – ${cellText(p.lo + (k + 1) * step, c)}` : cellText(p.lo, c);
          return `<rect x="${(k * w + 0.6).toFixed(2)}" y="${(24 - h).toFixed(2)}" width="${(w - 1.2).toFixed(2)}" height="${h.toFixed(2)}"><title>${escapeHtml(range)}: ${n.toLocaleString(loc)}</title></rect>`;
        })
        .join("");
      const ends =
        p.lo == null ? "" : `<span class="st-ends"><span>${escapeHtml(cellText(p.lo, c))}</span><span>${escapeHtml(cellText(p.hi, c))}</span></span>`;
      return `<svg class="st-hist" viewBox="0 0 100 24" preserveAspectRatio="none" role="img" aria-label="distribution of ${escapeHtml(head(c))}">${bars}</svg>${ends}${foot}`;
    }
    if (!p.top || !p.top.length) return foot;
    // every value once (ids, names): shares say nothing
    if (p.top[0][1] <= 1 && p.n > 1) return `<span class="st-more">all ${p.n.toLocaleString(loc)} values distinct</span>${foot}`;
    const distinct = Math.min(p.distinct, p.n); // (approx_count_distinct can overshoot)
    const lines = p.top
      .map(([v, n]) => {
        const sh = p.n ? n / p.n : 0;
        return `<span class="st-top" title="${escapeHtml(v)}: ${n.toLocaleString(loc)} (${pctTxt(sh)})"><span class="st-v">${escapeHtml(cellText(v, c) || v)}</span><span class="st-p">${pctTxt(sh)}</span><span class="st-bar" style="width:${(100 * sh).toFixed(1)}%"></span></span>`;
      })
      .join("");
    const more = distinct > p.top.length ? `<span class="st-more">+${(distinct - p.top.length).toLocaleString(loc)} more</span>` : "";
    return lines + more + foot;
  }
  function renderStatsRow() {
    if (sRow) sRow.remove();
    sRow = null;
    if (!showStats) return;
    sRow = thead.insertRow(hr.sectionRowIndex + 1);
    sRow.className = "tbl-stats";
    const fill = (prof) => {
      sRow.innerHTML = "";
      for (const c of visCols()) {
        const th = document.createElement("th");
        th.innerHTML = statCell(c, profileOf(c, prof));
        sRow.appendChild(th);
      }
    };
    if (!server) return fill(null);
    const row = sRow;
    fill(null);
    server
      .profile()
      .then((prof) => row === sRow && fill(prof))
      .catch(() => {});
  }

  // ── the per-column filter row (behind the Filters button) ──
  let fRow = null;
  function renderFilterRow() {
    if (fRow) fRow.remove();
    fRow = null;
    const total = server ? server.unfiltered ?? server.total : rows.length;
    if (!showFilters || total <= 10) return;
    fRow = thead.insertRow();
    fRow.className = "tbl-frow";
    for (const c of visCols()) {
      const th = document.createElement("th");
      const i = info[c];
      // a categorical column with few values → a dropdown (a ::PAGED table loads
      // its values from SQL: every distinct value under the other filters)
      const few = !i.numeric && i.f !== "SPARKLINE" && (colEq[c] != null || (i.distinct > 1 && i.distinct <= 30));
      if (few) {
        const sel = document.createElement("select");
        sel.className = "dp-colf";
        sel.onclick = (e) => e.stopPropagation();
        const fill = (vals) => {
          sel.innerHTML = `<option value="">All</option>`;
          for (const v of vals) {
            const o = document.createElement("option");
            o.value = v;
            o.textContent = v;
            sel.appendChild(o);
          }
          sel.value = colEq[c] || "";
        };
        if (server) {
          fill(colEq[c] ? [colEq[c]] : []);
          server
            .distinct(c)
            .then((vals) => fill(vals.filter((v) => v != null && v !== "")))
            .catch(() => {});
        } else fill([...new Set(rows.map((r) => unquote(r[c])))].filter((x) => x !== "").sort());
        sel.onchange = () => {
          if (sel.value) colEq[c] = sel.value;
          else delete colEq[c];
          filtersChanged();
        };
        th.appendChild(sel);
      } else if (i.f !== "SPARKLINE") {
        const inp = document.createElement("input");
        inp.className = "dp-colf";
        inp.placeholder = i.numeric ? "> n  < n" : "contains…";
        inp.value = colQ[c] || "";
        inp.onclick = (e) => e.stopPropagation();
        // a ::PAGED table is rebuilt after the query: give the focus back
        if (server && saved.focus === c) {
          requestAnimationFrame(() => {
            inp.focus();
            inp.setSelectionRange(inp.value.length, inp.value.length);
          });
          saved.focus = null;
        }
        inp.oninput = () => {
          const v = inp.value.trim();
          if (v) colQ[c] = v;
          else delete colQ[c];
          if (server) saved.focus = c;
          if (server) filtersChanged(true);
          else {
            applyFilters();
            page = 0;
            persist();
            body();
            renderStatsRow();
            renderBar();
          }
        };
        th.appendChild(inp);
        if (i.numeric && i.max > i.min) {
          const init = colRange[c] ? [colRange[c][0] ?? i.min, colRange[c][1] ?? i.max] : null;
          th.appendChild(
            dualRange(
              i.min,
              i.max,
              init,
              (lo, hi) => {
                if (lo == null && hi == null) delete colRange[c];
                else colRange[c] = [lo, hi];
                if (server) filtersChanged(true);
                else {
                  applyFilters();
                  page = 0;
                  persist();
                  body();
                  renderStatsRow();
                  renderBar();
                }
              },
              (v) => cellText(v, c)
            )
          );
        }
      }
      fRow.appendChild(th);
    }
  }

  // ── cells ──
  const tb = t.createTBody();
  const nil = () => `<span class="nil">—</span>`;
  function fillCell(td, v, c, row) {
    const i = info[c];
    const f = i.f;
    if (typeof v === "string" && /^"-?[\d.]+"$/.test(v)) v = v.slice(1, -1);
    if (f === "SPARKLINE") {
      td.className = "spark-cell";
      td.innerHTML = cellSpark(v, c);
      return;
    }
    if (v == null || unquote(v) === "") {
      td.innerHTML = nil();
      if (alignRight(c)) td.classList.add("num");
      return;
    }
    if (f === "BADGE") {
      td.innerHTML = badgeHtml(unq(v));
      return;
    }
    if (f === "LINK") {
      td.replaceChildren(linkCell(unquote(v), row || {}, row && k0 != null ? unquote(row[k0]) : ""));
      return;
    }
    if (i.bool) {
      const yes = v === true || v === "true";
      td.innerHTML = yes ? `<span class="bool yes" title="true">✓</span>` : `<span class="bool no" title="false">✗</span>`;
      td.classList.add("ctr");
      return;
    }
    if (!i.numeric) {
      td.textContent = cellText(v, c);
      if (i.longText) {
        td.classList.add("long");
        td.title = unquote(v);
      }
      return;
    }
    const n = cleanNum(v);
    td.classList.add("num");
    if (n == null) {
      td.textContent = unquote(v);
      return;
    }
    if (f === "TREND") {
      const flat = Math.abs(n) < 0.05; // rounds to 0.0: neither up nor down
      const up = n > 0;
      const txt = new Intl.NumberFormat(loc, { maximumFractionDigits: 1, minimumFractionDigits: 1 }).format(Math.abs(n));
      td.innerHTML = flat
        ? `<span class="trend flat">– ${escapeHtml(txt)}</span>`
        : `<span class="trend ${up ? "up" : "down"}">${up ? "▲ +" : "▼ \u2212"}${escapeHtml(txt)}</span>`;
      return;
    }
    if (String(f).startsWith("COLORSCALE")) {
      // low = green (::COLORSCALE), high = green (_HIGH_GOOD), or around zero (_DIVERGING)
      const tt = (n - i.min) / (i.max - i.min || 1);
      const div = f === "COLORSCALE_DIVERGING";
      const cs = div ? divColor(n / i.maxAbs) : heatColor(f === "COLORSCALE_HIGH_GOOD" ? 1 - tt : tt);
      const txt = new Intl.NumberFormat(loc, { maximumFractionDigits: 2 }).format(n);
      td.innerHTML = `<span class="cscale" style="--cs:${cs}">${escapeHtml(div && n > 0 ? "+" + txt : minus(txt))}</span>`;
      return;
    }
    td.textContent = cellText(n, c);
    if (n < 0 && negStyle === "red") td.classList.add("neg");
    if (n === 0) {
      td.classList.add("zero");
      if (zeroStyle === "dash") td.textContent = "–";
    }
    if (i.bars) {
      const frac = Math.max(0.02, Math.min(1, Math.abs(n) / i.maxAbs));
      const b = document.createElement("span");
      b.className = "cbar" + (n < 0 ? " neg" : "");
      b.style.width = `calc((100% - 1.5rem) * ${frac.toFixed(3)})`;
      td.appendChild(b);
    }
  }
  // ::FLAG: a ⚠ and the warning tint (never colour alone); ::HIGHLIGHT: the
  // rank of a top / bottom-n value, in bold
  let hlVals = {}; // column -> its values over all matching rows, best first
  const rankHighlights = () => {
    hlVals = {};
    const data = server ? rows : filtered;
    for (const [c, sp] of Object.entries(hlSpec))
      hlVals[c] = data
        .map((r) => cleanNum(r[c]))
        .filter((v) => v != null)
        .sort((a, b) => (sp.top ? b - a : a - b));
  };
  function decorate(td, v, c) {
    const th = thresholds[c] && thresholdRule(thresholds[c], v);
    if (th) {
      td.classList.add("th", `th-${th.status}`);
      td.title = `${th.status}: ${th.text}`;
      td.insertAdjacentHTML("afterbegin", `<span class="th-i" aria-label="${th.status}">${STATUS_ICON[th.status] || ""}</span>`);
    }
    const fl = flags[c] && flagOf(c, v);
    if (fl) {
      td.classList.add("flag");
      td.title = `flagged: ${fl.text}`;
      td.insertAdjacentHTML("afterbegin", `<span class="flag-i" aria-label="flagged">⚠</span>`);
    }
    const sp = hlSpec[c];
    const n = sp && cleanNum(v);
    if (n == null || !sp || !hlVals[c]) return;
    const better = hlVals[c].findIndex((x) => (sp.top ? x <= n : x >= n));
    const rk = (better < 0 ? hlVals[c].length : better) + 1;
    if (rk > sp.n) return;
    td.classList.add("hl", sp.top ? "hl-top" : "hl-bot");
    td.title ||= `${sp.top ? "top" : "bottom"} ${sp.n}: #${rk}${sp.top ? "" : " from the bottom"}${server ? " (on this page)" : ""}`;
    td.insertAdjacentHTML("afterbegin", `<span class="hl-r">${sp.top ? "#" : "↓"}${rk}</span>`);
  }
  // One body row. With ::ROWSELECT 'single' a click sets getvariable('selected')
  // and getvariable('<first column>') (click again / the background clears);
  // with 'multi' each click toggles the row in the list getvariable('<first column>').
  const k0 = cols[0];
  if (k0 != null) coerceSeededSelection(k0, selMode);
  function renderRow(tr, r, repeat = false) {
    const vc = visCols();
    const selectable = selMode !== "none" && k0 != null && !info[k0].numeric;
    if (selectable) {
      const keyVal = unquote(r[k0]);
      tr.classList.add("clickable");
      if (selMode === "single") {
        const own = dpXf[k0] !== undefined && !Array.isArray(dpXf[k0]) ? dpXf[k0] : dpSelected;
        if (own && keyVal === own) tr.classList.add("row-sel");
        tr.onclick = (e) => {
          if (document.body.classList.contains("mode-explore")) return; // preview tables aren't cross-filters
          e.stopPropagation();
          const on = (dpXf[k0] ?? "") !== keyVal;
          dpXf[k0] = on ? keyVal : "";
          dpFilter = on ? keyVal : "";
          dpSelected = dpFilter || null;
          run(false);
        };
      } else {
        const cur = Array.isArray(dpXf[k0]) ? dpXf[k0] : [];
        const on = cur.includes(keyVal);
        tr.classList.toggle("row-sel", on);
        tr.onclick = (e) => {
          if (document.body.classList.contains("mode-explore")) return;
          e.stopPropagation();
          // the selection as it is now (clicks can come faster than re-renders)
          const set = new Set(Array.isArray(dpXf[k0]) ? dpXf[k0] : []);
          if (set.has(keyVal)) set.delete(keyVal);
          else set.add(keyVal);
          dpXf[k0] = [...set];
          run(false);
        };
      }
    }
    if (selectable) tr.setAttribute("aria-selected", String(tr.classList.contains("row-sel")));
    vc.forEach((c, ix) => {
      const td = tr.insertCell();
      fillCell(td, r[c], c, r);
      decorate(td, r[c], c);
      if (repeat && c === subCol) td.classList.add("grp-rep"); // ::SUBTOTAL: the group's value once
      if (ix === 0 && selectable && selMode === "multi") {
        const cb = document.createElement("span");
        cb.className = "row-cb" + (tr.classList.contains("row-sel") ? " on" : "");
        td.prepend(cb);
      }
    });
  }

  // ── body: one page, or (Scroll view) the rows around the viewport ──
  const blocks = new Map(); // ::PAGED Scroll view: block index -> rows | Promise
  if (server && view === "scroll") blocks.set(0, rows);
  const loadedRows = () => [...blocks.keys()].sort((a, b) => a - b).flatMap((k) => (Array.isArray(blocks.get(k)) ? blocks.get(k) : []));
  const rowAt = (i) => {
    if (!server) return sortedCache[i];
    const b = Math.floor(i / SCROLL_BLOCK);
    const got = blocks.get(b);
    if (Array.isArray(got)) return got[i - b * SCROLL_BLOCK] ?? null;
    if (!got)
      blocks.set(
        b,
        server
          .fetchRows(b * SCROLL_BLOCK, SCROLL_BLOCK)
          .then((rs) => {
            blocks.set(b, rs);
            renderWindow(true); // the block arrived: redraw the rows it fills
          })
          .catch(() => blocks.delete(b))
      );
    return null; // still loading
  };
  const spacer = (h) => {
    const tr = document.createElement("tr");
    tr.className = "tbl-spacer";
    const td = document.createElement("td");
    td.colSpan = visCols().length;
    td.style.height = `${h}px`;
    tr.appendChild(td);
    return tr;
  };
  let lastWindow = "";
  function renderWindow(force = false) {
    const total = server ? nTotal() : sortedCache.length; // (with ::SUBTOTAL rows)
    const viewH = scroller.clientHeight || 560;
    const first = Math.max(0, Math.floor(scroller.scrollTop / ROW_H) - 8);
    const last = Math.min(total, first + Math.ceil(viewH / ROW_H) + 16);
    const sig = `${first}:${last}:${blocks.size}`;
    if (!force && sig === lastWindow) return;
    lastWindow = sig;
    tb.innerHTML = "";
    if (!total) return emptyRow();
    tb.appendChild(spacer(first * ROW_H));
    for (let i = first; i < last; i++) {
      const r = rowAt(i);
      const tr = document.createElement("tr");
      const prev = i > first ? rowAt(i - 1) : null;
      if (r && r[SUB] !== undefined) renderSubRow(tr, r[SUB]);
      else if (r) renderRow(tr, r, !!subCol && !!prev && prev[SUB] === undefined && groupKey(prev) === groupKey(r));
      else {
        tr.className = "tbl-loading";
        const td = tr.insertCell();
        td.colSpan = visCols().length;
        td.textContent = "…";
      }
      tb.appendChild(tr);
    }
    tb.appendChild(spacer((total - last) * ROW_H));
    updateFoot(nTotal(), 1);
  }
  const emptyRow = () => {
    const tr = tb.insertRow();
    const td = tr.insertCell();
    td.colSpan = visCols().length || 1;
    td.className = "tbl-none";
    td.textContent = server ? "No rows match." : "No rows match the filters.";
  };
  let rafPending = false;
  scroller.addEventListener("scroll", () => {
    if (view !== "scroll" || rafPending) return;
    rafPending = true;
    requestAnimationFrame(() => {
      rafPending = false;
      renderWindow();
    });
  });
  function setView() {
    scroller.classList.toggle("virtual", view === "scroll");
    t.classList.toggle("virtual", view === "scroll");
    if (view === "scroll") scroller.scrollTop = 0;
  }
  let printing = false; // print / PDF: a client table shows every matching row
  const body = () => {
    tb.innerHTML = "";
    rankHighlights();
    if (subCol) {
      subGroups = new Map();
      for (const r of filtered) {
        const k = groupKey(r);
        if (!subGroups.has(k)) subGroups.set(k, []);
        subGroups.get(k).push(r);
      }
    }
    if (view === "scroll" && !(printing && !server)) {
      sortedCache = server ? null : sortedRows();
      if (subCol) sortedCache = sortedCache.flatMap((r, j, all) => (closesGroup(all, j) === undefined ? [r] : [r, { [SUB]: groupKey(r) }]));
      renderWindow(true);
      renderTotals();
      return;
    }
    const data = sortedRows();
    if (server) {
      updateFoot(server.total, Math.max(1, Math.ceil(server.total / pageSize)));
    } else {
      const pages = Math.max(1, Math.ceil(data.length / pageSize));
      page = Math.min(page, pages - 1);
      updateFoot(data.length, pages);
    }
    const from = server || printing ? 0 : page * pageSize;
    const pageRows = server || printing ? data : data.slice(from, from + pageSize);
    // a long page scrolls inside the table, so the header and the totals stay in view
    scroller.classList.toggle("long", view !== "scroll" && !printing && pageRows.length > 20);
    if (!pageRows.length) emptyRow();
    pageRows.forEach((r, j) => {
      renderRow(tb.insertRow(), r, !!subCol && j > 0 && groupKey(pageRows[j - 1]) === groupKey(r));
      const k = closesGroup(data, from + j);
      if (k !== undefined) renderSubRow(tb.insertRow(), k);
    });
    renderTotals();
  };

  // ── totals row: per numeric column one aggregation — the viewer's pick in the
  // cell, else ::TOTALS, else Σ for amounts and counts / mean for rates; counts
  // for badges. A ::PAGED table's come from the SQL statistics (n, s, m, md, lo, hi).
  const aggOf = (c) => aggPick[c] ?? totSpec.col[c] ?? totSpec.all ?? (additive(c) ? "sum" : "mean");
  const aggValue = (c, a, of = null) => {
    if (server && !of) {
      const st = sstat(c);
      return st ? { sum: st.s, mean: st.m, median: st.md, min: st.lo, max: st.hi, count: st.n }[a] : null;
    }
    const nums = (of || filtered).map((r) => cleanNum(r[c])).filter((v) => v != null);
    if (a === "count") return nums.length;
    if (!nums.length) return null;
    if (a === "min" || a === "max") return nums.reduce((x, y) => (a === "min" ? Math.min(x, y) : Math.max(x, y)));
    if (a === "median") {
      nums.sort((x, y) => x - y);
      const h = nums.length >> 1;
      return nums.length % 2 ? nums[h] : (nums[h - 1] + nums[h]) / 2;
    }
    const sum = nums.reduce((x, y) => x + y, 0);
    return a === "sum" ? sum : sum / nums.length;
  };
  const aggText = (v, c, a) => {
    const i = info[c];
    if (a === "count") return v.toLocaleString(loc);
    if (i.f === "TREND" || String(i.f).startsWith("COLORSCALE"))
      return minus(new Intl.NumberFormat(loc, { maximumFractionDigits: 2 }).format(v));
    // a mean / median of whole numbers keeps one decimal
    if ((a === "mean" || a === "median") && (!i.f || i.f === "PLAIN") && i.dec === 0 && !Number.isInteger(v))
      return minus(new Intl.NumberFormat(loc, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(v));
    return cellText(v, c);
  };
  // ::SUBTOTAL: the group's row, each numeric column with its total's aggregation
  function renderSubRow(tr, k) {
    const grp = subGroups.get(k) || [];
    tr.className = "sub-row";
    visCols().forEach((c, ix) => {
      const td = tr.insertCell();
      const i = info[c];
      if (ix === 0) {
        td.innerHTML = `<span class="tot-l">Subtotal</span> ${k === "" ? nil() : escapeHtml(k)}`;
        td.title = `${head(subCol)}: ${k || "(empty)"} — ${grp.length.toLocaleString(loc)} matching rows`;
        return;
      }
      if (!i.numeric || i.label || ["SPARKLINE", "BADGE"].includes(i.f)) return;
      const a = aggOf(c);
      if (a === "none") return;
      const v = aggValue(c, a, grp);
      td.classList.add("num");
      if (v != null && v < 0 && negStyle === "red") td.classList.add("neg");
      td.textContent = v == null ? "–" : aggText(v, c, a);
      td.title = `${head(c)}: ${AGG_WORD[a]} over the ${grp.length.toLocaleString(loc)} rows of ${k || "(empty)"}`;
    });
  }
  let tfoot = null;
  function renderTotals() {
    if (tfoot) tfoot.remove();
    tfoot = null;
    legend.hidden = true;
    if (!showTotals || totalsOff) return;
    const data = server ? [] : filtered;
    tfoot = t.createTFoot();
    const tr = tfoot.insertRow();
    const n = nTotal().toLocaleString(loc);
    const byAgg = {}; // aggregation -> the headers showing it (the legend)
    visCols().forEach((c, ix) => {
      const td = tr.insertCell();
      const i = info[c];
      if (ix === 0) {
        td.innerHTML = `<span class="tot-l">Total</span>`;
        td.title = `over all ${n} matching rows${server ? " (computed in SQL)" : ""}; pick each column's aggregation in its cell`;
        return;
      }
      if (i.f === "BADGE" && !server) {
        const cnt = {};
        for (const r of data) if (r[c] != null) cnt[unq(r[c])] = (cnt[unq(r[c])] || 0) + 1;
        td.innerHTML = Object.entries(cnt)
          .sort((a, b) => b[1] - a[1])
          .slice(0, 3)
          .map(([k, n]) => `<span class="tot-badge">${n}× ${escapeHtml(k)}</span>`)
          .join(" ");
        td.classList.add("tot-badges"); // wraps rather than widening the column
        return;
      }
      if (!i.numeric || i.label || i.f === "SPARKLINE") return;
      const a = aggOf(c);
      if (a === "none") return;
      const v = aggValue(c, a);
      td.classList.add("num");
      // the aggregation: a quiet small-caps label that opens a menu previewing
      // every aggregation's value for this column
      const w = document.createElement("button");
      w.type = "button";
      w.className = "tot-agg";
      w.textContent = a;
      w.title = `${head(c)}: choose what the total shows`;
      w.setAttribute("aria-haspopup", "menu");
      w.setAttribute("aria-expanded", "false");
      w.onclick = (e) => {
        e.stopPropagation();
        const open = w.getAttribute("aria-expanded") === "true";
        document.querySelectorAll(".tot-menu").forEach((m) => m.closeMenu());
        if (open) return;
        wireTotMenuClose();
        const menu = document.createElement("div");
        menu.className = "tot-menu";
        menu.setAttribute("role", "menu");
        menu.closeMenu = () => {
          menu.remove();
          w.setAttribute("aria-expanded", "false");
        };
        for (const x of AGGS) {
          const xv = aggValue(c, x);
          const it = document.createElement("button");
          it.type = "button";
          it.className = "tot-mi" + (x === a ? " on" : "");
          it.setAttribute("role", "menuitemradio");
          it.setAttribute("aria-checked", String(x === a));
          it.innerHTML = `<span class="k">${AGG_LABEL[x]}</span><span class="v">${escapeHtml(xv == null ? "–" : aggText(xv, c, x))}</span>`;
          it.onclick = (ev) => {
            ev.stopPropagation();
            menu.closeMenu();
            aggPick[c] = x;
            persist();
            renderCols();
            if (subCol) body(); // the subtotals follow the pick
            else renderTotals();
          };
          menu.appendChild(it);
        }
        // on the page, not in the (clipping, scrolling) table: above the label,
        // or below it when there is no room
        document.body.appendChild(menu);
        const lb = w.getBoundingClientRect();
        const mb = menu.getBoundingClientRect();
        menu.style.left = `${Math.max(8, Math.min(lb.left, window.innerWidth - mb.width - 8))}px`;
        menu.style.top = `${lb.top - mb.height - 6 >= 8 ? lb.top - mb.height - 6 : lb.bottom + 6}px`;
        w.setAttribute("aria-expanded", "true");
        menu.querySelector(".tot-mi.on")?.focus();
      };
      const val = document.createElement("span");
      val.className = "tot-v";
      val.textContent = v == null ? "–" : aggText(v, c, a);
      if (v != null && v < 0 && negStyle === "red") td.classList.add("neg");
      td.title = `${head(c)}: ${AGG_WORD[a]} over ${n} rows`;
      td.append(w, val);
      (byAgg[a] ||= []).push(head(c));
    });
    // what each total is, in words
    const parts = Object.entries(byAgg).map(([a, hs]) => `${AGG_WORD[a]} of ${hs.join(", ")}`);
    legend.textContent = `Totals over all ${n} matching rows${server ? " (computed in SQL)" : ""}${parts.length ? ": " + parts.join(" · ") : ""}`;
    legend.hidden = false;
  }

  // ── pager (Pages view) / position (Scroll view) ──
  t._rows = rows;
  t._cols = cols;
  const foot = document.createElement("div");
  foot.className = "table-foot";
  wrap.appendChild(foot);
  function updateFoot(total, pages) {
    if (view === "scroll") {
      const first = Math.min(total, Math.floor(scroller.scrollTop / ROW_H) + 1);
      const last = Math.min(total, first - 1 + Math.floor((scroller.clientHeight || 560) / ROW_H));
      foot.style.display = total ? "" : "none";
      foot.innerHTML = `<span class="table-info">${first.toLocaleString(loc)}–${last.toLocaleString(loc)} of ${total.toLocaleString(loc)}</span>`;
      return;
    }
    // Hide the pager only for genuinely small tables (≤ the smallest page size).
    if (total <= 10) {
      foot.style.display = "none";
      return;
    }
    foot.style.display = "";
    foot.innerHTML = "";
    const cur = server ? server.page : page;
    const from = cur * pageSize + 1;
    const to = Math.min(total, (cur + 1) * pageSize);
    const mk = (label, title, disabled, fn) => {
      const btn = document.createElement("button");
      btn.className = "page-btn";
      btn.textContent = label;
      btn.title = title;
      btn.disabled = disabled;
      btn.onclick = (e) => {
        e.stopPropagation();
        fn();
        if (!server) {
          persist();
          body();
        }
      };
      return btn;
    };
    const go = (p) => (server ? server.onPage(p) : (page = p));
    const infoEl = document.createElement("span");
    infoEl.className = "table-info";
    infoEl.textContent = `${from.toLocaleString(loc)}–${to.toLocaleString(loc)} of ${total.toLocaleString(loc)}`;
    const sizeSel = document.createElement("select");
    sizeSel.className = "page-size";
    sizeSel.setAttribute("aria-label", "Rows per page");
    for (const nn of [10, 20, 50, 100]) {
      const o = document.createElement("option");
      o.value = String(nn);
      o.textContent = `${nn} / page`;
      if (nn === pageSize) o.selected = true;
      sizeSel.appendChild(o);
    }
    sizeSel.onclick = (e) => e.stopPropagation();
    sizeSel.onchange = (e) => {
      e.stopPropagation();
      const nn = Number(sizeSel.value);
      if (server) server.onPageSize(nn);
      else {
        pageSize = nn;
        page = 0;
        persist();
        body();
      }
    };
    foot.append(
      sizeSel,
      mk("«", "first page", cur === 0, () => go(0)),
      mk("‹", "previous page", cur === 0, () => go(Math.max(0, cur - 1))),
      infoEl,
      mk("›", "next page", cur >= pages - 1, () => go(Math.min(pages - 1, cur + 1))),
      mk("»", "last page", cur >= pages - 1, () => go(pages - 1))
    );
  }

  // Arrow-key navigation while the pointer is over the table: ↑/↓ move the
  // highlighted row, ←/→ (PageUp/Down) page (Scroll view: scroll a screen),
  // Enter selects (with ::ROWSELECT), Home/End jump.
  t.classList.add("dp-kbd");
  let focusIdx = -1;
  const bodyRows = () => [...tb.rows].filter((r) => !r.classList.contains("tbl-spacer"));
  const applyFocus = () => {
    bodyRows().forEach((r, i) => r.classList.toggle("row-focus", i === focusIdx));
    if (focusIdx >= 0) bodyRows()[focusIdx]?.scrollIntoView({ block: "nearest" });
  };
  const totalPages = () => Math.max(1, Math.ceil(nTotal() / pageSize));
  const gotoPage = (p) => {
    if (view === "scroll") {
      scroller.scrollTop += Math.sign(p - page) * (scroller.clientHeight - ROW_H);
      return;
    }
    p = Math.max(0, Math.min(totalPages() - 1, p));
    const cur = server ? server.page : page;
    if (p === cur) return;
    if (server) {
      dpKbdActive = true; // keep the highlight going on the reloaded table
      server.onPage(p);
    } else {
      page = p;
      focusIdx = 0;
      body();
      applyFocus();
    }
  };
  const controller = {
    move: (d) => {
      const n = bodyRows().length;
      if (!n) return;
      if (d === "home") focusIdx = 0;
      else if (d === "end") focusIdx = n - 1;
      else focusIdx = Math.max(0, Math.min(n - 1, (focusIdx < 0 ? 0 : focusIdx) + d));
      applyFocus();
    },
    page: (d) => gotoPage((server ? server.page : page) + d),
    drill: () => bodyRows()[focusIdx]?.click(),
  };
  t.addEventListener("mouseenter", () => {
    dpNav = controller;
    if (focusIdx < 0) {
      const sel = bodyRows().findIndex((r) => r.classList.contains("row-sel"));
      if (sel >= 0) {
        focusIdx = sel;
        applyFocus();
      }
    }
  });
  t.addEventListener("mouseleave", () => {
    if (dpNav === controller) dpNav = null;
  });
  if (server && dpKbdActive) {
    dpKbdActive = false;
    dpNav = controller;
    focusIdx = 0;
    requestAnimationFrame(applyFocus);
  }

  t.classList.toggle("compact", compact);
  // in-cell bars already guide the eye along a row: no zebra stripes then
  t.classList.toggle("bars", cols.some((c) => info[c].bars));
  if (!server)
    dpPrintHooks.add(function hook(on) {
      if (!wrap.isConnected) return dpPrintHooks.delete(hook);
      printing = on;
      body();
    });
  setView();
  renderHead();
  renderStatsRow();
  renderFilterRow();
  body();
  renderBar();
  return wrap;
}

// ::COLORSCALE cell colour — green→amber→red by the normalised value t∈[0,1]
// (low = green, high = red); the cell shows it as a soft tint (CSS --cs).
function heatColor(t) {
  t = Math.max(0, Math.min(1, t));
  // Theme tokens --ax-scale-lo/-mid/-hi (green / amber / red in the default look).
  const rgb = (name) => {
    const h = axToken(name).replace("#", "");
    return h.length === 6 ? [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) : [128, 128, 128];
  };
  const stops = [rgb("scale-lo"), rgb("scale-mid"), rgb("scale-hi")];
  const seg = t < 0.5 ? 0 : 1;
  const u = t < 0.5 ? t / 0.5 : (t - 0.5) / 0.5;
  const a = stops[seg];
  const b = stops[seg + 1];
  const m = (i) => Math.round(a[i] + (b[i] - a[i]) * u);
  return `rgb(${m(0)},${m(1)},${m(2)})`;
}

// ::COLORSCALE_DIVERGING cell colour — u ∈ [−1, 1]: the card colour at 0, the
// good end (--ax-scale-lo) above zero, the bad end (--ax-scale-hi) below.
function divColor(u) {
  u = Math.max(-1, Math.min(1, u || 0));
  const rgb = (name) => {
    const h = axToken(name).replace("#", "");
    return h.length === 6 ? [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) : [128, 128, 128];
  };
  const a = rgb("card");
  const b = rgb(u >= 0 ? "scale-lo" : "scale-hi");
  const m = (i) => Math.round(a[i] + (b[i] - a[i]) * Math.abs(u));
  return `rgb(${m(0)},${m(1)},${m(2)})`;
}

// ::BADGE — a coloured status pill; colour inferred from common status words
// (src/diagram.rs `badge_status` mirrors these lists for diagram nodes).
function badgeHtml(text) {
  const t = text.toLowerCase();
  let cls = "badge-neutral";
  // English and German status words (a dashboard may be localised)
  if (/\b(ok|good|on.?track|pass(ed)?|active|done|up|healthy|nominal|green|low|grow(ing|th)?|ris(e|ing)|improv(ed|ing)|correct|right|met|achieved|erreicht|gut|aktiv|erledigt)\b/.test(t))
    cls = "badge-good";
  else if (/\b(warn(ing)?|risk|at.?risk|pending|review|amber|medium|hold|watch|flat|stable|delayed|slow|knapp|prüfen|offen|verzögert)\b/.test(t))
    cls = "badge-warn";
  else if (/\b(bad|fail(ed)?|late|error|down|critical|red|overdue|stale|high|breach|alarm|alert|shrink(ing)?|declin(e|ing)|fall(ing)?|wrong|miss(ed)?|verfehlt|fehler|kritisch)\b/.test(t))
    cls = "badge-bad";
  return `<span class="badge ${cls}">${escapeHtml(text)}</span>`;
}

// ::SPARKLINE cell — a word-sized trend from a numeric array (DuckDB list()):
// a soft area under the line, the last point marked, and first → last / min /
// max on hover.
function cellSpark(v) {
  let arr = Array.isArray(v) ? v : null;
  if (!arr && typeof v === "string") {
    try {
      const p = JSON.parse(v);
      if (Array.isArray(p)) arr = p;
    } catch (_) {}
  }
  const nums = (arr || []).map(Number).filter((x) => isFinite(x));
  if (nums.length < 2) return `<span class="nil">—</span>`;
  const w = 104;
  const h = 26;
  const pad = 2.5;
  const mn = Math.min(...nums);
  const mx = Math.max(...nums);
  const rng = mx - mn || 1;
  const xs = (i) => pad + (i * (w - 2 * pad)) / (nums.length - 1);
  const ys = (y) => h - pad - ((y - mn) / rng) * (h - 2 * pad);
  const pts = nums.map((y, i) => `${xs(i).toFixed(1)},${ys(y).toFixed(1)}`).join(" ");
  const area = `${xs(0).toFixed(1)},${h} ${pts} ${xs(nums.length - 1).toFixed(1)},${h}`;
  const first = nums[0];
  const last = nums[nums.length - 1];
  const f = (n) => fmtNum(n, "COMPACT");
  const chg = first ? ` (${last >= first ? "+" : "−"}${Math.abs(((last - first) / Math.abs(first)) * 100).toFixed(0)}%)` : "";
  const tip = `${f(first)} → ${f(last)}${chg} · min ${f(mn)} · max ${f(mx)} · ${nums.length} points`;
  return (
    `<svg class="spark" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}"><title>${escapeHtml(tip)}</title>` +
    `<polygon points="${area}" fill="var(--ax-spark)" fill-opacity="0.1"/>` +
    `<polyline points="${pts}" fill="none" stroke="var(--ax-spark)" stroke-width="1.25" stroke-linejoin="round" stroke-linecap="round"/>` +
    `<circle cx="${xs(nums.length - 1).toFixed(1)}" cy="${ys(last).toFixed(1)}" r="2.2" fill="var(--ax-spark-dot)"/></svg>`
  );
}

// A KPI tile's ::SPARKLINE (a list): a trend line across the tile under the
// value, stretched to the tile's width (non-scaling strokes), the last point
// marked. "" without ≥ 2 numbers.
function kpiSpark(v) {
  let arr = Array.isArray(v) ? v : null;
  if (!arr && typeof v === "string") {
    try {
      const p = JSON.parse(v);
      if (Array.isArray(p)) arr = p;
    } catch (_) {}
  }
  const nums = (arr || []).filter((x) => x != null && x !== "").map(Number).filter((x) => isFinite(x));
  if (nums.length < 2) return "";
  const w = 200;
  const h = 36;
  const pad = 3;
  const mn = Math.min(...nums);
  const mx = Math.max(...nums);
  const rng = mx - mn || 1;
  const xs = (i) => (i * w) / (nums.length - 1);
  const ys = (y) => h - pad - ((y - mn) / rng) * (h - 2 * pad);
  const pts = nums.map((y, i) => `${xs(i).toFixed(1)},${ys(y).toFixed(1)}`).join(" ");
  const lx = xs(nums.length - 1).toFixed(1);
  const ly = ys(nums[nums.length - 1]).toFixed(1);
  const f = (n) => fmtNum(n, "COMPACT");
  const tip = `${f(nums[0])} → ${f(nums[nums.length - 1])} · min ${f(mn)} · max ${f(mx)} · ${nums.length} points`;
  return (
    `<svg class="metric-spark" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" role="img" aria-label="${escapeHtml(tip)}"><title>${escapeHtml(tip)}</title>` +
    `<polygon points="0,${h} ${pts} ${w},${h}" fill="var(--ax-spark)" fill-opacity="0.12"/>` +
    `<polyline points="${pts}" fill="none" stroke="var(--ax-spark)" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round" vector-effect="non-scaling-stroke"/>` +
    `<path d="M${lx} ${ly}h0" stroke="var(--ax-spark-dot)" stroke-width="5.5" stroke-linecap="round" vector-effect="non-scaling-stroke"/></svg>`
  );
}

// Number formatting locale — pinned (not the viewer's) so a dashboard reads the
// same for everyone: "$53.8T"/"1.92", never a locale-dependent "53,8 Bio.".
const NUM_LOCALE = "en-US";

// Format a KPI / table value. fmt: METRIC (plain), MONEY, PERCENT, COMPACT.
// Delegates to the Rust formatter (wasm `format_number`) so the browser and the
// headless renderer print identical numbers ("$12,400", "$53.8M", "1.2K", "46%");
// see src/format.rs for the spec + shared test vectors. A non-numeric value is
// shown verbatim (callers HTML-escape it).
function fmtNum(v, fmt) {
  if (v == null) return "–";
  const n = typeof v === "number" ? v : parseFloat(v);
  if (Number.isNaN(n)) return String(v);
  try {
    const loc = dashLocale();
    return loc ? format_number_locale(n, fmt || "METRIC", loc) : format_number(n, fmt || "METRIC");
  } catch (_) {
    return n.toLocaleString(NUM_LOCALE, { maximumFractionDigits: 2 });
  }
}

// A plain table cell in the dashboard locale: numbers grouped with its marks,
// ISO dates in its date order. Without a ::LOCALE the raw value is shown
// (unchanged from earlier releases).
function fmtCell(v) {
  const loc = dashLocale();
  if (!loc || v == null) return v == null ? "" : v;
  if (typeof v === "number") return format_number_locale(v, "METRIC", loc);
  const s = String(v);
  if (/^-?\d+(\.\d+)?$/.test(s)) return format_number_locale(parseFloat(s), "METRIC", loc);
  return format_date_locale(s, loc);
}

// Escape for HTML text AND attribute contexts (& < > " ').
function escapeHtml(s) {
  return String(s ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]
  );
}

// A URL safe to put in href/src: http(s), mailto, or relative (no scheme);
// images may also be data:image/*. Anything else (javascript:, vbscript:,
// data:text/html, …) returns "" — callers fall back to no link/image.
function safeUrl(u, { image = false } = {}) {
  const s = String(u ?? "").trim();
  // Browsers ignore ASCII whitespace/control chars inside a scheme ("java\tscript:").
  const probe = s.replace(/[\u0000-\u0020]/g, "").toLowerCase();
  const m = probe.match(/^([a-z][a-z0-9+.-]*):/);
  if (!m) return s.startsWith("//") ? "https:" + s : s; // relative (or protocol-relative)
  if (["http", "https", "mailto"].includes(m[1])) return s;
  if (image && /^data:image\/(png|jpe?g|gif|webp|svg\+xml);/.test(probe)) return s;
  return "";
}
// An image URL ::LOGO / ::FAVICON accept — the rules of safe_url in
// src/theme.rs (lint chrome/bad-url): relative, http(s): or data:image/, no
// quotes, brackets, backslashes or control characters, at most 512 000 long.
function chromeUrlOk(u) {
  const t = String(u ?? "").trim();
  if (!t || t.length > 512000 || /["'<>()\\`\u0000-\u001f\u007f-\u009f]/.test(t)) return false;
  const low = t.toLowerCase();
  if (low.startsWith("data:")) return low.startsWith("data:image/");
  const i = low.indexOf(":");
  return i < 0 || low.slice(0, i).includes("/") || low.startsWith("https:") || low.startsWith("http:");
}
// The SQL text with its `'…'::<roleName>` statement set to `value`: the first
// statement casting to the role is replaced (a string inside quotes doesn't
// count), else one is added at the end of the "-- ── Look" section (or after
// the leading comments). An empty value removes the statement.
function setChromeSql(sql, roleName, value) {
  const str = "'(?:[^']|'')*'";
  const re = new RegExp(`^[ \\t]*SELECT\\b(?:${str}|[^';])*?::\\s*${roleName}\\b(?:${str}|[^';])*(?:;|(?![\\s\\S]))`, "im");
  const stmt = value ? `SELECT '${String(value).replace(/'/g, "''")}'::${roleName};` : "";
  const m = sql.match(re);
  if (m) {
    const rest = sql.slice(m.index + m[0].length);
    return sql.slice(0, m.index) + (stmt || "") + (stmt ? rest : rest.replace(/^[ \t]*\r?\n?/, ""));
  }
  if (!stmt) return sql;
  const lines = sql.split("\n");
  let at = lines.findIndex((l) => /^\s*--\s*─+\s*Look\b/i.test(l));
  if (at >= 0) {
    at++;
    while (at < lines.length && lines[at].trim()) at++;
    lines.splice(at, 0, stmt);
    return lines.join("\n");
  }
  at = 0;
  while (at < lines.length && /^\s*(--.*)?$/.test(lines[at])) at++;
  while (at > 0 && !lines[at - 1].trim()) at--;
  const add = [stmt];
  if (at > 0) add.unshift("");
  if (at < lines.length && lines[at].trim()) add.push("");
  lines.splice(at, 0, ...add);
  return lines.join("\n");
}

// Text → [{text} | {tex, display}]: `$…$` inline and `$$…$$` display
// formulas, by the KaTeX auto-render / pandoc rules (src/math.rs `spans`, the
// same rules headless): an opening `$` needs a non-space after it, a closing
// one a non-space before it and no digit after it, so "$5 and $10" stays text;
// an inline formula ends at its first unescaped `$`. `\$` is a literal
// dollar; a `code` span is never math.
function mathSpans(s) {
  const out = [];
  let text = "",
    i = 0;
  const flush = () => {
    if (text) out.push({ text });
    text = "";
  };
  // the closing $ of an inline formula opening at j0: the next unescaped $,
  // if it may close — otherwise that $ is no formula
  const close = (j0) => {
    if (s.length - j0 < 3 || /[\s$]/.test(s[j0 + 1])) return -1;
    let j = j0 + 2;
    while (j < s.length && !(s[j] === "$" && s[j - 1] !== "\\")) j++;
    return j < s.length && !/\s/.test(s[j - 1]) && !/[0-9$]/.test(s[j + 1] ?? "") ? j : -1;
  };
  while (i < s.length) {
    if (s.startsWith("\\$", i)) {
      text += "$";
      i += 2;
    } else if (s[i] === "`") {
      const e = s.indexOf("`", i + 1);
      const end = e < 0 ? i + 1 : e + 1;
      text += s.slice(i, end);
      i = end;
    } else if (s.startsWith("$$", i)) {
      const e = s.indexOf("$$", i + 2);
      const tex = e < 0 ? "" : s.slice(i + 2, e).trim();
      if (tex) {
        flush();
        out.push({ tex, display: true });
        i = e + 2;
      } else {
        text += "$$";
        i += 2;
      }
    } else if (s[i] === "$" && close(i) > 0) {
      const e = close(i);
      flush();
      out.push({ tex: s.slice(i + 1, e), display: false });
      i = e + 1;
    } else text += s[i++];
  }
  flush();
  return out;
}

// KaTeX, loaded from the CDN on the first formula only (pinned + SRI).
const KATEX_CDN = "https://cdn.jsdelivr.net/npm/katex@0.19.0/dist/";
const KATEX_SRI = {
  js: "sha384-QFFtAGzvvj+bfgCGxXJlNZZR1nXEZgvG8tDLCCY1F19xl20WlfTYgguB4VcNdxYk",
  css: "sha384-3rdsX6e5mueWyoweR9NIVmtEsUkokpBT/0ALqKKIBMr9j4qhHkaIkAcGgsE6uVlp",
};
let katexLoad = null;
function loadKatex() {
  if (window.katex) return Promise.resolve(window.katex);
  katexLoad ??= new Promise((ok, fail) => {
    const css = document.createElement("link");
    css.rel = "stylesheet";
    css.href = KATEX_CDN + "katex.min.css";
    css.integrity = KATEX_SRI.css;
    css.crossOrigin = "anonymous";
    const js = document.createElement("script");
    js.src = KATEX_CDN + "katex.min.js";
    js.integrity = KATEX_SRI.js;
    js.crossOrigin = "anonymous";
    js.onload = () => ok(window.katex);
    js.onerror = () => {
      katexLoad = null; // offline now: try again on the next render
      fail(new Error("KaTeX did not load"));
    };
    document.head.append(css, js);
  });
  return katexLoad;
}
// Typeset the .md-math spans under `root` (renderMarkdown). KaTeX writes HTML
// for the eye plus MathML for screen readers, in the text colour (so both
// themes); `trust: false` turns \href, \htmlClass & co. into plain error text.
// Without KaTeX (offline) or for a formula it rejects, the TeX source stays.
async function typesetMath(root) {
  const els = [...root.querySelectorAll(".md-math:not(.md-math-done)")];
  if (!els.length) return;
  let katex;
  try {
    katex = await loadKatex();
  } catch (_) {
    return;
  }
  for (const el of els) {
    try {
      el.innerHTML = katex.renderToString(el.dataset.tex, {
        displayMode: el.classList.contains("md-math-display"),
        output: "htmlAndMathml",
        throwOnError: true,
        trust: false,
        strict: "ignore",
        maxSize: 20,
        maxExpand: 300,
      });
      el.classList.add("md-math-done");
    } catch (e) {
      el.classList.add("md-math-err");
      el.title = e.message;
    }
  }
}

// Minimal, dependency-free Markdown → HTML (headings, bold/italic, inline +
// fenced code, links, blockquotes, ordered/unordered lists, rules, paragraphs).
// Raw HTML in the source is escaped, so it's safe to inject.
function renderMarkdown(src) {
  const esc = escapeHtml;
  // $…$ / $$…$$ formulas (mathSpans) pass the Markdown pass as inert tokens,
  // then become a .md-math span showing their TeX source until typesetMath
  // swaps in KaTeX; a token inside a tag (a link URL) is dropped.
  const inline = (s) => {
    const math = [];
    const md = mathSpans(s)
      .map((g) => (g.tex == null ? g.text : (math.push(g), `\uE002${math.length - 1}\uE003`)))
      .join("");
    const html = inlineMd(md);
    if (!math.length) return html;
    const tok = /\uE002(\d+)\uE003/g;
    const span = (g) =>
      `<span class="md-math${g.display ? " md-math-display" : ""}" data-tex="${esc(g.tex)}"><code class="md-tex">${esc(g.tex)}</code></span>`;
    return html.replace(/<[^>]*>|[^<]+/g, (part) =>
      part[0] === "<" ? part.replace(tok, "") : part.replace(tok, (_, k) => span(math[+k]))
    );
  };
  const inlineMd = (s) =>
    esc(s)
      .replace(/`([^`]+)`/g, (m, c) => `<code>${c}</code>`)
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/__([^_]+)__/g, "<strong>$1</strong>")
      .replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>")
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, (m, t, u) => {
        // `u` is already HTML-escaped by esc(s) above; undo that to check the
        // scheme, then re-escape for the attribute.
        const unesc = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'" };
        const raw = u.replace(/&(amp|lt|gt|quot|#39);/g, (_, e) => unesc[e]);
        const href = safeUrl(raw);
        return href ? `<a href="${esc(href)}" target="_blank" rel="noopener">${t}</a>` : t;
      });
  const lines = String(src ?? "").replace(/\r/g, "").split("\n");
  let html = "",
    i = 0,
    list = null;
  const closeList = () => {
    if (list) {
      html += `</${list}>`;
      list = null;
    }
  };
  const special = /^(#{1,6}\s|```|>\s?|\s*[-*+]\s|\s*\d+\.\s)/;
  while (i < lines.length) {
    const line = lines[i];
    if (/^```/.test(line)) {
      closeList();
      i++;
      let code = "";
      while (i < lines.length && !/^```/.test(lines[i])) code += esc(lines[i++]) + "\n";
      i++;
      html += `<pre><code>${code}</code></pre>`;
      continue;
    }
    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      closeList();
      html += `<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`;
      i++;
      continue;
    }
    if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      closeList();
      html += "<hr>";
      i++;
      continue;
    }
    if (/^>\s?/.test(line)) {
      closeList();
      html += `<blockquote>${inline(line.replace(/^>\s?/, ""))}</blockquote>`;
      i++;
      continue;
    }
    if (/^\s*[-*+]\s+/.test(line)) {
      if (list !== "ul") {
        closeList();
        html += "<ul>";
        list = "ul";
      }
      html += `<li>${inline(line.replace(/^\s*[-*+]\s+/, ""))}</li>`;
      i++;
      continue;
    }
    if (/^\s*\d+\.\s+/.test(line)) {
      if (list !== "ol") {
        closeList();
        html += "<ol>";
        list = "ol";
      }
      html += `<li>${inline(line.replace(/^\s*\d+\.\s+/, ""))}</li>`;
      i++;
      continue;
    }
    if (/^\s*$/.test(line)) {
      closeList();
      i++;
      continue;
    }
    closeList();
    let para = line;
    i++;
    while (i < lines.length && !/^\s*$/.test(lines[i]) && !special.test(lines[i])) para += " " + lines[i++];
    html += `<p>${inline(para)}</p>`;
  }
  closeList();
  return html;
}

// ---------- export & share ----------
function download(blob, name) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

function csvOf(table) {
  // Export the FULL result (all pages), not just the rendered page.
  if (table._rows && table._cols) return csvOfRows(table._rows, table._cols);
  return [...table.rows]
    .map((r) => [...r.cells].map((c) => `"${c.textContent.replace(/"/g, '""')}"`).join(","))
    .join("\n");
}

// ::TEXT_SMALL/_MEDIUM/_LARGE → "small" | "medium" | "large" (or null).
function textSizeOf(s) {
  const t = s.roles.find((r) => TEXT_SIZES.includes(r[1]));
  return t ? t[1].split("_")[1].toLowerCase() : null;
}

const unq = (v) => String(v ?? "").replace(/^"|"$/g, "");

// CSV / .xls (HTML-table Excel) directly from JSON result rows.
function csvOfRows(rows, cols) {
  const esc = (v) => `"${unq(v).replace(/"/g, '""')}"`;
  return [cols.map(esc).join(","), ...rows.map((r) => cols.map((c) => esc(r[c])).join(","))].join("\n");
}
// The raw query rows as a workbook (::DOWNLOAD_XLSX): numbers stay numbers,
// ISO dates become dates, a bold frozen header with a filter.
function xlsxOfRows(rows, cols) {
  const cell = (v) => {
    if (v == null) return null;
    if (typeof v === "number" || typeof v === "boolean") return v;
    const t = unq(v);
    const d = excelDate(t);
    if (d != null) return { v: d, fmt: /[ T]\d/.test(t) ? "yyyy-mm-dd hh:mm" : "yyyy-mm-dd" };
    return NUM_RE.test(t) && t.trim() !== "" ? Number(t) : t;
  };
  const out = [cols.map((c) => ({ v: String(c), style: "head" })), ...rows.map((r) => cols.map((c) => cell(r[c])))];
  return xlsxOf({
    cols: cols.map((c) => ({ width: Math.min(50, Math.max(8, String(c).length + 2, ...rows.slice(0, 200).map((r) => String(unq(r[c] ?? "")).length + 2))) })),
    rows: out,
    freeze: 1,
    filter: cols.length ? `A1:${xlsxCol(cols.length - 1)}${rows.length + 1}` : null,
  });
}

// ── a minimal .xlsx writer (no dependency): SpreadsheetML parts in a stored
// (uncompressed) zip. `xlsxOf(sheet)` takes
//   { name, cols: [{ width }], rows: [[cell]], freeze: rows above the pane,
//     filter: "A2:F9" | null, merges: ["B1:C1"] }
// where a cell is null, a number, a string, a boolean or
// { v, f: formula without "=", fmt: Excel number format, style: "head" | "group" |
//   "body" | "sub" | "tot" }. Returns the file's bytes.
function crc32(bytes) {
  let c,
    crc = -1;
  crc32.t ||= Array.from({ length: 256 }, (_, n) => {
    c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  for (const b of bytes) crc = crc32.t[(crc ^ b) & 0xff] ^ (crc >>> 8);
  return (crc ^ -1) >>> 0;
}
function zipStored(files) {
  const enc = new TextEncoder();
  const parts = [];
  const central = [];
  let off = 0;
  for (const f of files) {
    const name = enc.encode(f.name);
    const data = typeof f.data === "string" ? enc.encode(f.data) : f.data;
    const crc = crc32(data);
    const loc = new DataView(new ArrayBuffer(30));
    // signature, version, flags (UTF-8 names), method 0 = stored, time, date (1980-01-01)
    [[0, 0x04034b50, 4], [4, 20, 2], [6, 0x800, 2], [8, 0, 2], [10, 0, 2], [12, 33, 2], [14, crc, 4], [18, data.length, 4], [22, data.length, 4], [26, name.length, 2], [28, 0, 2]].forEach(([o, v, n]) => (n === 4 ? loc.setUint32(o, v, true) : loc.setUint16(o, v, true)));
    const cen = new DataView(new ArrayBuffer(46));
    [[0, 0x02014b50, 4], [4, 20, 2], [6, 20, 2], [8, 0x800, 2], [10, 0, 2], [12, 0, 2], [14, 33, 2], [16, crc, 4], [20, data.length, 4], [24, data.length, 4], [28, name.length, 2], [30, 0, 2], [32, 0, 2], [34, 0, 2], [36, 0, 2], [38, 0, 4], [42, off, 4]].forEach(([o, v, n]) => (n === 4 ? cen.setUint32(o, v, true) : cen.setUint16(o, v, true)));
    parts.push(new Uint8Array(loc.buffer), name, data);
    central.push(new Uint8Array(cen.buffer), name);
    off += 30 + name.length + data.length;
  }
  const cenLen = central.reduce((a, b) => a + b.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  [[0, 0x06054b50, 4], [4, 0, 2], [6, 0, 2], [8, files.length, 2], [10, files.length, 2], [12, cenLen, 4], [16, off, 4], [20, 0, 2]].forEach(([o, v, n]) => (n === 4 ? end.setUint32(o, v, true) : end.setUint16(o, v, true)));
  const all = [...parts, ...central, new Uint8Array(end.buffer)];
  const out = new Uint8Array(all.reduce((a, b) => a + b.length, 0));
  let p = 0;
  for (const b of all) {
    out.set(b, p);
    p += b.length;
  }
  return out;
}
// a 0-based column index as Excel's letters (0 → A, 26 → AA)
function xlsxCol(i) {
  return (i >= 26 ? xlsxCol(Math.floor(i / 26) - 1) : "") + String.fromCharCode(65 + (i % 26));
}
function xlsxOf(sheet) {
  const x = (t) => String(t).replace(/[&<>"]/g, (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[ch]).replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, "");
  const colName = xlsxCol;
  // styles: one cell format per (number format, style) pair
  const fmts = []; // custom number formats (ids from 164)
  const xfs = [{ fmt: 0, style: "body" }];
  const xfOf = (fmt, style = "body") => {
    let id = 0;
    if (fmt) {
      let k = fmts.indexOf(fmt);
      if (k < 0) k = fmts.push(fmt) - 1;
      id = 164 + k;
    }
    let i = xfs.findIndex((s) => s.fmt === id && s.style === style);
    if (i < 0) i = xfs.push({ fmt: id, style }) - 1;
    return i;
  };
  const rowsXml = sheet.rows
    .map((row, r) => {
      const cells = row
        .map((cell, ci) => {
          if (cell == null) return "";
          const c = typeof cell === "object" ? cell : { v: cell };
          const ref = `${colName(ci)}${r + 1}`;
          const s = xfOf(c.fmt, c.style);
          const f = c.f ? `<f>${x(c.f)}</f>` : "";
          if (typeof c.v === "number" && isFinite(c.v)) return `<c r="${ref}" s="${s}">${f}<v>${c.v}</v></c>`;
          if (typeof c.v === "boolean") return `<c r="${ref}" s="${s}" t="b">${f}<v>${c.v ? 1 : 0}</v></c>`;
          if (c.v == null || c.v === "") return f ? `<c r="${ref}" s="${s}">${f}</c>` : `<c r="${ref}" s="${s}"/>`;
          return `<c r="${ref}" s="${s}" t="inlineStr"><is><t xml:space="preserve">${x(c.v)}</t></is></c>`;
        })
        .join("");
      return `<row r="${r + 1}">${cells}</row>`;
    })
    .join("");
  const ns = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';
  const pane = sheet.freeze
    ? `<sheetViews><sheetView workbookViewId="0"><pane ySplit="${sheet.freeze}" topLeftCell="A${sheet.freeze + 1}" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>`
    : "";
  const cols = (sheet.cols || []).length
    ? `<cols>${sheet.cols.map((c, i) => `<col min="${i + 1}" max="${i + 1}" width="${(c.width || 10).toFixed(1)}" customWidth="1"/>`).join("")}</cols>`
    : "";
  const filter = sheet.filter ? `<autoFilter ref="${sheet.filter}"/>` : "";
  const merges = (sheet.merges || []).length ? `<mergeCells count="${sheet.merges.length}">${sheet.merges.map((m) => `<mergeCell ref="${m}"/>`).join("")}</mergeCells>` : "";
  const ws = `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<worksheet ${ns}>${pane}${cols}<sheetData>${rowsXml}</sheetData>${filter}${merges}</worksheet>`;
  // fonts: 0 regular, 1 bold · fills: 0 none, 1 gray125 (required), 2 header / subtotal tint
  // borders: 0 none, 1 thin bottom (header), 2 thin top (subtotal), 3 double top (total)
  const look = {
    body: [0, 0, 0],
    head: [1, 2, 1],
    group: [1, 0, 0],
    sub: [1, 2, 2],
    tot: [1, 0, 3],
  };
  const xfXml = xfs
    .map(({ fmt, style }) => {
      const [font, fill, border] = look[style] || look.body;
      const align = style === "head" ? '<alignment wrapText="1" vertical="bottom"/>' : style === "group" ? '<alignment horizontal="center"/>' : "";
      return `<xf numFmtId="${fmt}" fontId="${font}" fillId="${fill}" borderId="${border}" xfId="0"${fmt ? ' applyNumberFormat="1"' : ""}${font ? ' applyFont="1"' : ""}${fill ? ' applyFill="1"' : ""}${border ? ' applyBorder="1"' : ""}${align ? ' applyAlignment="1">' + align + "</xf>" : "/>"}`;
    })
    .join("");
  const styles =
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">` +
    (fmts.length ? `<numFmts count="${fmts.length}">${fmts.map((f, i) => `<numFmt numFmtId="${164 + i}" formatCode="${x(f)}"/>`).join("")}</numFmts>` : "") +
    `<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>` +
    `<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFF2F3F5"/><bgColor indexed="64"/></patternFill></fill></fills>` +
    `<borders count="4"><border><left/><right/><top/><bottom/><diagonal/></border>` +
    `<border><left/><right/><top/><bottom style="thin"><color rgb="FF5B6170"/></bottom><diagonal/></border>` +
    `<border><left/><right/><top style="thin"><color rgb="FF9AA0AC"/></top><bottom/><diagonal/></border>` +
    `<border><left/><right/><top style="double"><color rgb="FF2B2F38"/></top><bottom/><diagonal/></border></borders>` +
    `<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>` +
    `<cellXfs count="${xfs.length}">${xfXml}</cellXfs>` +
    `<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>`;
  const name = String(sheet.name || "Data").replace(/[\[\]:*?/\\]/g, " ").trim().slice(0, 31) || "Data";
  const defined = sheet.filter
    ? `<definedNames><definedName name="_xlnm._FilterDatabase" localSheetId="0" hidden="1">'${x(name.replace(/'/g, "''"))}'!${sheet.filter.replace(/([A-Z]+)(\d+)/g, "$$$1$$$2")}</definedName></definedNames>`
    : "";
  return zipStored([
    {
      name: "[Content_Types].xml",
      data:
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">` +
        `<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>` +
        `<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>` +
        `<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>` +
        `<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>`,
    },
    {
      name: "_rels/.rels",
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>`,
    },
    {
      name: "xl/workbook.xml",
      data: `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<workbook ${ns}><sheets><sheet name="${x(name)}" sheetId="1" r:id="rId1"/></sheets>${defined}</workbook>`,
    },
    {
      name: "xl/_rels/workbook.xml.rels",
      data:
        `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">` +
        `<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>` +
        `<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>`,
    },
    { name: "xl/worksheets/sheet1.xml", data: ws },
    { name: "xl/styles.xml", data: styles },
  ]);
}
const XLSX_TYPE = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
// ISO date text → Excel's serial day number (1900 date system), else null
function excelDate(v) {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[ T](\d{2}):(\d{2})(?::(\d{2}(?:\.\d+)?))?)?$/.exec(String(v));
  if (!m) return null;
  const ms = Date.UTC(+m[1], +m[2] - 1, +m[3], +(m[4] || 0), +(m[5] || 0), 0) + 1000 * +(m[6] || 0);
  return isFinite(ms) ? (ms - Date.UTC(1899, 11, 30)) / 864e5 : null;
}

// A ::DOWNLOAD_CSV/_XLSX/_PDF button. CSV/XLSX export the query rows; PDF prints.
function mkDownload(s, rows) {
  const fmt = role(s, "DOWNLOAD_XLSX") ? "xlsx" : role(s, "DOWNLOAD_PDF") ? "pdf" : "csv";
  const label = { csv: "Download CSV", xlsx: "Download Excel", pdf: "Download PDF" }[fmt];
  const wrap = document.createElement("div");
  wrap.className = "controls download-row";
  const btn = document.createElement("button");
  btn.className = "dl-btn";
  btn.textContent = "⤓ " + label;
  btn.onclick = () => {
    if (fmt === "pdf") return window.print();
    const cols = rows.length ? Object.keys(rows[0]) : [];
    if (fmt === "xlsx") download(new Blob([xlsxOfRows(rows, cols)], { type: XLSX_TYPE }), "data.xlsx");
    else download(new Blob([csvOfRows(rows, cols)], { type: "text/csv" }), "data.csv");
  };
  wrap.appendChild(btn);
  return wrap;
}

// Rasterise a chart's SVG to a PNG (white background, 2× for crispness).
function svgToPng(svg, name) {
  const vb = svg.viewBox.baseVal;
  const w = vb.width || svg.clientWidth || 460;
  const h = vb.height || svg.clientHeight || 300;
  const xml = new XMLSerializer().serializeToString(svg);
  const img = new Image();
  img.onload = () => {
    const c = document.createElement("canvas");
    c.width = w * 2;
    c.height = h * 2;
    const ctx = c.getContext("2d");
    ctx.scale(2, 2);
    ctx.fillStyle = axToken("export-bg", "white"); // the theme's card colour
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob((b) => download(b, name + ".png"));
  };
  img.src = "data:image/svg+xml;base64," + btoa(unescape(encodeURIComponent(xml)));
}

// A hover CSV-download button on table panels. Charts get their download (PNG)
// and full-size actions from the toolbox (attachToolbox), which lives in the same
// top-right corner — so we don't add overlapping corner buttons on charts.
function addExportButtons() {
  document.querySelectorAll(".panel").forEach((fig) => {
    if (fig.querySelector(".dl")) return;
    const svg = fig.querySelector("svg");
    const table = fig.querySelector(".dp-table");
    if (svg || !table || fig.querySelector(".tbl-bar")) return; // tables export from their toolbar
    fig.style.position = "relative";
    const btn = document.createElement("button");
    btn.className = "dl";
    btn.textContent = "⤓";
    btn.title = "download CSV";
    btn.onclick = (e) => {
      e.stopPropagation();
      download(new Blob([csvOf(table)], { type: "text/csv" }), "data.csv");
    };
    fig.appendChild(btn);
  });
}

// Open a chart panel large in a centred overlay. The SVG is vector, so we clone
// it and let it scale to fill (Esc / backdrop / ✕ closes).
// A panel's chart as one SVG: the chart's own, or — with ::TRACK strips —
// the chart and its strips stacked in a new one.
function panelSvg(fig) {
  const svg = fig.querySelector("svg");
  const strips = [...fig.querySelectorAll(".panel-tracks > svg")];
  if (!svg || !strips.length) return svg;
  const all = [svg, ...strips];
  const w = svg.viewBox.baseVal.width;
  const out = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  let y = 0;
  for (const s of all) {
    const c = s.cloneNode(true);
    const h = s.viewBox.baseVal.height;
    for (const [k, v] of [["x", 0], ["y", y], ["width", w], ["height", h]]) c.setAttribute(k, v);
    out.appendChild(c);
    y += h;
  }
  out.setAttribute("viewBox", `0 0 ${w} ${y}`);
  out.setAttribute("width", w);
  out.setAttribute("height", y);
  return out;
}

function openFullSize(fig) {
  const svg = panelSvg(fig);
  if (!svg) return;
  const title = fig.querySelector(".panel-title")?.textContent || "Full size";
  const back = document.createElement("div");
  back.className = "dp-modal-back";
  const modal = document.createElement("div");
  modal.className = "dp-modal dp-modal-full";
  const head = document.createElement("div");
  head.className = "dp-modal-head";
  const h = document.createElement("strong");
  h.textContent = title;
  const x = document.createElement("button");
  x.className = "dp-modal-x";
  x.textContent = "✕";
  head.append(h, x);
  const body = document.createElement("div");
  body.className = "dp-modal-body";
  const clone = svg.cloneNode(true);
  clone.removeAttribute("width");
  clone.removeAttribute("height");
  clone.style.width = "100%";
  clone.style.height = "auto";
  body.appendChild(clone);
  modal.append(head, body);
  back.appendChild(modal);
  const close = () => {
    back.remove();
    document.removeEventListener("keydown", onKey);
  };
  const onKey = (e) => {
    if (e.key === "Escape") close();
  };
  x.onclick = close;
  back.onclick = (e) => {
    if (e.target === back) close();
  };
  document.addEventListener("keydown", onKey);
  document.body.appendChild(back);
}

// Share: encode the SQL in the URL hash (no server needed).
const encodeSql = (sql) => btoa(unescape(encodeURIComponent(sql)));
function decodeHashSql() {
  const m = location.hash.match(/sql=([^&]+)/);
  if (!m) return null;
  try {
    return decodeURIComponent(escape(atob(m[1])));
  } catch (e) {
    return null;
  }
}
// Share a view-only link: the SQL rides in the #sql hash and ?embed=1 hides the
// editor, sidebar and toolbar, so the recipient can view and interact with the
// dashboard but not edit its SQL. (Shift-click for an editable link instead.)
function shareLink(e) {
  const encoded = encodeSql($("sql").value);
  const editable = e && e.shiftKey;
  const base = location.origin + location.pathname;
  // The view travels too: the open tab and the viewer state (?v.* inputs and
  // selections), so the recipient lands on exactly this view.
  const q = new URLSearchParams(editable ? "" : "embed=1");
  const tab = new URLSearchParams(location.search).get("tab");
  if (tab) q.set("tab", tab);
  for (const [k, v] of urlStateParams()) q.append(k, v);
  const qs = q.toString();
  const url = base + (qs ? "?" + qs : "") + "#sql=" + encoded;
  navigator.clipboard.writeText(url).then(
    () => status(editable ? "editable link copied ✓" : "view-only link copied ✓"),
    () => {
      location.hash = "sql=" + encoded;
      status("copy the link from the address bar");
    }
  );
}

// Download the current dashboard as a standalone, self-contained HTML file.
// It carries the dashboard stylesheet (web/theme-base.css) and the CURRENT
// theme variant's tokens, so a dark dashboard stays dark when opened alone.
async function downloadHtml() {
  await lazyFlushAll(); // the snapshot holds every tab
  const sheetText = (sheet) => {
    try {
      return [...sheet.cssRules].map((r) => r.cssText).join("\n");
    } catch (_) {
      return "";
    }
  };
  // The stylesheet's source text (re-serialised CSSOM can reorder shorthand /
  // longhand pairs such as the KPI's clipped-text background).
  const sheet = [...document.styleSheets].find((sh) => sh.href && /theme-base\.css/.test(sh.href));
  let base = "";
  try {
    base = await (await fetch(sheet.href)).text();
  } catch (_) {
    base = sheet ? sheetText(sheet) : "";
  }
  const tokens = document.getElementById("ax-theme")?.textContent || "";
  await typesetMath(document.querySelector(".dash"));
  // A snapshot has no live tables or chart tools: drop their search / filter /
  // pager controls and the panel toolboxes.
  const snap = document.querySelector(".dash").cloneNode(true);
  snap.querySelectorAll(".tbl-bar-r, .tbl-chip, tr.tbl-frow, .table-foot, .dp-toolbox").forEach((el) => el.remove());
  // …nor the builder's header editing (#60).
  snap.querySelectorAll(".dp-hedit, .dp-head-blank").forEach((el) => el.remove());
  snap.querySelectorAll(".dp-title[data-title]").forEach((h) => {
    if (!h.dataset.title) return h.remove();
    h.textContent = h.dataset.title;
    h.removeAttribute("data-title");
  });
  dpAnim.strip(snap); // never a mid-animation frame
  const math = snap.querySelector(".katex") ? await katexCssInline() : "";
  // no KaTeX stylesheet to embed: the formulas' MathML alone (drawn natively)
  if (!math) snap.querySelectorAll(".katex-html").forEach((el) => el.remove());
  const content = snap.innerHTML;
  const v = dpTheme.set && dpTheme.set[dpTheme.variant];
  const b = (v && v.brand) || {};
  const title = b.title ? b.title + (b.title_suffix || "") : "anofox-visualization dashboard";
  const fav = b.favicon && safeUrl(b.favicon, { image: true });
  const html =
    `<!doctype html><html data-ax-variant="${dpTheme.variant}"><head><meta charset="utf-8"><title>${escapeHtml(title)}</title>` +
    (fav ? `<link rel="icon" href="${escapeHtml(fav)}">` : "") +
    `<style>${tokens}\n${base}\nbody{margin:0}</style>` +
    (math ? `<style>${math}</style>` : "") +
    `</head><body class="embed">` +
    `<div class="dash">${content}</div><div id="dp-tip" class="dp-tip"></div>` +
    `<script>${SNAPSHOT_JS}<\/script><script>${snapshotMotion()}<\/script></body></html>`;
  download(new Blob([html], { type: "text/html" }), "dashboard.html");
}

// KaTeX's stylesheet with its fonts inlined (woff2 data: URLs), so an
// exported dashboard draws its formulas offline; "" when it can't be fetched.
async function katexCssInline() {
  try {
    const css = await (await fetch(KATEX_CDN + "katex.min.css", { integrity: KATEX_SRI.css })).text();
    const fonts = {};
    for (const f of new Set(css.match(/fonts\/[\w-]+\.woff2/g))) {
      const buf = new Uint8Array(await (await fetch(KATEX_CDN + f)).arrayBuffer());
      let bin = "";
      for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
      fonts[f] = "data:font/woff2;base64," + btoa(bin);
    }
    // each @font-face keeps just its woff2
    return css.replace(/src:url\((fonts\/[\w-]+\.woff2)\)[^;}]*/g, (_, f) => `src:url(${fonts[f]}) format("woff2")`);
  } catch (_) {
    return "";
  }
}

// The export's chart motion: web/anim.js's kit (its own source, so there is
// one implementation) replaying each chart's entrance and rebuilding its
// ::STORY stepper at the overview.
function snapshotMotion() {
  return `(function(){var K=(${animKit})(window);
  document.querySelectorAll('.panel svg[data-anim]').forEach(function(s){K.enter(s,s.getAttribute('data-anim'));});
  document.querySelectorAll('.panel-svg > svg[data-story]').forEach(function(s){K.story(s.closest('.panel'),s);});
})();`;
}

// Self-contained hover + tab switching for the exported snapshot.
const SNAPSHOT_JS = `(function(){
  var tip=document.getElementById('dp-tip');
  document.querySelectorAll('.dp-hit').forEach(function(el){
    var t=el.getAttribute('data-tip'); if(!t)return;
    // the formatted value (::YFORMAT, ::LOCALE) in place of the raw one
    var fv=el.getAttribute('data-fvalue'), x=el.getAttribute('data-fx')||el.getAttribute('data-x'), s=el.getAttribute('data-series');
    if(fv)t=(x&&x!==s?x+(s?' · '+s:''):(s||x||''))+': '+fv;
    el.addEventListener('mouseenter',function(){tip.textContent=t;tip.classList.add('show');});
    el.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
    el.addEventListener('mouseleave',function(){tip.classList.remove('show');});
  });
  var wrap=document.querySelector('.tabwrap');
  document.querySelectorAll('.tab-btn').forEach(function(btn,i){
    btn.addEventListener('click',function(){
      wrap.querySelectorAll('.tabpane').forEach(function(p){p.style.display='none';});
      document.querySelectorAll('.tab-btn').forEach(function(b){b.classList.remove('active');});
      wrap.querySelectorAll('.tabpane')[i].style.display=''; btn.classList.add('active');
    });
  });
})();`;

function showError(grid, msg) {
  const d = document.createElement("div");
  d.className = "err";
  d.textContent = msg;
  grid.appendChild(d);
}

// Scroll-to-zoom / drag-to-pan for a map panel. Re-renders the panel's SVG with
// a lon/lat zoom window (double-click resets to auto-fit). The initial view is
// seeded from the geometry bounds, padded to the panel's aspect so the first
// zoom step doesn't jump.
function attachMapZoom(holder, rowsJson, roles, ph) {
  const W = () => holder.querySelector("svg")?.viewBox.baseVal.width || 460; // the width the map is drawn at
  let view = null; // {x0,x1,y0,y1} in lon/lat; null = auto-fit
  let raf = 0;

  const fit = () => {
    let b;
    try {
      b = JSON.parse(map_bounds(rowsJson, JSON.stringify(roles)));
    } catch (_) {
      b = [];
    }
    if (b.length !== 4) return { x0: -180, x1: 180, y0: -90, y1: 90 };
    const aspect = W() / ph;
    let [x0, x1, y0, y1] = b;
    const w = x1 - x0,
      h = y1 - y0;
    if (w / h < aspect) {
      const nw = h * aspect,
        c = (x0 + x1) / 2;
      x0 = c - nw / 2;
      x1 = c + nw / 2;
    } else {
      const nh = w / aspect,
        c = (y0 + y1) / 2;
      y0 = c - nh / 2;
      y1 = c + nh / 2;
    }
    return { x0, x1, y0, y1 };
  };

  const draw = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      const zoom = view ? JSON.stringify([view.x0, view.x1, view.y0, view.y1]) : "";
      renderChart(holder, rowsJson, roles, W(), ph, zoom);
      wireCharts(); // re-wire hover + cross-filter clicks on the fresh marks
    });
  };

  holder.style.cursor = "grab";
  holder.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      if (!view) view = fit();
      const r = holder.getBoundingClientRect();
      const px = view.x0 + ((e.clientX - r.left) / r.width) * (view.x1 - view.x0);
      const py = view.y1 - ((e.clientY - r.top) / r.height) * (view.y1 - view.y0);
      const k = e.deltaY < 0 ? 0.85 : 1 / 0.85;
      view = {
        x0: px + (view.x0 - px) * k,
        x1: px + (view.x1 - px) * k,
        y0: py + (view.y0 - py) * k,
        y1: py + (view.y1 - py) * k,
      };
      draw();
    },
    { passive: false }
  );

  let drag = null;
  holder.addEventListener("mousedown", (e) => {
    if (!view) view = fit();
    drag = { x: e.clientX, y: e.clientY, v: { ...view }, moved: false };
    holder.style.cursor = "grabbing";
  });
  window.addEventListener("mousemove", (e) => {
    if (!drag) return;
    if (Math.abs(e.clientX - drag.x) + Math.abs(e.clientY - drag.y) > 4) drag.moved = true;
    const r = holder.getBoundingClientRect();
    const dx = ((e.clientX - drag.x) / r.width) * (drag.v.x1 - drag.v.x0);
    const dy = ((e.clientY - drag.y) / r.height) * (drag.v.y1 - drag.v.y0);
    view = { x0: drag.v.x0 - dx, x1: drag.v.x1 - dx, y0: drag.v.y0 + dy, y1: drag.v.y1 + dy };
    draw();
  });
  window.addEventListener("mouseup", () => {
    if (drag && drag.moved) holder._panned = true; // suppress the trailing click
    if (drag) holder.style.cursor = "grab";
    drag = null;
  });
  // A drag-pan ends with a click event — swallow it so it doesn't cross-filter.
  holder.addEventListener(
    "click",
    (e) => {
      if (holder._panned) {
        holder._panned = false;
        e.stopPropagation();
      }
    },
    true
  );
  holder.addEventListener("dblclick", () => {
    view = null;
    draw();
  });
}

// A mark's tooltip value for display: ggplot-rs writes `data-value` raw, so
// round numbers like its own tooltip formatter (3 decimals).
function fmtTipValue(v) {
  const f = Number(v);
  return v !== "" && Number.isFinite(f) ? String(Math.round(f * 1000) / 1000) : v;
}

// Hover / selection metadata of a mark. ggplot-rs (>= 0.16) writes
// `data-series` (colour/fill/group level), `data-value` (the raw measured value)
// and `data-x`; the renderer adds `data-fvalue` / `data-fx` — the value and x
// as the panel formats them (`::YFORMAT` currency/percent/unit, `::XFORMAT`,
// the dashboard locale). Marks without them (maps, older SVGs) fall back to
// parsing the "series: value" `<title>` text `tip`. `attr` reads an attribute
// (null if absent). `detail` is the full tooltip when it says more than
// "series: value" (box-plot stats, OHLC, bin ranges, …), else "". `fx` is set
// when x carries its own formatted label; `own` when the series is the x
// itself (a single-series bar).
function markInfo(attr, tip) {
  const i = tip.lastIndexOf(": ");
  const tSeries = i >= 0 ? tip.slice(0, i) : tip;
  const tValue = i >= 0 ? tip.slice(i + 2) : "";
  const ds = attr("data-series");
  const dv = attr("data-value");
  const fv = attr("data-fvalue");
  const series = ds !== null && ds !== "" ? ds : tSeries;
  const raw = dv !== null && dv !== "" ? fmtTipValue(dv) : tValue;
  const value = fv !== null && fv !== "" ? fv : raw;
  const plain = [raw, value].some(
    (v) => tip === v || tip === series || tip === `${series}: ${v}` || tip === `${tSeries}: ${v}`
  );
  const fx = attr("data-fx") || "";
  const x = attr("data-x") || "";
  return { series, value, x: fx || x, fx: !!fx, own: series === x, detail: plain ? "" : tip };
}

// A discrete x drawn on numeric slots (grouped bars with ::DATALABELS) carries a
// `data-xticks` map {slot: level} on its root <svg>: show "W1", not "1.225".
function xTickLabel(el, x) {
  const root = el.closest && el.closest("svg[data-xticks]");
  if (!root || x === "") return x;
  try {
    const ticks = JSON.parse(root.getAttribute("data-xticks"));
    const slot = String(Math.round(Number(x)));
    return ticks[x] ?? ticks[slot] ?? x;
  } catch (_) {
    return x;
  }
}

// Styled hover tooltips + click-to-highlight LINKING across all panels.
// Every mark carrying a `<title>` becomes hoverable; its series key (ggplot's
// `data-series`, else the title part before ": ") is the selection key.
// Clicking a mark highlights that series everywhere and dims the rest; click
// again (or the background) clears.
let dpSelected = null;

function attachHover() {
  const tip = $("dp-tip");
  // Only the charts not seen yet (a lazily rendered tab adds some): a wired
  // mark has given its <title> up already.
  const marks = [
    ...document.querySelectorAll(
      ".panel svg:not([data-hw]) rect,.panel svg:not([data-hw]) circle,.panel svg:not([data-hw]) polygon,.panel svg:not([data-hw]) polyline"
    ),
  ].filter((el) => el.querySelector("title") && el.querySelector("title").textContent.trim());
  document.querySelectorAll(".panel svg:not([data-hw])").forEach((svg) => svg.setAttribute("data-hw", ""));

  // Dim non-selected marks (the shared emphasis state's "select" layer), but
  // only inside a chart that actually contains the selected series. A chart
  // keyed by a different dimension (e.g. a line filtered to one channel, keyed
  // by week) has no matching mark, so it stays fully visible instead of every
  // point dimming away.
  const apply = () => {
    // Every wired mark: a panel a re-run kept was wired before.
    const all = [...document.querySelectorAll(".panel svg .dp-hit")];
    const markSet = new Set(all);
    const bySvg = new Map();
    for (const el of all) {
      const svg = rootSvg(el);
      (bySvg.get(svg) || bySvg.set(svg, []).get(svg)).push(el);
    }
    for (const [svg, els] of bySvg) {
      const hasSel = !!dpSelected && els.some((el) => el.getAttribute("data-series") === dpSelected);
      dpAnim.emph(
        svg,
        "select",
        hasSel ? (el) => (markSet.has(el) && el.getAttribute("data-series") !== dpSelected ? 0.15 : null) : null
      );
    }
  };

  marks.forEach((el) => {
    const t = el.querySelector("title");
    const txt = t.textContent;
    const info = markInfo((k) => el.getAttribute(k), txt);
    if (!info.fx) info.x = xTickLabel(el, info.x);
    const series = info.series;
    el.removeChild(t);
    el.setAttribute("data-series", series);
    el.setAttribute("data-tip", txt);
    el.classList.add("dp-hit");
    // A reference rule (::XLINE, ::REFLINE, thresholds) keeps its tooltip but
    // is no value to click: it neither selects, drills nor cross-filters.
    // A ::DIAGRAM edge is hover-only too (it names no value to select).
    const rule = el.hasAttribute("data-rule") || el.hasAttribute("data-edge");
    el.style.cursor = rule ? "default" : "pointer";
    // A thin dashed rule is hard to hover (the pointer falls into its dash
    // gaps or onto the axis below): a transparent, wider twin on top hands the
    // pointer on to it.
    if (rule) {
      const pad = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
      for (const [k, v] of [["points", el.getAttribute("points")], ["fill", "none"], ["stroke", "transparent"], ["stroke-width", "8"], ["aria-hidden", "true"]])
        pad.setAttribute(k, v);
      for (const type of ["mouseenter", "mousemove", "mouseleave"])
        pad.addEventListener(type, (e) => el.dispatchEvent(new MouseEvent(type, { clientX: e.clientX, clientY: e.clientY })));
      pad.addEventListener("click", (e) => e.stopPropagation());
      el.after(pad);
    }
    // The panel-level crosshair shows the tooltips of the chart's own marks;
    // a ::TRACK strip's marks show their own.
    const pointered = () => !!el.closest(".has-axis-pointer") && !el.closest(".panel-tracks");
    el.addEventListener("mouseenter", () => {
      // The crosshair shows the tooltip — except for a mark focused from the
      // keyboard (a11y), which has no pointer to drive the crosshair.
      if (pointered() && document.activeElement !== el) return;
      const fill = el.getAttribute("fill") || getComputedStyle(el).fill || "#619cff";
      tip.innerHTML = (info.x ? `<div class="tip-head">${escapeHtml(info.x)}</div>` : "") + tipRow(info, fill);
      tip.classList.add("show");
    });
    el.addEventListener("mousemove", (e) => {
      if (pointered()) return;
      tip.style.left = e.clientX + 14 + "px";
      tip.style.top = e.clientY + 14 + "px";
    });
    el.addEventListener("mouseleave", () => {
      if (pointered()) return;
      tip.classList.remove("show");
    });
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      if (rule) return;
      if (el.closest(".panel-tracks")) return toggleTrackShade(el);
      const ia = el.closest(".panel")?._ia;
      // The mark's own value — its x (a drill level is the x), else its series.
      const dx = el.getAttribute("data-x");
      const key = dx ? String(xTickLabel(el, dx)) : series;
      if (ia && ia.drill && drillInto(ia.drill, key)) return;
      if (ia && ia.link) {
        dpLinkFrom = markKey(el.closest(".panel"), el);
        return followLink(chartLink(ia.link, el, key, series));
      }
      if (ia && ia.select) return clickNamedSelection(ia.select, series, e.shiftKey || e.ctrlKey || e.metaKey);
      // Cross-filter: toggle `selected` to this value and re-query. Queries that
      // opt in (getvariable('selected')) filter; the rest just highlight it.
      dpFilter = dpFilter === series ? "" : series;
      dpSelected = dpFilter || null;
      run(false);
    });
  });
  apply();
  decorateSelections();
  attachAxisPointer();
  attachLegendToggle();
  attachToolbox();
  attachChartA11y();
}

// ---- accessible charts (#37) ----
// Every chart SVG gets a one-sentence screen-reader summary (aria-label +
// <desc>). A chart with marks is one Tab stop: arrow keys walk its marks
// (←/→ along a series, ↑/↓ across series, Home/End), Enter/Space clicks the
// focused mark (select / drill / link), Esc returns to the chart. The focused
// mark shows its tooltip. "Charts as tables" swaps every chart for its data.
let dpAsTables = (() => {
  try {
    return new URLSearchParams(location.search).get("tables") === "1" || localStorage.getItem("dp-as-tables") === "1";
  } catch (_) {
    return false;
  }
})();
let dpA11yFocus = null; // {stmt, series, x} — the mark ({stmt, chart}: the chart) to refocus after a re-run

const CHART_KINDS = [
  [/^BARCHART|^BAR_/, "Bar chart"],
  [/^LINECHART|^STEP|^SPARKLINE/, "Line chart"],
  [/^AREACHART/, "Area chart"],
  [/^SCATTER|^BUBBLE|^JITTER|^SMOOTH/, "Scatter plot"],
  [/^PIE/, "Pie chart"],
  [/^DONUT/, "Donut chart"],
  [/^GAUGE/, "Gauge"],
  [/^RADAR/, "Radar chart"],
  [/^HISTOGRAM/, "Histogram"],
  [/^DENSITY/, "Density plot"],
  [/^BOXPLOT/, "Box plot"],
  [/^VIOLIN/, "Violin plot"],
  [/^QQ/, "Q-Q plot"],
  [/^HEATMAP/, "Heatmap"],
  [/^CALENDAR/, "Calendar heatmap"],
  [/^CANDLESTICK/, "Candlestick chart"],
  [/^MAP/, "Map"],
  [/^DIAGRAM/, "Diagram"],
];
function chartKind(roles) {
  for (const r of roles || []) for (const [re, name] of CHART_KINDS) if (re.test(r[1])) return name;
  return "Chart";
}

// One sentence: kind, title, series, x range, value range and extremes.
// items = [{series, x, value}] (value a number or null).
function chartSummary(kind, title, items, fmt = (v) => Number(v).toLocaleString()) {
  let out = kind + (title ? `: ${title}` : "");
  if (!items.length) return out + ".";
  const series = [...new Set(items.map((m) => m.series).filter((v) => v !== "" && v != null))];
  const xs = [...new Set(items.map((m) => m.x).filter((v) => v !== "" && v != null))];
  const parts = [];
  const multi = series.length > 1 && !series.every((v, i) => v === xs[i]);
  if (multi)
    parts.push(
      `${series.length} series (${series.slice(0, 4).join(", ")}${series.length > 4 ? `, and ${series.length - 4} more` : ""})`
    );
  if (xs.length > 1) parts.push(`${xs.length} points from ${xs[0]} to ${xs[xs.length - 1]}`);
  const vals = items.filter((m) => m.value != null && Number.isFinite(m.value));
  if (vals.length) {
    const hi = vals.reduce((a, b) => (b.value > a.value ? b : a));
    const lo = vals.reduce((a, b) => (b.value < a.value ? b : a));
    const at = (m) => {
      const w = [multi ? m.series : "", m.x || (!multi ? m.series : "")].filter(Boolean);
      return w.length ? ` (${w.join(", ")})` : "";
    };
    if (hi === lo) parts.push(`value ${fmt(hi.value)}${at(hi)}`);
    else parts.push(`highest ${fmt(hi.value)}${at(hi)}, lowest ${fmt(lo.value)}${at(lo)}`);
  }
  return out + (parts.length ? "; " + parts.join("; ") : "") + ".";
}

// A chart's data marks — not its reference rules (`data-rule`: ::XLINE,
// ::REFLINE, thresholds), which keep their tooltip but are no data point.
function chartMarks(svg) {
  return [...svg.querySelectorAll(".dp-hit")].filter(
    (el) => el.getAttribute("data-tip") && !el.hasAttribute("data-rule") && !el.hasAttribute("data-edge")
  );
}

function attachChartA11y() {
  // Each chart, and each ::TRACK strip under it (its own summary and marks).
  const charts = [...document.querySelectorAll(".dash .panel")].flatMap((fig) =>
    [fig.querySelector(".panel-svg > svg"), ...fig.querySelectorAll(".panel-tracks > svg")].filter(Boolean).map((svg) => [fig, svg])
  );
  for (const [fig, svg] of charts) {
    if (svg.dataset.a11y) continue;
    svg.dataset.a11y = "1";
    const marks = chartMarks(svg);
    const title = svg._track ? trackName(svg._track) : fig.querySelector(".panel-title")?.textContent?.trim() || "";
    // A strip's ticks / tiles are events in lanes: their text is no value.
    const lanes = !!svg._track && svg.hasAttribute("data-ylevels");
    const items = marks.map((el) => {
      const info = markInfo((k) => el.getAttribute(k), el.getAttribute("data-tip"));
      return { series: info.series, x: info.fx ? info.x : xTickLabel(el, info.x), value: lanes ? null : markValue(el) };
    });
    // A ::DIAGRAM brings its own summary (steps, stages, loops) from the renderer.
    const ds = svg.getAttribute("data-summary");
    const kind = svg._track ? "Strip under the chart" : chartKind(fig._dp && (fig._dp.drawn || fig._dp.roles));
    const summary =
      ds != null ? `Diagram${title ? `: ${title}` : ""}; ${ds}.` : chartSummary(kind, title, items, (v) => fmtTipValue(String(v)));
    svg.setAttribute("aria-label", summary);
    svg.setAttribute("aria-roledescription", "chart");
    svg.querySelector(":scope > desc")?.remove();
    const desc = document.createElementNS("http://www.w3.org/2000/svg", "desc");
    desc.textContent = summary;
    svg.prepend(desc);
    if (!marks.length) {
      svg.setAttribute("role", "img");
      continue;
    }
    svg.setAttribute("role", "group");
    svg.setAttribute("tabindex", "0");
    const tb = svg.getAttribute("data-diagram") === "TB";
    svg.setAttribute(
      "aria-description",
      svg.hasAttribute("data-diagram")
        ? `Arrow keys follow the connections (${tb ? "down" : "right"} to the next step, ${tb ? "up" : "left"} to the previous), ${tb ? "left and right" : "up and down"} move within a stage, Enter selects, Escape leaves.`
        : "Arrow keys move between data points, Enter selects, Escape leaves."
    );
    marks.forEach((el) => {
      el.setAttribute("tabindex", "-1");
      el.setAttribute("role", "img");
      el.setAttribute("aria-label", diagramNodeLabel(svg, el) || el.getAttribute("data-tip"));
      el.addEventListener("focus", () => {
        el.dispatchEvent(new MouseEvent("mouseenter"));
        const r = el.getBoundingClientRect();
        const tip = $("dp-tip");
        if (tip) {
          tip.style.left = r.right + 8 + "px";
          tip.style.top = r.top + 8 + "px";
        }
      });
      el.addEventListener("blur", () => {
        // a diagram node reached along an edge announced it: back to its own label
        if (el._dpVia) (el._dpVia = null), el.setAttribute("aria-label", diagramNodeLabel(svg, el));
        el.dispatchEvent(new MouseEvent("mouseleave"));
        if (el.closest(".has-axis-pointer")) $("dp-tip")?.classList.remove("show"); // shown by the focus
      });
    });
    svg.addEventListener("keydown", (e) => chartKey(e, svg, fig));
    // A click on a mark of the focused chart may re-run the dashboard too
    // (capture phase: before the mark's own handler starts the run).
    svg.addEventListener(
      "click",
      (e) => {
        const m = e.target.closest?.(".dp-hit");
        if (m && svg.contains(document.activeElement)) dpA11yFocus = markKey(fig, m);
      },
      true
    );
  }
  // After a re-run that replaced the chart, focus the same mark (or the chart
  // itself, if that had the focus), else the chart, else (the chart came back empty) the panel's title or the panel.
  // While the old chart still holds the focus the run is yet to come.
  if (dpA11yFocus) {
    const f = dpA11yFocus;
    const fig = document.querySelector(`.dash .panel[data-stmt="${f.stmt}"]`);
    const lost = !document.activeElement || document.activeElement === document.body;
    if (lost || !fig?.contains(document.activeElement)) dpA11yFocus = null;
    if (fig && lost) {
      const svg = fig.querySelector(".panel-svg > svg");
      const marks = svg ? chartMarks(svg) : [];
      let to = f.chart ? svg : marks.find((m) => m.getAttribute("data-series") === f.series && (m.getAttribute("data-x") || "") === f.x) || marks[0];
      if (!to) {
        to = svg || fig.querySelector(".panel-title") || fig;
        if (!to.hasAttribute("tabindex")) to.setAttribute("tabindex", "-1");
      }
      to.focus();
    }
  }
  applyAsTables();
  requestAnimationFrame(keyboardScrollers);
}

// A table that scrolls but holds nothing focusable (no row selection, links or
// buttons) becomes a Tab stop itself, so the keyboard can scroll it (WCAG
// scrollable-region-focusable).
function keyboardScrollers() {
  for (const el of document.querySelectorAll(".dash .tbl-scroll")) {
    const scrolls = el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
    const focusable = el.querySelector("a[href], button, input, select, [tabindex]:not([tabindex='-1'])");
    if (scrolls && !focusable) {
      el.tabIndex = 0;
      el.setAttribute("role", "region");
      const t = el.closest(".panel")?.querySelector(".panel-title")?.textContent?.trim();
      el.setAttribute("aria-label", t ? `${t} (scrollable table)` : "Scrollable table");
    } else if (el.getAttribute("role") === "region") {
      el.removeAttribute("tabindex");
      el.removeAttribute("role");
      el.removeAttribute("aria-label");
    }
  }
}

// Marks of a chart in reading order: series by first appearance, each sorted by x.
function markGrid(svg) {
  // A ::DIAGRAM: one row of nodes in reading order (layer by layer).
  if (svg.hasAttribute("data-diagram"))
    return [chartMarks(svg).sort((a, b) => +a.getAttribute("data-order") - +b.getAttribute("data-order"))].filter((r) => r.length);
  const by = new Map();
  for (const el of chartMarks(svg)) {
    const k = el.getAttribute("data-series") || "";
    (by.get(k) || by.set(k, []).get(k)).push(el);
  }
  const cx = (el) => {
    const r = el.getBoundingClientRect();
    return r.left + r.width / 2;
  };
  return [...by.values()].map((els) => els.map((el) => [cx(el), el]).sort((a, b) => a[0] - b[0]).map((p) => p[1]));
}

// What identifies a mark across re-runs (dpA11yFocus).
function markKey(fig, el) {
  return { stmt: fig.dataset.stmt, series: el.getAttribute("data-series"), x: el.getAttribute("data-x") || "" };
}

// A ::DIAGRAM node's spoken label: its tooltip and where it connects.
function diagramNodeLabel(svg, el) {
  const id = el.getAttribute("data-node");
  if (id == null || !svg.hasAttribute("data-diagram")) return "";
  const name = (n) => svg.querySelector(`rect[data-node="${CSS.escape(n)}"]`)?.getAttribute("data-fx") || n;
  const edges = [...svg.querySelectorAll("polyline[data-edge]")];
  const from = edges.filter((e) => e.getAttribute("data-to") === id).map((e) => name(e.getAttribute("data-from")));
  const to = edges.filter((e) => e.getAttribute("data-from") === id).map((e) => name(e.getAttribute("data-to")));
  return [el.getAttribute("data-tip"), from.length && `from ${from.join(", ")}`, to.length && `to ${to.join(", ")}`].filter(Boolean).join(". ");
}

// The keyboard walk of a ::DIAGRAM: along the edges (→ out, ← in; ↓ / ↑ on
// a top-to-bottom one), within a stage (↑/↓, ←/→), Home / End. Repeated
// steps out of a node take its next connection in turn; a step back returns
// along the edge it came by. The target announces the edge it was reached by.
function diagramStep(svg, cur, key) {
  const tb = svg.getAttribute("data-diagram") === "TB";
  const nodes = markGrid(svg)[0] || [];
  const byId = new Map(nodes.map((n) => [n.getAttribute("data-node"), n]));
  const id = cur.getAttribute("data-node");
  if (key === "Home") return [nodes[0]];
  if (key === "End") return [nodes.at(-1)];
  const [fwd, back, up, down] = tb ? ["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"] : ["ArrowRight", "ArrowLeft", "ArrowUp", "ArrowDown"];
  if (key === up || key === down) {
    const layer = nodes.filter((n) => n.getAttribute("data-layer") === cur.getAttribute("data-layer"));
    return [layer[layer.indexOf(cur) + (key === down ? 1 : -1)]];
  }
  const out = key === fwd;
  if (!out && key !== back) return [];
  const [mine, other] = out ? ["data-from", "data-to"] : ["data-to", "data-from"];
  const hops = [...svg.querySelectorAll("polyline[data-edge]")]
    .filter((e) => e.getAttribute(mine) === id && e.getAttribute(other) !== id && byId.has(e.getAttribute(other)))
    .map((e) => [byId.get(e.getAttribute(other)), e])
    .sort((a, b) => +a[0].getAttribute("data-order") - +b[0].getAttribute("data-order"));
  if (!hops.length) return [];
  let i = 0;
  if (!out && cur._dpCameFrom) i = Math.max(0, hops.findIndex((h) => h[0] === cur._dpCameFrom));
  if (out) i = cur._dpNextOut = ((cur._dpNextOut ?? -1) + 1) % hops.length;
  return hops[i];
}

function chartKey(e, svg, fig) {
  const keys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "Enter", " ", "Escape"];
  if (!keys.includes(e.key)) return;
  const grid = markGrid(svg);
  if (!grid.length) return;
  const cur = e.target;
  if (cur !== svg && cur.hasAttribute?.("data-node") && !["Enter", " ", "Escape"].includes(e.key)) {
    e.preventDefault();
    e.stopPropagation();
    const [to, edge] = diagramStep(svg, cur, e.key);
    if (!to) return;
    if (edge) {
      to._dpVia = edge;
      if (e.key !== (svg.getAttribute("data-diagram") === "TB" ? "ArrowUp" : "ArrowLeft")) to._dpCameFrom = cur;
      to.setAttribute("aria-label", `${edge.getAttribute("data-tip") || edge.getAttribute("data-fx")}. ${diagramNodeLabel(svg, to)}`);
    }
    to.focus();
    return;
  }
  if (cur === svg) {
    if (e.key === "Escape") return;
    e.preventDefault();
    (e.key === "End" ? grid[grid.length - 1].at(-1) : grid[0][0]).focus();
    return;
  }
  let si = grid.findIndex((g) => g.includes(cur));
  if (si < 0) return;
  let mi = grid[si].indexOf(cur);
  e.preventDefault();
  e.stopPropagation();
  if (e.key === "Escape") return svg.focus();
  if (e.key === "Enter" || e.key === " ") {
    dpA11yFocus = markKey(fig, cur);
    cur.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: e.shiftKey, ctrlKey: e.ctrlKey }));
    return;
  }
  if (e.key === "ArrowRight") mi = Math.min(grid[si].length - 1, mi + 1);
  else if (e.key === "ArrowLeft") mi = Math.max(0, mi - 1);
  else if (e.key === "Home") mi = 0;
  else if (e.key === "End") mi = grid[si].length - 1;
  else {
    // ↑/↓: the neighbouring series, at the mark nearest in x
    const x = cur.getBoundingClientRect().left;
    si = Math.max(0, Math.min(grid.length - 1, si + (e.key === "ArrowDown" ? 1 : -1)));
    const near = grid[si].map((el, k) => [Math.abs(el.getBoundingClientRect().left - x), k]).sort((a, b) => a[0] - b[0]);
    mi = near[0][1];
  }
  grid[si][mi].focus();
}

// "Charts as tables": every chart panel shows its data table instead.
function applyAsTables() {
  // a focused mark may just have been hidden (a re-wire alone keeps the tooltip)
  if (document.body.classList.contains("dp-as-tables") !== dpAsTables) $("dp-tip")?.classList.remove("show");
  document.body.classList.toggle("dp-as-tables", dpAsTables);
  const btn = document.querySelector(".dp-astable-btn");
  if (btn) btn.setAttribute("aria-pressed", String(dpAsTables));
  for (const fig of document.querySelectorAll(".dash .panel")) {
    const holder = fig.querySelector(".panel-svg");
    if (!holder || !fig._dp) continue;
    let view = fig.querySelector(".dp-astable-view");
    if (dpAsTables && !view) {
      view = document.createElement("div");
      view.className = "dp-astable-view";
      const title = fig.querySelector(".panel-title")?.textContent?.trim() || "Chart data";
      const dsvg = holder.querySelector(":scope > svg[data-diagram]");
      view.innerHTML = dsvg
        ? diagramTextHtml(dsvg, title)
        : dataTableHtml(fig._dp, 200, title) +
          (holder._tracks || []).map((t) => dataTableHtml(t, 200, trackName(t) || "Strip data")).join("");
      holder.after(view);
    }
    holder.hidden = dpAsTables;
    if (holder._trackBox) holder._trackBox.hidden = dpAsTables;
    if (view) view.hidden = !dpAsTables;
  }
}
function toggleAsTables() {
  dpAsTables = !dpAsTables;
  try {
    localStorage.setItem("dp-as-tables", dpAsTables ? "1" : "0");
  } catch (_) {}
  applyAsTables();
}


// One tooltip row: colour swatch + series + value (+ the full tooltip text
// when it carries more, e.g. box-plot stats or OHLC). A single-series bar's
// series is its x, already the tooltip's header: the row then shows the value.
function tipRow(info, fill) {
  const name = info.own || (info.x && info.series === info.x) ? "" : info.series;
  return (
    `<div class="tip-row"><span><span class="tip-dot" style="background:${escapeHtml(fill)}"></span>${escapeHtml(name)}</span><b>${escapeHtml(info.value)}</b></div>` +
    (info.detail ? `<div class="tip-row"><span>${escapeHtml(info.detail)}</span></div>` : "")
  );
}

// A reference rule's tooltip row: a dashed swatch, and where the rule sits
// (its formatted value, or the date / x it was drawn at).
function ruleRow(el) {
  const info = markInfo((k) => el.getAttribute(k), el.getAttribute("data-tip") || "");
  return `<div class="tip-row"><span><span class="tip-rule"></span>Reference line</span><b>${escapeHtml(info.value || info.x)}</b></div>`;
}

// ECharts-style toolbox: a hover-reveal toolbar per chart panel — chart-type
// toggle (line↔bar), data view, restore (reset zoom), and save-as-PNG.
function attachToolbox() {
  document.querySelectorAll(".panel").forEach((panel) => {
    if (panel.dataset.toolboxWired) return;
    const svg = panel.querySelector("svg");
    if (!svg || !svg.viewBox || !svg.viewBox.baseVal || !svg.viewBox.baseVal.width) return;
    if (panel.querySelector(".dp-table")) return; // a table's sparklines are not a chart
    if (panel.classList.contains("metric")) return; // nor is a KPI tile's trend line
    panel.dataset.toolboxWired = "1";
    const bar = document.createElement("div");
    bar.className = "dp-toolbox";
    const mkTool = (title, glyph, fn) => {
      const b = document.createElement("button");
      b.className = "dp-tool";
      b.title = title;
      b.textContent = glyph;
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        openTools(bar, false);
        fn();
      });
      bar.appendChild(b);
      return b;
    };
    // Where the bar would cover the title (.dp-narrow, set by foldTools) the
    // tools fold behind this one button.
    const more = document.createElement("button");
    more.className = "dp-tool dp-tool-more";
    more.title = "Chart tools";
    more.textContent = "⋯";
    more.setAttribute("aria-expanded", "false");
    more.addEventListener("click", (e) => {
      e.stopPropagation();
      openTools(bar, !bar.classList.contains("dp-open"));
    });
    bar.appendChild(more);
    bar.addEventListener("keydown", (e) => {
      if (e.key !== "Escape" || !bar.classList.contains("dp-open")) return;
      e.preventDefault(); // the menu has the key, not the selection reset
      openTools(bar, false);
      more.focus();
    });
    const dp = panel._dp;
    // magicType: swap a line chart to bars and back (only for line/bar charts).
    if (dp && !dp.isMap && magicSwap(dp.roles)) {
      mkTool("Line / bar", "⇄", () => toggleMagicType(panel));
    }
    // dataView: show the panel's underlying rows as a table.
    if (dp) mkTool("Data view", "▤", () => showDataView(panel));
    // brush + value filter: only for charts with real marks (points or bars).
    if (dp && !dp.isMap && !svg.hasAttribute("data-diagram") && svg.querySelector(".dp-hit")) {
      mkTool("Brush select", "▧", () => toggleBrush(panel));
      mkTool("Value filter", "◧", () => toggleVisualMap(panel));
    }
    // range slider: opt-in show/hide of the dataZoom bar (off by default).
    const zoombar = panel.querySelector(".dp-zoom");
    if (zoombar) {
      const t = mkTool("Range slider", "⬍", () => {
        const on = zoombar.style.display === "none";
        zoombar.style.display = on ? "" : "none";
        t.classList.toggle("dp-tool-on", on);
      });
    }
    // restore: reset any zoom/pan (a double-click on the chart does the same).
    if (zoombar) {
      mkTool("Restore", "⟳", () => {
        const h = panel.querySelector(".panel-svg");
        if (h) h.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
      });
    }
    mkTool("Full size", "⤢", () => openFullSize(panel));
    mkTool("Save as PNG", "⭳", () => savePanelPng(panel));
    panel.appendChild(bar);
    foldTools(panel);
  });
  if (!dpToolsAutoClose) {
    dpToolsAutoClose = true;
    document.addEventListener("click", () => document.querySelectorAll(".dp-toolbox.dp-open").forEach((b) => openTools(b, false)));
  }
}

// Open / close a folded toolbox's menu.
let dpToolsAutoClose = false;
function openTools(bar, open) {
  bar.classList.toggle("dp-open", open);
  bar.querySelector(".dp-tool-more")?.setAttribute("aria-expanded", String(open));
}

// Return the swapped chart-type role string (LINE↔BAR family), or null if the
// panel isn't a plain line/bar chart. Used by the magicType toggle.
function magicSwap(roles) {
  const map = {
    LINECHART: "BARCHART",
    LINE: "BARCHART",
    BARCHART: "LINECHART",
    BAR: "LINECHART",
    AREACHART: "BARCHART",
    AREA: "BARCHART",
  };
  const v = roles.find((r) => map[r[1]]);
  return v ? map[v[1]] : null;
}

function toggleMagicType(panel) {
  const dp = panel._dp;
  const holder = panel.querySelector(".panel-svg");
  if (!dp || !holder) return;
  const next = panel._magic ? 0 : 1;
  panel._magic = next;
  let use = dp.roles;
  if (next) {
    const vi = dp.roles.findIndex((r) => magicSwap([r])); // first line/bar value role
    use = dp.roles.map((r, i) => (i === vi ? [r[0], magicSwap([r]), r[2] || ""] : r));
  }
  dp.drawn = use; // the roles on screen (the summary names the chart kind shown)
  renderChart(holder, dp.rows, use, fitWidth(holder, dp.rw), dp.ph);
  wireCharts();
}

// A modal listing the panel's rows as a table (ECharts toolbox "data view").
// A panel's data (fig._dp: rows + roles) as an HTML table, data columns only.
// Rows JSON as DuckDB writes it: a DOUBLE can be a bare Infinity / NaN (an
// ROC curve's first threshold), which JSON.parse rejects — read them as text.
function parseRowsLenient(json) {
  try {
    return JSON.parse(json);
  } catch (e) {
    const fixed = json.replace(/([:,\[]\s*)(-?Infinity|NaN)(?=\s*[,}\]])/g, '$1"$2"');
    if (fixed === json) throw e;
    return JSON.parse(fixed);
  }
}
// A ::DIAGRAM as text: its steps as an ordered list in reading order (value,
// status, where each leads) and its connections as a table — read from the
// SVG's node and edge marks.
function diagramTextHtml(svg, caption) {
  const esc = (v) => escapeHtml(v == null ? "" : String(v));
  const nodes = [...svg.querySelectorAll("rect[data-node]")].sort((a, b) => +a.dataset.order - +b.dataset.order);
  const edges = [...svg.querySelectorAll("polyline[data-edge]")];
  const name = new Map(nodes.map((n) => [n.dataset.node, n.dataset.fx || n.dataset.node]));
  const items = nodes.map((n) => {
    const st = n.dataset.status && n.dataset.status !== "neutral" ? n.dataset.status : "";
    const extra = [n.dataset.fvalue, n.dataset.badge || st].filter(Boolean);
    const next = edges.filter((e) => e.dataset.from === n.dataset.node).map((e) => name.get(e.dataset.to) || e.dataset.to);
    return `<li>${esc(name.get(n.dataset.node))}${extra.length ? ` (${esc(extra.join(", "))})` : ""}${next.length ? ` → ${esc(next.join(", "))}` : ""}</li>`;
  });
  const label = caption ? ` aria-label="${esc(caption)}"` : "";
  const rows = edges.length
    ? edges.map((e) => `<tr><td>${esc(name.get(e.dataset.from))}</td><td>${esc(name.get(e.dataset.to))}</td><td>${esc(e.dataset.fvalue || "")}</td></tr>`)
    : [];
  const table = edges.length
    ? `<table class="dp-dv"${caption ? ` aria-label="${esc(caption)}: connections"` : ""}><thead><tr><th scope="col">From</th><th scope="col">To</th><th scope="col">Value</th></tr></thead><tbody>${rows.join("")}</tbody></table>`
    : `<table class="dp-dv"${label}><thead><tr><th scope="col">Step</th><th scope="col">Value</th></tr></thead><tbody>${nodes
        .map((n) => `<tr><td>${esc(name.get(n.dataset.node))}</td><td>${esc(n.dataset.fvalue || "")}</td></tr>`)
        .join("")}</tbody></table>`;
  return `<ol class="dp-diagram-steps"${label}>${items.join("")}</ol>${table}`;
}

function dataTableHtml(dp, limit, caption) {
  let rows;
  try {
    rows = typeof dp.rows === "string" ? parseRowsLenient(dp.rows) : dp.rows;
  } catch (_) {
    return "";
  }
  const roleFor = (k) => dp.roles.find((x) => x[0] === +String(k).replace(/^c/, ""));
  // Drop non-data columns (the panel title, tooltips, hints) from the view.
  const SKIP = new Set(["TITLE", "HINT", "LABEL", "TRACK"]);
  const keys = (rows.length ? Object.keys(rows[0]) : []).filter((k) => {
    const r = roleFor(k);
    return !r || !SKIP.has(r[1]);
  });
  // Header names: the role's display name, else a friendly role name, else key.
  const FRIENDLY = { XAXIS: "x", YAXIS: "y", CATEGORY: "series" };
  const nameFor = (k) => {
    const r = roleFor(k);
    if (r && r[2]) return r[2];
    if (r && FRIENDLY[r[1]]) return FRIENDLY[r[1]];
    if (r && /CHART|BAR|LINE|AREA|SCATTER|STEP|SMOOTH/.test(r[1])) return "value";
    return k;
  };
  const esc = (v) => escapeHtml(v == null ? "" : String(v));
  const head = keys.map((k) => `<th scope="col">${esc(nameFor(k))}</th>`).join("");
  const body = rows
    .slice(0, limit)
    .map((r) => `<tr>${keys.map((k) => `<td>${esc(r[k])}</td>`).join("")}</tr>`)
    .join("");
  return (
    `<table class="dp-dv"${caption ? ` aria-label="${esc(caption)}"` : ""}><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>` +
    (rows.length > limit ? `<div class="dp-modal-note">Showing ${limit} of ${rows.length} rows</div>` : "")
  );
}

function showDataView(panel) {
  const dp = panel._dp;
  if (!dp) return;
  const esc = (v) => escapeHtml(v == null ? "" : String(v));
  const title = panel.querySelector(".panel-title")?.textContent?.trim() || "Data";
  const back = document.createElement("div");
  back.className = "dp-modal-back";
  back.innerHTML =
    `<div class="dp-modal"><div class="dp-modal-head"><b>${esc(title)}</b>` +
    `<button class="dp-modal-x" title="Close">✕</button></div>` +
    `<div class="dp-modal-body">${dataTableHtml(dp, 500, title)}</div></div>`;
  const close = () => back.remove();
  back.addEventListener("click", (e) => {
    if (e.target === back || e.target.classList.contains("dp-modal-x")) close();
  });
  document.body.appendChild(back);
}

// A mark's measure: ggplot's `data-value`, else the last number in its
// tooltip ("label: 22", "web: 22", "(3, 22)"); null if none.
function markValue(el) {
  const dv = parseFloat(el.getAttribute("data-value"));
  if (Number.isFinite(dv)) return dv;
  const m = (el.getAttribute("data-tip") || "").match(/-?\d[\d,]*\.?\d*(?:[eE][+-]?\d+)?/g);
  if (!m) return null;
  const n = parseFloat(m[m.length - 1].replace(/,/g, ""));
  return isNaN(n) ? null : n;
}

// Every drawn data mark (scatter/line points AND bars carry class "dp-hit").
function marksOf(panel) {
  return panel.querySelectorAll("svg .dp-hit");
}

// Dim every data mark that fails `keep(el)`; pass null to clear the emphasis
// (the shared emphasis state's "filter" layer: brush, value filter, named
// selections).
function filterMarks(panel, keep) {
  const hits = new Set(marksOf(panel));
  const svgs = new Set([...hits].map(rootSvg));
  for (const svg of svgs) dpAnim.emph(svg, "filter", keep ? (el) => (hits.has(el) && !keep(el) ? 0.12 : null) : null);
}

// The panel-level <svg> a mark belongs to (the root of nested plot svgs).
function rootSvg(el) {
  let s = el.ownerSVGElement || el;
  while (s.ownerSVGElement) s = s.ownerSVGElement;
  return s;
}

// ECharts-style brush: toggle a drag-to-select overlay; marks inside the box
// stay highlighted, the rest dim. Toggling again (or Esc) clears it.
function toggleBrush(panel) {
  if (panel._brush) {
    panel._brush.remove();
    panel._brush = null;
    filterMarks(panel, null);
    return;
  }
  const holder = panel.querySelector(".panel-svg");
  const svg = panel.querySelector("svg");
  if (!holder || !svg) return;
  filterMarks(panel, null);
  const ov = document.createElement("div");
  ov.className = "dp-brush-ov";
  const rect = document.createElement("div");
  rect.className = "dp-brush-rect";
  ov.appendChild(rect);
  holder.appendChild(ov);
  panel._brush = ov;
  // On a ::SELECT_AS chart with a continuous / date x the brush is an x band.
  const sel = panel._ia && panel._ia.select;
  const xKind = sel ? xKindOf([...marksOf(panel)].map((el) => el.getAttribute("data-x") || "")) : null;
  const xBand = !!xKind;
  ov.addEventListener("click", (e) => e.stopPropagation()); // a drag is not a click on empty space
  let start = null;
  const rel = (e) => {
    const r = ov.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  ov.addEventListener("mousedown", (e) => {
    e.preventDefault();
    e.stopPropagation();
    // Clear any leftover axis-pointer scaling so hit-test rects are accurate.
    marksOf(panel).forEach((m) => (m.style.transform = ""));
    start = rel(e);
    Object.assign(rect.style, { left: start.x + "px", top: start.y + "px", width: 0, height: 0, display: "block" });
  });
  ov.addEventListener("mousemove", (e) => {
    e.stopPropagation();
    if (!start) return;
    const p = rel(e);
    const x = Math.min(start.x, p.x),
      y = xBand ? 0 : Math.min(start.y, p.y),
      w = Math.abs(p.x - start.x),
      h = xBand ? ov.clientHeight : Math.abs(p.y - start.y);
    Object.assign(rect.style, { left: x + "px", top: y + "px", width: w + "px", height: h + "px" });
  });
  ov.addEventListener("mouseup", (e) => {
    e.stopPropagation();
    if (!start) return;
    const p = rel(e);
    const box = {
      x0: Math.min(start.x, p.x),
      y0: Math.min(start.y, p.y),
      x1: Math.max(start.x, p.x),
      y1: Math.max(start.y, p.y),
    };
    start = null;
    if (box.x1 - box.x0 < 4 && box.y1 - box.y0 < 4) {
      filterMarks(panel, null);
      return;
    }
    const ovr = ov.getBoundingClientRect();
    const inBox = (el) => {
      const b = el.getBoundingClientRect();
      const cx = b.left + b.width / 2 - ovr.left,
        cy = b.top + b.height / 2 - ovr.top;
      return cx >= box.x0 && cx <= box.x1 && (xBand || (cy >= box.y0 && cy <= box.y1));
    };
    filterMarks(panel, inBox);
    // A ::SELECT_AS chart turns the brush into variables: the brushed x range
    // (<name>_from / <name>_to) on a continuous / date x, the brushed values on a
    // discrete x in multi mode.
    if (!sel) return;
    const hit = [...marksOf(panel)].filter(inBox);
    if (xKind) {
      const xs = hit.map((el) => xVal(el.getAttribute("data-x") || "", xKind)).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      dpRange[sel.name] = xs.length ? { from: xs[0], to: xs[xs.length - 1] } : null;
    } else if (sel.mode === "multi") {
      dpXf[sel.name] = [...new Set(hit.map((el) => el.getAttribute("data-series")))];
    } else return;
    run(false);
  });
}

// ECharts-style visualMap: a value slider; marks whose value falls outside the
// selected [lo,hi] range dim. Toggling again removes the control + emphasis.
function toggleVisualMap(panel) {
  if (panel._vmap) {
    panel._vmap.remove();
    panel._vmap = null;
    filterMarks(panel, null);
    return;
  }
  const vals = [...marksOf(panel)].map(markValue).filter((v) => v != null);
  if (vals.length < 2) return;
  const min = Math.min(...vals),
    max = Math.max(...vals),
    span = max - min || 1;
  const bar = document.createElement("div");
  bar.className = "dp-vmap";
  bar.innerHTML =
    `<span class="dp-vmap-lab dp-vmap-lo"></span>` +
    `<div class="dp-vmap-track"><div class="dp-vmap-fill"></div>` +
    `<div class="dp-vmap-h dp-vmap-h0"></div><div class="dp-vmap-h dp-vmap-h1"></div></div>` +
    `<span class="dp-vmap-lab dp-vmap-hi"></span>`;
  (panel.querySelector(".panel-svg") || panel).after
    ? panel.querySelector(".panel-svg").after(bar)
    : panel.appendChild(bar);
  panel._vmap = bar;
  const track = bar.querySelector(".dp-vmap-track");
  const fill = bar.querySelector(".dp-vmap-fill");
  const h0 = bar.querySelector(".dp-vmap-h0");
  const h1 = bar.querySelector(".dp-vmap-h1");
  const loLab = bar.querySelector(".dp-vmap-lo");
  const hiLab = bar.querySelector(".dp-vmap-hi");
  let f0 = 0,
    f1 = 1;
  const fmt = (v) => (Math.abs(v) >= 100 ? Math.round(v) : Math.round(v * 10) / 10);
  const apply = () => {
    fill.style.left = f0 * 100 + "%";
    fill.style.width = (f1 - f0) * 100 + "%";
    h0.style.left = f0 * 100 + "%";
    h1.style.left = f1 * 100 + "%";
    const lo = min + f0 * span,
      hi = min + f1 * span;
    loLab.textContent = fmt(lo);
    hiLab.textContent = fmt(hi);
    filterMarks(panel, (el) => {
      const v = markValue(el);
      return v == null || (v >= lo - 1e-9 && v <= hi + 1e-9);
    });
  };
  const fracAt = (cx) => {
    const r = track.getBoundingClientRect();
    return Math.max(0, Math.min(1, (cx - r.left) / (r.width || 1)));
  };
  let drag = null;
  const onMove = (e) => {
    if (!drag) return;
    const f = fracAt(e.clientX);
    if (drag === "h0") f0 = Math.min(f, f1 - 0.02);
    else f1 = Math.max(f, f0 + 0.02);
    apply();
  };
  const onUp = () => {
    drag = null;
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
  };
  const start = (which) => (e) => {
    e.preventDefault();
    e.stopPropagation();
    drag = which;
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };
  h0.addEventListener("mousedown", start("h0"));
  h1.addEventListener("mousedown", start("h1"));
  bar.addEventListener("click", (e) => e.stopPropagation());
  apply();
}

function savePanelPng(panel) {
  const svg = panelSvg(panel);
  if (!svg) return;
  const vb = svg.viewBox.baseVal;
  const clone = svg.cloneNode(true);
  clone.querySelectorAll(".dp-toolbox").forEach((el) => el.remove());
  if (!clone.getAttribute("width")) clone.setAttribute("width", vb.width);
  if (!clone.getAttribute("height")) clone.setAttribute("height", vb.height);
  const xml = new XMLSerializer().serializeToString(clone);
  const url = "data:image/svg+xml;base64," + btoa(unescape(encodeURIComponent(xml)));
  const scale = 2;
  const img = new Image();
  img.onload = () => {
    const c = document.createElement("canvas");
    c.width = vb.width * scale;
    c.height = vb.height * scale;
    const ctx = c.getContext("2d");
    ctx.fillStyle = axToken("export-bg", "white"); // the theme's card colour
    ctx.fillRect(0, 0, c.width, c.height);
    ctx.drawImage(img, 0, 0, c.width, c.height);
    c.toBlob((blob) => {
      if (!blob) return;
      const title = (panel.querySelector(".panel-title")?.textContent || "chart")
        .trim()
        .replace(/[^\w.-]+/g, "_")
        .slice(0, 60) || "chart";
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = title + ".png";
      a.click();
      URL.revokeObjectURL(a.href);
    });
  };
  img.src = url;
}

// ECharts-style legend toggle: click a series name in the top legend to hide /
// show that series (its marks + line). Hidden state persists per panel across
// zoom re-renders; the legend entry dims when off.
function attachLegendToggle() {
  document.querySelectorAll(".panel").forEach((panel) => {
    const svg = panel.querySelector("svg");
    if (!svg || svg.dataset.legendWired || !svg.viewBox || !svg.viewBox.baseVal.width) return;
    svg.dataset.legendWired = "0"; // looked at once (a re-render brings a new svg)
    // A heatmap's series are its rows (the y axis), not a legend.
    if (svg.getAttribute("data-kind") === "heatmap") return;
    const series = new Set([...svg.querySelectorAll("[data-series]")].map((el) => el.getAttribute("data-series")));
    if (series.size < 2) return; // need a real multi-series legend
    const vbH = svg.viewBox.baseVal.height;
    // Legend labels: text in the top band whose content is a series name (so
    // titles / axis numbers are excluded).
    const legendTexts = [...svg.querySelectorAll("text")].filter(
      (t) => (+t.getAttribute("y") || 0) < vbH * 0.16 && series.has(t.textContent.trim())
    );
    if (!legendTexts.length) return;
    svg.dataset.legendWired = "1";
    const hidden = panel._hiddenSeries || (panel._hiddenSeries = new Set());
    // The swatch is a small coloured rect just left of each label at a similar y.
    const swatches = [...svg.querySelectorAll("rect")].filter((r) => {
      const bb = r.getBBox();
      return bb.y < vbH * 0.16 && bb.width < 20 && bb.width > 3;
    });
    const entries = legendTexts.map((t) => {
      const tb = t.getBBox();
      let swatch = null,
        best = Infinity;
      for (const r of swatches) {
        const rb = r.getBBox();
        const d = tb.x - (rb.x + rb.width); // gap to the swatch on the left
        if (d >= -2 && d < 24 && Math.abs(rb.y + rb.height / 2 - (tb.y + tb.height / 2)) < 10 && d < best) {
          best = d;
          swatch = r;
        }
      }
      return { s: t.textContent.trim(), text: t, swatch, fill: swatch ? swatch.getAttribute("fill") : null };
    });
    const apply = () => {
      svg
        .querySelectorAll("[data-series]")
        .forEach((el) => (el.style.display = hidden.has(el.getAttribute("data-series")) ? "none" : ""));
      entries.forEach((e) => {
        const off = hidden.has(e.s);
        e.text.style.opacity = off ? "0.35" : "";
        if (e.swatch && e.fill) e.swatch.setAttribute("fill", off ? axToken("legend-off", "grey") : e.fill); // grey when off
      });
    };
    // ECharts-style focus: hovering a legend entry emphasises that series and
    // fades the others (visible ones only; hidden/toggled-off series stay off).
    const emphasize = (only) => {
      dpAnim.emph(
        svg,
        "legend",
        only
          ? (el) => {
              const s = el.getAttribute("data-series");
              return s != null && !hidden.has(s) && s !== only ? 0.15 : null;
            }
          : null
      );
      entries.forEach((e) => {
        if (hidden.has(e.s)) return;
        e.text.style.fontWeight = only === e.s ? "700" : "";
      });
    };
    entries.forEach((e) => {
      const toggle = (ev) => {
        ev.stopPropagation();
        hidden.has(e.s) ? hidden.delete(e.s) : hidden.add(e.s);
        apply();
      };
      [e.text, e.swatch].forEach((el) => {
        if (!el) return;
        el.style.cursor = "pointer";
        el.addEventListener("click", toggle);
        el.addEventListener("mouseenter", () => emphasize(e.s));
        el.addEventListener("mouseleave", () => emphasize(null));
      });
    });
    apply();
  });
}

// ECharts-style axis pointer: hovering a cartesian chart draws a vertical
// crosshair at the nearest x and shows one tooltip listing every series' value
// there (colour swatch + name + value), instead of a per-point tooltip.
function attachAxisPointer() {
  const tip = $("dp-tip");
  const cross = $("dp-cross");
  document.querySelectorAll(".panel").forEach((panel) => {
    // Wire each panel once; the handlers read the marks fresh so they survive a
    // zoom re-render (which swaps the SVG inside the same panel).
    if (panel.dataset.axisWired) return;
    // Maps aren't cartesian — a vertical x-crosshair grouping marks by longitude
    // is meaningless there. They keep their own per-point (<title>) hover.
    if (panel._dp && panel._dp.isMap) return;
    // A pure scatter has no shared-x series to group, so an axis crosshair would
    // highlight every point sharing the nearest x (two far-apart dots at once).
    // Skip it — the per-point (<title>) hover shows the individual point instead.
    const roles = (panel._dp && panel._dp.roles) || [];
    // Radar is polar, bullet/funnel are horizontal and jitter/scatter has no
    // shared-x series — a vertical x-crosshair is wrong for all of them; their
    // per-point (<title>) hover takes over.
    if (roles.some((r) => /^(RADAR|SPIDER|CANDLESTICK|BULLET|BULLETCHART|FUNNEL|FUNNELCHART|DIAGRAM)$/.test(r[1]))) return;
    const scatterOnly =
      roles.some((r) => /^(SCATTER|POINT|SCATTERCHART|JITTER|STRIP)$/.test(r[1])) &&
      !roles.some((r) => /LINE|AREA|STEP|SMOOTH|BAND/.test(r[1]));
    if (scatterOnly) return;
    const svg0 = panel.querySelector("svg");
    if (!svg0 || !svg0.viewBox || !svg0.viewBox.baseVal || !svg0.viewBox.baseVal.width) return;
    if (svg0.querySelectorAll("circle.dp-hit").length < 3) return; // needs a line/scatter chart
    panel.dataset.axisWired = "1";
    panel.classList.add("has-axis-pointer");
    const area = panel.querySelector(".panel-svg") || svg0;

    const move = (e) => {
      const svg = area.querySelector ? area.querySelector("svg") : svg0;
      if (!svg || !svg.viewBox.baseVal.width) return;
      const circles = [...svg.querySelectorAll("circle.dp-hit")];
      if (circles.length < 3) return leave();
      const pts = circles.map((el) => ({
        el,
        cx: +el.getAttribute("cx"),
        info: markInfo((k) => el.getAttribute(k), el.getAttribute("data-tip") || ""),
        fill: el.getAttribute("fill") || getComputedStyle(el).fill || "#619cff",
      }));
      const vb = svg.viewBox.baseVal;
      const r = svg.getBoundingClientRect();
      if (!r.width) return;
      const scale = r.width / vb.width;
      const ux = vb.x + (e.clientX - r.left) / scale;
      let best = null,
        bd = Infinity;
      for (const p of pts) {
        const d = Math.abs(p.cx - ux);
        if (d < bd) {
          bd = d;
          best = Math.round(p.cx);
        }
      }
      const colPts = pts.filter((p) => Math.round(p.cx) === best);
      cross.style.left = r.left + (best - vb.x) * scale + "px";
      cross.style.top = r.top + "px";
      // …down through the chart's ::TRACK strips (same x)
      const tb = panel.querySelector(".panel-tracks");
      cross.style.height = (tb && tb.offsetHeight ? tb.getBoundingClientRect().bottom - r.top : r.height) + "px";
      cross.style.display = "";
      pts.forEach((p) => (p.el.style.transform = ""));
      colPts.forEach((p) => {
        p.el.style.transformBox = "fill-box";
        p.el.style.transformOrigin = "center";
        p.el.style.transform = "scale(1.7)";
      });
      // Reference rules (data-rule) are no column value, but the crosshair names
      // them: a vertical one (an ::XLINE, "Today") at the column nearest to it,
      // a horizontal one (::REFLINE, a threshold) while the pointer is near it.
      const uy = vb.y + (e.clientY - r.top) / scale;
      const nearest = (x) => pts.reduce((a, p) => (Math.abs(p.cx - x) < Math.abs(a - x) ? p.cx : a), Infinity);
      const rules = [...svg.querySelectorAll("[data-rule]")].filter((el) => {
        const [x, y] = (el.getAttribute("points") || "").trim().split(/[\s,]+/).map(Number);
        return el.getAttribute("data-rule") === "x" ? Math.round(nearest(x)) === best : Math.abs(y - uy) * scale <= 8;
      });
      const head = colPts[0] && colPts[0].info.x ? `<div class="tip-head">${escapeHtml(colPts[0].info.x)}</div>` : "";
      tip.innerHTML = head + colPts.map((p) => tipRow(p.info, p.fill)).join("") + rules.map(ruleRow).join("");
      tip.classList.add("show");
      tip.style.left = Math.min(e.clientX + 16, window.innerWidth - 240) + "px";
      tip.style.top = e.clientY + 8 + "px";
    };
    const leave = () => {
      cross.style.display = "none";
      tip.classList.remove("show");
      area.querySelectorAll("circle.dp-hit").forEach((el) => (el.style.transform = ""));
    };
    area.addEventListener("mousemove", move);
    area.addEventListener("mouseleave", leave);
  });
}

// The position domain a rendered chart SVG actually shows: ggplot-rs writes the
// trained, expanded `data-domain="x0 x1 y0 y1"` on the root <svg> when both
// axes are continuous (absent for a discrete axis). null when unusable.
function svgDomain(svg) {
  const d = ((svg && svg.getAttribute("data-domain")) || "").trim().split(/\s+/).map(Number);
  if (d.length !== 4 || !d.every(Number.isFinite) || !(d[1] > d[0]) || !(d[3] > d[2])) return null;
  return { x0: d[0], x1: d[1], y0: d[2], y1: d[3] };
}

// Scroll-to-zoom / drag-to-pan for a continuous cartesian chart (double-click
// resets). Uses the SVG's data-plot rect (panel area in viewBox units) and its
// data-domain (the domain drawn there) to map the cursor accurately to data
// coords, and re-renders with a zoom window.
function attachCartZoom(holder, rowsJson, roles, ph) {
  // Only plain cartesian kinds honour a zoom window; flipped panels don't.
  if (!roles.some((r) => ZOOMABLE.includes(r[1]))) return;
  const svg0 = holder.querySelector("svg");
  if (!svg0 || svg0.getAttribute("data-flip") === "true") return;
  const full = svgDomain(svg0);
  if (!full) return; // not a continuous x/y chart → no zoom
  let view = null; // null = auto (full extent)
  let raf = 0;
  let syncSlider = () => {}; // set up below once the slider DOM exists
  const draw = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      const zoom = view ? JSON.stringify([view.x0, view.x1, view.y0, view.y1]) : "";
      const W = holder.querySelector("svg")?.viewBox.baseVal.width || 460; // the width the chart is drawn at
      renderChart(holder, rowsJson, roles, W, ph, zoom);
      wireCharts();
      syncSlider();
    });
  };
  const plotMap = () => {
    const svg = holder.querySelector("svg");
    if (!svg) return null;
    const pa = (svg.dataset.plot || "").split(" ").map(Number);
    if (pa.length !== 4 || !pa[2]) return null;
    const r = svg.getBoundingClientRect();
    const scale = r.width / svg.viewBox.baseVal.width;
    return { pa, r, scale };
  };
  const toData = (e, v) => {
    const m = plotMap();
    if (!m) return null;
    v = svgDomain(holder.querySelector("svg")) || v; // the domain actually drawn
    const vx = (e.clientX - m.r.left) / m.scale;
    const vy = (e.clientY - m.r.top) / m.scale;
    const fx = (vx - m.pa[0]) / m.pa[2];
    const fy = (vy - m.pa[1]) / m.pa[3];
    return { x: v.x0 + fx * (v.x1 - v.x0), y: v.y1 - fy * (v.y1 - v.y0) };
  };
  holder.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      if (!view) view = { ...full };
      const d = toData(e, view);
      if (!d) return;
      const k = e.deltaY < 0 ? 0.82 : 1 / 0.82;
      view = {
        x0: d.x + (view.x0 - d.x) * k,
        x1: d.x + (view.x1 - d.x) * k,
        y0: d.y + (view.y0 - d.y) * k,
        y1: d.y + (view.y1 - d.y) * k,
      };
      draw();
    },
    { passive: false }
  );
  let drag = null;
  holder.addEventListener("mousedown", (e) => {
    if (!view) view = { ...full };
    drag = { x: e.clientX, y: e.clientY, v: { ...view }, moved: false };
  });
  window.addEventListener("mousemove", (e) => {
    if (!drag) return;
    if (Math.abs(e.clientX - drag.x) + Math.abs(e.clientY - drag.y) > 4) drag.moved = true;
    const m = plotMap();
    if (!m) return;
    const dx = ((e.clientX - drag.x) / m.scale / m.pa[2]) * (drag.v.x1 - drag.v.x0);
    const dy = ((e.clientY - drag.y) / m.scale / m.pa[3]) * (drag.v.y1 - drag.v.y0);
    view = { x0: drag.v.x0 - dx, x1: drag.v.x1 - dx, y0: drag.v.y0 + dy, y1: drag.v.y1 + dy };
    draw();
  });
  window.addEventListener("mouseup", () => {
    if (drag && drag.moved) holder._panned = true;
    drag = null;
  });
  holder.addEventListener(
    "click",
    (e) => {
      if (holder._panned) {
        holder._panned = false;
        e.stopPropagation();
      }
    },
    true
  );
  holder.addEventListener("dblclick", () => {
    view = null;
    draw();
  });

  // ECharts-style dataZoom slider: a range bar under the chart. Drag a handle to
  // resize the x-window, the middle band to pan it, or click the track to jump.
  // Stays in sync with the wheel/drag zoom; double-click the chart still resets.
  if (ph >= 160) {
    const bar = document.createElement("div");
    bar.className = "dp-zoom";
    // Off by default — the toolbox "range slider" tool toggles it on. (Scroll /
    // drag zoom still works without it.)
    bar.style.display = "none";
    bar.innerHTML =
      '<div class="dp-zoom-track"><div class="dp-zoom-fill"></div>' +
      '<div class="dp-zoom-h dp-zoom-h0"></div><div class="dp-zoom-h dp-zoom-h1"></div></div>';
    holder.after(bar);
    bar.addEventListener("click", (e) => e.stopPropagation());
    const track = bar.querySelector(".dp-zoom-track");
    const fill = bar.querySelector(".dp-zoom-fill");
    const h0 = bar.querySelector(".dp-zoom-h0");
    const h1 = bar.querySelector(".dp-zoom-h1");
    const span = () => full.x1 - full.x0 || 1;
    const MINW = 0.02; // smallest window = 2% of the full extent
    syncSlider = () => {
      const v = view || full;
      const f0 = Math.max(0, Math.min(1, (v.x0 - full.x0) / span()));
      const f1 = Math.max(0, Math.min(1, (v.x1 - full.x0) / span()));
      fill.style.left = f0 * 100 + "%";
      fill.style.width = Math.max(0, f1 - f0) * 100 + "%";
      h0.style.left = f0 * 100 + "%";
      h1.style.left = f1 * 100 + "%";
      bar.classList.toggle("dp-zoom-on", !!view);
    };
    const setX = (x0, x1) => {
      const y = view || full;
      view = { x0, x1, y0: y.y0, y1: y.y1 };
      draw();
    };
    const fracAt = (clientX) => {
      const r = track.getBoundingClientRect();
      return Math.max(0, Math.min(1, (clientX - r.left) / (r.width || 1)));
    };
    let zd = null;
    const onMove = (e) => {
      if (!zd) return;
      const xAt = full.x0 + fracAt(e.clientX) * span();
      const min = span() * MINW;
      if (zd.mode === "h0") setX(Math.min(xAt, zd.x1 - min), zd.x1);
      else if (zd.mode === "h1") setX(zd.x0, Math.max(xAt, zd.x0 + min));
      else {
        const dx = (fracAt(e.clientX) - fracAt(zd.x)) * span();
        let nx0 = zd.x0 + dx,
          nx1 = zd.x1 + dx;
        if (nx0 < full.x0) ((nx1 += full.x0 - nx0), (nx0 = full.x0));
        if (nx1 > full.x1) ((nx0 -= nx1 - full.x1), (nx1 = full.x1));
        setX(nx0, nx1);
      }
    };
    const onUp = () => {
      zd = null;
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
    };
    const start = (mode) => (e) => {
      e.preventDefault();
      e.stopPropagation();
      const v = view || full;
      zd = { mode, x: e.clientX, x0: v.x0, x1: v.x1 };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    };
    h0.addEventListener("mousedown", start("h0"));
    h1.addEventListener("mousedown", start("h1"));
    fill.addEventListener("mousedown", start("mid"));
    track.addEventListener("mousedown", (e) => {
      if (e.target !== track) return; // handled by fill/handles otherwise
      const w = view ? view.x1 - view.x0 : span() * 0.5;
      const cx = full.x0 + fracAt(e.clientX) * span();
      setX(Math.max(full.x0, cx - w / 2), Math.min(full.x1, cx + w / 2));
    });
    syncSlider();
  }
}

// Click empty dashboard space to clear all cross-filters / selections.
document.querySelector(".dash").addEventListener("click", () => {
  if (clearSelections()) run(false);
});
// Esc does the same (unless a dialog, menu or text field has the key).
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape" || e.defaultPrevented) return;
  const el = document.activeElement;
  const tag = ((el && el.tagName) || "").toLowerCase();
  if (["input", "textarea", "select"].includes(tag) || document.querySelector(".dp-modal-back")) return;
  document.querySelectorAll(".dash .panel").forEach((p) => p._brush && toggleBrush(p)); // close an open brush
  if (clearSelections()) run(false);
});

// Before boot, so a remembered sidebar width applies without a jump.
initSideResizer();
initSideDrawer();
boot().catch((e) => status("boot failed: " + e));
