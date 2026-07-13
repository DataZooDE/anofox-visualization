// Browser dashboard builder — 100% client-side.
//   DuckDB-Wasm runs the SQL, duckplot (wasm) plans the ::ROLE annotations and
//   renders each panel to SVG. No server, no DuckDB extension.
import init, { plan, render_panel, map_bounds, panel_bounds } from "./pkg/duckplot.js";

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
SELECT '**How duckplot works:** every panel is a SQL query whose columns are tagged with \`::ROLE\` casts — \`::XAXIS\`, \`::CATEGORY\`, a chart kind like \`::BARCHART_STACKED\`, \`::PIE\`, \`::LINECHART\`, \`::TABLE\`, or a KPI (\`::COMPACT\`/\`::METRIC\`). Un-annotated statements (the \`CREATE TABLE\`) are setup. **Click a pie slice or a table row** — the KPI strip filters via \`getvariable(''selected'')\`.'::MARKDOWN, 'What this shows'::TITLE;

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

SELECT 6::COL;
SELECT 'Detail'::TITLE, channel, week, sum(n) AS n ::TABLE
FROM sessions GROUP BY ALL ORDER BY channel, week;`,

      "Signal explorer": `-- Everything below is generated on the fly with range() — no table needed.
SELECT 'No table needed — a whole dashboard from pure SQL math'::LABEL;

SELECT 12::COL;
SELECT 'The point of this example: **you don''t need a data source.** Every panel is a formula over DuckDB''s **range()** (a row-number generator), so the entire dashboard is computed on the fly — no table, file, or database. Handy for **synthetic demos, parametric what-ifs, and teaching**. It still exercises the full chart vocabulary from math alone: a **line with a ± confidence band** (::BAND_LOWER / ::BAND_UPPER), a **gauge**, a **scatter**, a **histogram**, and a **heatmap surface**, plus a KPI strip. Hover any chart for the toolbox.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 'Signal stats'::GROUP;
SELECT round(avg(sin(i/6.0)*40+50),1)::METRIC, 'Mean'::LABEL FROM range(0,120) t(i);
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
SELECT 'Charts are grouped into **tabs by family**: bars, lines & areas, scatter, distributions, pie/gauge/radar, heatmaps & candlestick, sparklines — plus an **interactivity playground** and **maps**. This tab: a **stacked bar**, a **horizontal bar** (::FLIP swaps the axes), and **bars with data labels** (::DATALABELS).'::MARKDOWN, 'What this shows'::TITLE;
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
-- Radar (::RADAR): axes from x, one polygon per ::CATEGORY series.
SELECT 6::COL; SELECT metric::XAXIS, score::RADAR, model::CATEGORY, 'Radar — model comparison'::TITLE FROM radar;

SELECT 'Heatmap, calendar & candlestick'::TAB;
SELECT 6::COL; SELECT week::XAXIS, channel::YAXIS, round(avg(value),1)::HEATMAP, 'Heatmap — week × channel'::TITLE FROM m GROUP BY ALL ORDER BY week, channel;
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
    group: "Forecasting",
    items: {
      "M5 forecast (live extension)": `-- Live in-browser SeasonalES forecast: DuckDB-Wasm loads a parquet and runs the
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
-- holdout (lx_bt), for the selected item (defaults to the largest by forecast).
SELECT 12::COL;
SELECT ds       ::XAXIS,
       'Actual' ::CATEGORY,
       y        ::LINECHART,
       y        ::BAND_LOWER,
       y        ::BAND_UPPER,
       'History + 12-month SeasonalES forecast (shaded = 95% interval) + backtest'::TITLE
FROM lx_m WHERE series=COALESCE(NULLIF(getvariable('selected'),''),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'Forecast', y, y, y, '' FROM lx_m
  WHERE series=COALESCE(NULLIF(getvariable('selected'),''),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1)) AND ds=(SELECT max(ds) FROM lx_m)
UNION ALL
SELECT ds, 'Forecast', yhat, lo, hi, '' FROM lx_fc WHERE series=COALESCE(NULLIF(getvariable('selected'),''),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'SeasonalES (backtest)', predicted, predicted, predicted, '' FROM lx_bt
  WHERE method='SeasonalES' AND series=COALESCE(NULLIF(getvariable('selected'),''),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
UNION ALL
SELECT ds, 'SeasonalNaive (backtest)', predicted, predicted, predicted, '' FROM lx_bt
  WHERE method='SeasonalNaive' AND series=COALESCE(NULLIF(getvariable('selected'),''),(SELECT item FROM lx_summary ORDER BY fc_total DESC LIMIT 1))
ORDER BY 1;

-- Two tabs share the chart above: a per-series Summary and the Backtest
-- statistics. Both tables are click-to-drill (set getvariable('selected')).
SELECT 'Summary'::TAB;
SELECT 12::COL;
SELECT 'Per-series **summary** of every forecast item — last actual, next-month and 12-month forecast totals, and growth vs the prior 12 months. **Click a row** to drill the chart above to that item.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 12::COL;
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
SELECT item    AS "Item × store" ::PAGED,
       winner  AS "Winner"       ::BADGE,
       es_mae  AS "ES · MAE"     ::COMPACT,
       sn_mae  AS "SN · MAE"     ::COMPACT,
       es_rmse AS "ES · RMSE"    ::COMPACT,
       sn_rmse AS "SN · RMSE"    ::COMPACT
FROM lx_scores ORDER BY item;`,
      "M5 analytics": `-- M5 forecast · decomposition · backtest on the anofox-forecast extension,
-- one dashboard across all categories (monthly). Method: SeasonalES.
-- Heavy steps cached with CREATE TABLE IF NOT EXISTS.
CREATE TABLE IF NOT EXISTS an_cat AS
  SELECT split_part(series,'_',1) AS category, ds, sum(y) AS y
  FROM read_parquet('m5_monthly.parquet') GROUP BY 1,2;

-- 12-month forecast (SeasonalES = seasonal exponential smoothing, so the forward
-- path carries the smoothed level plus the learned monthly seasonal pattern)
CREATE TABLE IF NOT EXISTS an_fc AS
  SELECT category, ds, round(yhat,0) AS yhat
  FROM ts_forecast_by('an_cat', category, ds, y, 'SeasonalES', 12, '1mo', MAP{'seasonal_period':'12'});

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

-- Backtest: hold out the last 12 months, forecast them from the train (SeasonalES), compare to actuals
CREATE TABLE IF NOT EXISTS an_train AS SELECT * FROM an_cat WHERE ds <= (SELECT max(ds) FROM an_cat)-INTERVAL 12 MONTH;
CREATE TABLE IF NOT EXISTS an_bt AS
  SELECT f.category, f.ds, round(f.yhat,0) AS predicted, t.y AS actual
  FROM ts_forecast_by('an_train', category, ds, y, 'SeasonalES', 12, '1mo', MAP{'seasonal_period':'12'}) f
  JOIN an_cat t ON f.category=t.category AND f.ds=t.ds;

SELECT 'M5 — forecast · decomposition · backtest (SeasonalES, all categories)'::LABEL;

SELECT 12::COL;
SELECT 'A full **time-series workflow** on M5 monthly sales aggregated to category: an at-a-glance **SeasonalES forecast** (history + 12-month prediction) and a 12-month-holdout **backtest** for all categories, then a **tab per category** (FOODS / HOBBIES / HOUSEHOLD) with an additive **decomposition** (MSTL trend + classical seasonal + remainder) and **residual diagnostics** — a histogram and a normal **Q-Q plot** of the remainder. Residuals that look normal (bell histogram, points hugging the Q-Q line) mean the trend+seasonal model captured the structure.'::MARKDOWN, 'What this shows'::TITLE;

-- Forecast overview: one small multiple per category so the forecast reads in a
-- distinct colour (Actual vs Forecast) instead of matching its own history, and
-- each panel auto-scales (FOODS ≫ HOBBIES). The forecast is bridged to the last
-- actual so the two segments join.
SELECT 'Forecast — history + 12-month SeasonalES (actual vs forecast)'::LABEL;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'FOODS'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='FOODS'
      UNION ALL SELECT ds, 'Forecast', yhat FROM an_fc WHERE category='FOODS'
      UNION ALL SELECT ds, 'Forecast', y FROM an_cat WHERE category='FOODS' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOBBIES'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOBBIES'
      UNION ALL SELECT ds, 'Forecast', yhat FROM an_fc WHERE category='HOBBIES'
      UNION ALL SELECT ds, 'Forecast', y FROM an_cat WHERE category='HOBBIES' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;
SELECT 4::COL;
SELECT ds ::XAXIS, phase ::CATEGORY, val ::LINECHART, 'HOUSEHOLD'::TITLE
FROM (SELECT ds, 'Actual' AS phase, y AS val FROM an_cat WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, 'Forecast', yhat FROM an_fc WHERE category='HOUSEHOLD'
      UNION ALL SELECT ds, 'Forecast', y FROM an_cat WHERE category='HOUSEHOLD' AND ds=(SELECT max(ds) FROM an_cat)) ORDER BY phase, ds;

-- Backtest overview: 12-month-holdout error metrics per category.
SELECT 'Backtest — 12-month holdout (SeasonalES)'::LABEL;
SELECT 12::COL;
SELECT category AS "Category" ::TABLE,
       round(avg(abs(actual-predicted)),0)                      AS "MAE"    ::COMPACT,
       round(sqrt(avg(pow(actual-predicted,2))),0)              AS "RMSE"   ::COMPACT,
       round(100*avg(abs(actual-predicted)/nullif(actual,0)),1) AS "MAPE %" ::PLAIN
FROM an_bt GROUP BY 1 ORDER BY 2;

-- Per-category deep dive (one tab each): decomposition + residual diagnostics
-- (histogram + normal Q-Q of the remainder) + the backtest fit.
SELECT 'FOODS'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='FOODS' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='FOODS';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='FOODS';
SELECT 12::COL; SELECT ds ::XAXIS, actual ::LINECHART, predicted ::LINECHART, 'Backtest — actual vs predicted'::TITLE FROM an_bt WHERE category='FOODS' ORDER BY ds;

SELECT 'HOBBIES'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='HOBBIES' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='HOBBIES';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='HOBBIES';
SELECT 12::COL; SELECT ds ::XAXIS, actual ::LINECHART, predicted ::LINECHART, 'Backtest — actual vs predicted'::TITLE FROM an_bt WHERE category='HOBBIES' ORDER BY ds;

SELECT 'HOUSEHOLD'::TAB;
SELECT 4::COL; SELECT ds ::XAXIS, observed ::LINECHART, trend ::LINECHART, 'Observed + MSTL trend'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, seasonal ::LINECHART, 'Seasonal'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 4::COL; SELECT ds ::XAXIS, remainder ::LINECHART, 'Remainder'::TITLE FROM an_decomp WHERE category='HOUSEHOLD' ORDER BY ds;
SELECT 6::COL; SELECT remainder ::HISTOGRAM, 'Residual histogram'::TITLE FROM an_decomp WHERE category='HOUSEHOLD';
SELECT 6::COL; SELECT remainder ::QQ, 'Residual normal Q-Q'::TITLE FROM an_decomp WHERE category='HOUSEHOLD';
SELECT 12::COL; SELECT ds ::XAXIS, actual ::LINECHART, predicted ::LINECHART, 'Backtest — actual vs predicted'::TITLE FROM an_bt WHERE category='HOUSEHOLD' ORDER BY ds;`,
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
SELECT 'Interactive **controls** driven entirely from SQL: a dropdown, two multiselects (the 80-option one is **searchable**), a slider (::NUMBER), free text, and a date. Each control''s output **column name becomes a DuckDB variable** — the charts read it with **getvariable(''name'')**, so changing a control re-runs their queries live. (Date range and cross-filter are on the next tabs.)'::MARKDOWN, 'What this shows'::TITLE;

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
SELECT 'Two **independent named cross-filters**. Each table emits its own variable (named after its first column — sku / market); **click a SKU and a market** and the KPI + chart below filter by *both* at once. This is finer-grained than the single generic click-to-filter — you compose several selections. Click a selected row again to clear it.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 4::COL; SELECT sku, sum(amount) AS total ::TABLE FROM sales2 GROUP BY sku ORDER BY total DESC;
SELECT 4::COL; SELECT region AS market, sum(amount) AS total ::TABLE FROM sales2 GROUP BY region ORDER BY total DESC;

SELECT 4::COL;
SELECT sum(amount)::METRIC, 'Total (filtered)'::LABEL FROM sales2
WHERE (COALESCE(getvariable('sku'),'') = '' OR sku = getvariable('sku'))
  AND (COALESCE(getvariable('market'),'') = '' OR region = getvariable('market'));

SELECT 12::COL;
SELECT month::XAXIS, sum(amount)::LINECHART, 'Monthly (by SKU & market)'::TITLE FROM sales2
WHERE (COALESCE(getvariable('sku'),'') = '' OR sku = getvariable('sku'))
  AND (COALESCE(getvariable('market'),'') = '' OR region = getvariable('market'))
GROUP BY month ORDER BY month;`,
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
SELECT 'A **rich data table** — each column carries its own formatting role: **::MONEY** currency, **::COLORSCALE** heat-shaded cells, **::BADGE** status pills, **::TREND** an up/down arrow, and an in-cell **::SPARKLINE**. One SELECT, per-column presentation.'::MARKDOWN, 'What this shows'::TITLE;
SELECT 12::COL;
SELECT sku::TABLE,
       forecast::MONEY,
       mape AS "MAPE %" ::COLORSCALE,
       status::BADGE,
       growth AS "growth %" ::TREND,
       (SELECT list(sales ORDER BY m) FROM hist WHERE hist.sku = fc.sku) AS trend ::SPARKLINE
FROM fc ORDER BY forecast DESC;

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
// making it interactive → tables → page layout → the advanced live-forecasting
// extension last (so a missing extension never blocks the core demos).
const GROUP_ORDER = ["Start here", "Charts", "Interactivity", "Tables", "Layout", "Forecasting"];
SAMPLE_GROUPS.sort((a, b) => GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group));

const SAMPLES = Object.fromEntries(SAMPLE_GROUPS.flatMap((g) => Object.entries(g.items)));

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

// Forecast example: load a small M5 parquet into the browser DB, and (for
// statements that need it) INSTALL/LOAD the anofox-forecast COMMUNITY extension
// so ts_forecast_by() runs client-side. Needs a v1.5.x DuckDB-Wasm engine
// (matches the community wasm build). The parquet always loads; the extension
// step is separate because its wasm build currently fails to LOAD in the browser
// (DataZooDE/anofox-forecast#239) — we surface that clearly instead of a
// cryptic "function does not exist".
let fcParquet = null;
let fcExt = null;
async function ensureForecast(sql) {
  if (backend === "live") return; // native backend resolves paths + extension itself
  if (!db) throw new Error("in-browser DuckDB not ready");
  // 1. Register the parquet (memoised) — this is the "load the data" step.
  if (!fcParquet) {
    fcParquet = (async () => {
      const bytes = new Uint8Array(await (await fetch("m5_monthly.parquet")).arrayBuffer());
      await db.registerFileBuffer("m5_monthly.parquet", bytes);
    })().catch((e) => {
      fcParquet = null;
      throw e;
    });
  }
  await fcParquet;
  // 2. Load the community extension only for statements that use it (memoised;
  //    a failure is remembered so we don't re-hammer the CDN each panel).
  if (/\bts_\w+\b|\banofox_forecast\b/i.test(sql)) {
    if (!fcExt) {
      fcExt = (async () => {
        // json (from the default repo) is needed by some anofox macros
        // (e.g. ts_mstl_decomposition_by parses JSON params).
        try {
          await conn.query("INSTALL json; LOAD json;");
        } catch (_) {
          /* usually bundled */
        }
        // Load our locally-built wasm extension (served from web/localext/ in the
        // repo layout <version>/<platform>/). The signed community build doesn't
        // link against this runtime (#239); this local build's imports do.
        const repo = new URL("localext", document.baseURI).href.replace(/\/$/, "");
        await conn.query(`SET custom_extension_repository='${repo}';`);
        await conn.query("INSTALL anofox_forecast;");
        await conn.query("LOAD anofox_forecast;");
        // Restore the default repo so other autoloaded extensions (parquet,
        // spatial, …) still resolve — our repo only hosts anofox_forecast.
        await conn.query("RESET custom_extension_repository;");
      })().catch((e) => {
        throw new Error(
          "Couldn't load the anofox_forecast wasm extension in the browser. The " +
            "parquet is loaded and queryable; run ts_forecast_by() on a native backend. " +
            "Underlying error: " +
            (e.message || e)
        );
      });
    }
    await fcExt;
  }
}

// Run one SQL statement and return its rows as a JSON string ([{c0,…}, …]).
async function runSql(sql) {
  if (/\bST_Read\b|\bspatial\b/i.test(sql)) await ensureGeo();
  if (/m5_monthly|\bts_\w+\b|\banofox_forecast\b/i.test(sql)) await ensureForecast(sql);
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
  await init(); // duckplot wasm (plan + render_panel — used in both modes)

  // Prefer a live DuckDB bridge (served by `duckplot serve`); else DuckDB-Wasm.
  try {
    const r = await fetch("/query", { method: "POST", body: "SELECT 1 AS ok" });
    if (r.ok) backend = "live";
  } catch (_) {}

  if (backend !== "live") {
    const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.33.1-dev57.0/+esm");
    const bundle = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" })
    );
    db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), new Worker(workerUrl));
    await db.instantiate(bundle.mainModule, bundle.pthreadWorker);
    // Allow unsigned extensions so we can load our locally-built anofox_forecast
    // wasm (the signed community build doesn't link against this runtime — #239).
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
  renderSidebar();
  $("side-new").onclick = () => loadDash("", "");
  $("save").onclick = saveDash;
  $("side-toggle").onclick = () => document.body.classList.toggle("side-collapsed");
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
  //   ?dashboard=Name load a saved/example dashboard by name
  //   #sql=<base64>   inline SQL (from Share)
  const params = new URLSearchParams(location.search);
  const primary = params.get("primary");
  if (primary && /^#?[0-9a-fA-F]{6}$/.test(primary)) {
    const hex = primary.replace(/^#/, "");
    document.documentElement.style.setProperty("--accent", "#" + hex);
    document.documentElement.style.setProperty("--accent2", "#" + hex);
    dpPrimary = hex; // chart primary colour (passed to the wasm renderer)
  }
  if (params.get("embed") === "1" || params.has("embed")) {
    document.body.classList.add("embed");
  }
  const hashSql = decodeHashSql();
  const wantDash = params.get("dashboard");
  const savedItems = dashStore().items;
  if (hashSql) {
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
  $("dark").onclick = () => document.body.classList.toggle("dark");
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
  if (!exc.__all__) {
    // Group headers are static labels — only the master "Examples" collapses.
    for (const g of SAMPLE_GROUPS) {
      const hdr = document.createElement("div");
      hdr.className = "side-section side-sub";
      hdr.textContent = g.group;
      nav.appendChild(hdr);
      for (const [n, sql] of Object.entries(g.items)) {
        const it = sideItem(n, sql, false);
        it.classList.add("side-ex-item"); // indent examples under their group header
        nav.appendChild(it);
      }
    }
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
  dpSubTab = {};
  dpFilter = "";
  dpSelected = null;
  dpXf = {};
  dpPage = {};
  dpSort = {};
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
const INPUTS = ["DROPDOWN", "NUMBER", "DATE", "TEXT", "MULTISELECT", "DATERANGE"];
const inputKind = (s) => INPUTS.find((k) => role(s, k));
const isInput = (s) => !!inputKind(s);
const METRICS = ["METRIC", "MONEY", "PERCENT", "COMPACT"];
const metricRole = (s) => s.roles.find((r) => METRICS.includes(r[1]));
const isHeading = (s) => s.roles.length === 1 && s.roles[0][1] === "LABEL";
const directive = (s) =>
  ["COLUMNS", "GROUP", "ENDGROUP", "SPAN", "HEIGHT", "TAB", "SUBTAB", "PLACEHOLDER"].find((d) => role(s, d));
let dpVars = {}; // DuckDB variable name -> selected value (persists across runs)
let dpCols = 2; // default panels-per-row on the 12-column grid
let dpFilter = ""; // generic cross-filter: last clicked value, as getvariable('selected')
let dpXf = {}; // named cross-filters: column-name -> value, each getvariable('<name>')
let dpPage = {}; // ::PAGED tables: statement index -> current page (server-side)
let dpSort = {}; // ::PAGED tables: statement index -> {col, dir} (server-side sort)
let dpKbdActive = false; // one-shot: keep row highlight after a server page-change
let dpNav = null; // arrow-key controller for the table the pointer is over
let dpPageSize = {}; // ::PAGED tables: statement idx -> rows per page
let dpFilterText = {}; // ::PAGED tables: statement idx -> full-text filter term

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
let dpTimer = null; // auto-refresh interval handle
let dpPrimary = null; // optional brand colour (hex, no #) for chart primary

// `fresh` = a full run (Run button / load): execute every setup statement.
// A cross-filter re-run (selecting a row, an input, clearing) passes fresh=false
// so we SKIP setup that doesn't depend on the selection — e.g. an expensive
// CREATE OR REPLACE TABLE fc AS ts_forecast_by(...) is computed once, not on
// every row click. Setup that references getvariable() still re-runs.
async function run(fresh = true) {
  const grid = $("grid");
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

  // Cross-filter values — the generic `selected` (last click) plus any NAMED
  // cross-filters (each table emits getvariable('<its first column>') so two
  // tables give two independent live selections). Unset named vars read as NULL,
  // so targets guard with COALESCE(getvariable('name'),'').
  try {
    await runSql(`SET VARIABLE selected = '${dpFilter.replace(/'/g, "''")}'`);
    for (const [k, v] of Object.entries(dpXf)) {
      if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(k)) {
        await runSql(`SET VARIABLE ${k} = '${String(v).replace(/'/g, "''")}'`);
      }
    }
  } catch (e) {
    /* ignore */
  }

  // Pre-pass: run setup + set each dropdown's DuckDB variable (before charts),
  // caching options for the render pass by statement index.
  const dd = {};
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    try {
      if (s.setup) {
        // On a cross-filter re-run, skip selection-independent setup (it already
        // ran on the last full run) so clicks don't recompute forecasts etc.
        if (!fresh && !/getvariable/i.test(s.sql)) continue;
        await runSql(s.sql);
      } else if (isInput(s)) {
        const kind = inputKind(s);
        const rows = JSON.parse(await runSql(s.sql));
        if (!rows.length) continue;
        if (kind === "DATERANGE") {
          const keys = Object.keys(rows[0]);
          const fk = keys[0];
          const tk = keys[1] || keys[0];
          if (dpVars[fk] === undefined) dpVars[fk] = String(rows[0][fk] ?? "");
          if (dpVars[tk] === undefined) dpVars[tk] = String(rows[0][tk] ?? "");
          dd[i] = { kind, varnames: [fk, tk] };
          await runSql(`SET VARIABLE ${fk} = '${String(dpVars[fk]).replace(/'/g, "''")}'`);
          await runSql(`SET VARIABLE ${tk} = '${String(dpVars[tk]).replace(/'/g, "''")}'`);
          continue;
        }
        const varname = Object.keys(rows[0])[0];
        let lit;
        if (kind === "DROPDOWN") {
          const options = rows.map((r) => String(r[varname]));
          if (dpVars[varname] === undefined || !options.includes(dpVars[varname])) dpVars[varname] = options[0];
          // Optional ::HINT column → a hint shown next to each option.
          const hr = s.roles.find((r) => r[1] === "HINT");
          const hints = hr ? rows.map((r) => String(r["c" + hr[0]] ?? "")) : null;
          dd[i] = { kind, varname, options, hints };
          lit = `'${String(dpVars[varname]).replace(/'/g, "''")}'`;
        } else if (kind === "MULTISELECT") {
          const options = rows.map((r) => String(r[varname]));
          if (!Array.isArray(dpVars[varname])) dpVars[varname] = options.slice(); // default: all
          dpVars[varname] = dpVars[varname].filter((v) => options.includes(v));
          dd[i] = { kind, varname, options };
          lit = "[" + dpVars[varname].map((v) => `'${String(v).replace(/'/g, "''")}'`).join(",") + "]";
        } else {
          // number / date / text: the query's value is the default
          if (dpVars[varname] === undefined) dpVars[varname] = String(rows[0][varname] ?? "");
          dd[i] = { kind, varname };
          const v = String(dpVars[varname]);
          lit = kind === "NUMBER" ? v || "0" : `'${v.replace(/'/g, "''")}'`; // number unquoted
        }
        await runSql(`SET VARIABLE ${varname} = ${lit}`);
      }
    } catch (e) {
      showError(newGrid, `${s.sql}\n\n${e}`);
    }
  }

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
  let defaultSpan = Math.max(1, Math.round(12 / dpCols)); // 12-col bootstrap default
  let panels = 0;
  const firstValue = async (s) => {
    const rows = JSON.parse(await runSql(s.sql));
    return rows[0] ? Object.values(rows[0])[0] : null;
  };
  for (let i = 0; i < stmts.length; i++) {
    const s = stmts[i];
    if (s.setup) continue;
    const d = directive(s);
    try {
      if (d === "COLUMNS") {
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
          tabWrap.querySelectorAll(".tabpane").forEach((p) => (p.style.display = "none"));
          tabBar.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
          pane.style.display = "";
          btn.classList.add("active");
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
      } else if (isInput(s)) {
        if (dd[i]) container.appendChild(makeControl(dd[i], container === curGrid));
      } else if (role(s, "PAGED")) {
        // SQL-driven pagination: only ONE page (+ a COUNT) is fetched, so the
        // browser never holds the whole table. LIMIT/OFFSET + ORDER BY run in
        // DuckDB — the same over a huge parquet in S3 or MotherDuck.
        const span = Math.min(12, nextSpan || defaultSpan);
        const fig = document.createElement("figure");
        fig.className = "panel";
        if (container === curGrid) fig.style.gridColumn = `span ${span}`;
        const TFMT = ["MONEY", "PERCENT", "COMPACT", "METRIC", "TREND", "COLORSCALE", "BADGE", "SPARKLINE", "PLAIN"];
        const fmtByIdx = {};
        for (const [ix, r] of s.roles) if (TFMT.includes(r)) fmtByIdx[ix] = r;
        const titleRole = s.roles.find((r) => r[1] === "TITLE");
        const titleIdx = titleRole ? titleRole[0] : -1;
        const base = s.sql;
        const idx = i;
        const titleHolder = document.createElement("div");
        // Server-side full-text filter: one WHERE across all columns, re-queried
        // (with a fresh COUNT) as you type, so it scales like the paging itself.
        const filterInput = document.createElement("input");
        filterInput.className = "dp-table-filter";
        filterInput.type = "search";
        filterInput.placeholder = "Filter all rows…";
        filterInput.value = dpFilterText[idx] || "";
        const holder = document.createElement("div");
        fig.append(titleHolder, filterInput, holder);
        const qident = (c) => `"${String(c).replace(/"/g, '""')}"`;
        let cachedTotal = null;
        const whereClause = () => {
          const q = (dpFilterText[idx] || "").trim();
          // Cast the whole row (the subquery alias is a STRUCT of all columns) to
          // text and match — a generic full-text filter over every column.
          return q ? ` WHERE CAST(_dp AS VARCHAR) ILIKE '%${q.replace(/'/g, "''")}%'` : "";
        };
        const load = async () => {
          const pageSize = dpPageSize[idx] || 10;
          const page = dpPage[idx] || 0;
          const sort = dpSort[idx];
          const where = whereClause();
          if (cachedTotal == null) {
            try {
              const c = JSON.parse(await runSql(`SELECT count(*) AS n FROM (${base}) _dp${where}`));
              cachedTotal = Number(c[0] && c[0].n) || 0;
            } catch (_) {
              cachedTotal = 0;
            }
          }
          const order = sort && sort.col ? ` ORDER BY ${qident(sort.col)} ${sort.dir > 0 ? "ASC" : "DESC"}` : "";
          let rows = [];
          try {
            rows = JSON.parse(await runSql(`SELECT * FROM (${base}) _dp${where}${order} LIMIT ${pageSize} OFFSET ${page * pageSize}`));
          } catch (e) {
            holder.innerHTML = "";
            showError(holder, String(e));
            return;
          }
          titleHolder.innerHTML = "";
          holder.innerHTML = "";
          let sk = -1;
          if (titleIdx >= 0 && rows.length) {
            const tv = Object.values(rows[0])[titleIdx];
            if (tv != null) titleHolder.appendChild(mkTitle(String(tv).replace(/^"|"$/g, "")));
            sk = titleIdx;
          }
          const server = {
            total: cachedTotal,
            page,
            pageSize,
            sortCol: sort ? sort.col : null,
            sortDir: sort ? sort.dir : 1,
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
          };
          holder.appendChild(renderTable(rows, sk, fmtByIdx, server));
        };
        let filterTimer = null;
        filterInput.oninput = () => {
          clearTimeout(filterTimer);
          filterTimer = setTimeout(() => {
            dpFilterText[idx] = filterInput.value;
            cachedTotal = null; // filter changes the row count
            dpPage[idx] = 0;
            load();
          }, 180);
        };
        await load();
        container.appendChild(fig);
        panels++;
        nextSpan = 0;
        nextHeight = 0;
      } else {
        const rowsJson = await runSql(s.sql);
        const span = Math.min(12, nextSpan || defaultSpan);
        const boxH = nextHeight;
        const mkPanel = () => {
          const fig = document.createElement("figure");
          fig.className = "panel";
          if (container === curGrid) fig.style.gridColumn = `span ${span}`;
          if (boxH) fig.style.minHeight = boxH + "px";
          return fig;
        };
        const firstCell = () => {
          const r = JSON.parse(rowsJson)[0];
          return r ? String(Object.values(r)[0] ?? "").replace(/^"|"$/g, "") : "";
        };
        if (isHeading(s)) {
          const rows = JSON.parse(rowsJson);
          const h = document.createElement("h2");
          h.className = "section";
          h.textContent = rows[0] ? Object.values(rows[0])[0] : "";
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
          img.src = firstCell();
          container.appendChild(img);
        } else if (role(s, "FOOTER_LINK")) {
          const rows = JSON.parse(rowsJson);
          const vals = rows[0] ? Object.values(rows[0]).map((v) => String(v ?? "")) : [""];
          const a = document.createElement("a");
          a.className = "footer-link";
          a.href = vals[0];
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
          const body = document.createElement("div");
          body.className = "md-body";
          body.innerHTML = renderMarkdown(mdr ? r0["c" + mdr[0]] : firstCell());
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
          const rows = JSON.parse(rowsJson);
          const fig = mkPanel();
          const tr = s.roles.find((r) => r[1] === "TITLE");
          let skip = -1;
          if (tr && rows.length) {
            skip = tr[0];
            const tv = Object.values(rows[0])[skip];
            if (tv != null) fig.appendChild(mkTitle(String(tv).replace(/^"|"$/g, "")));
          }
          // Per-column formatting (::MONEY/::PERCENT/::COMPACT/::METRIC number
          // formats, ::TREND arrows, ::COLORSCALE heatmap cells, ::BADGE pills,
          // ::SPARKLINE mini charts), keyed by output column index.
          const TFMT = ["MONEY", "PERCENT", "COMPACT", "METRIC", "TREND", "COLORSCALE", "BADGE", "SPARKLINE", "PLAIN"];
          const fmtByIdx = {};
          for (const [idx, r] of s.roles) if (TFMT.includes(r)) fmtByIdx[idx] = r;
          fig.appendChild(renderTable(rows, skip, fmtByIdx));
          container.appendChild(fig);
          panels++;
        } else if (metricRole(s)) {
          const r0 = JSON.parse(rowsJson)[0] || {};
          const mr = metricRole(s);
          const lr = s.roles.find((r) => r[1] === "LABEL");
          const dr = s.roles.find((r) => r[1] === "DELTA");
          const fig = mkPanel();
          fig.classList.add("metric");
          let deltaHtml = "";
          if (dr) {
            const cur = parseFloat(r0["c" + mr[0]]);
            const prev = parseFloat(r0["c" + dr[0]]);
            if (!isNaN(cur) && !isNaN(prev) && prev !== 0) {
              const pct = ((cur - prev) / Math.abs(prev)) * 100;
              const up = pct >= 0;
              deltaHtml =
                `<div class="metric-delta ${up ? "up" : "down"}">${up ? "▲" : "▼"} ` +
                `${Math.abs(pct).toLocaleString(undefined, { maximumFractionDigits: 1 })}%</div>`;
            }
          }
          fig.innerHTML =
            `<div class="metric-value">${fmtNum(r0["c" + mr[0]], mr[1])}</div>` +
            deltaHtml +
            `<div class="metric-cap">${escapeHtml(lr ? r0["c" + lr[0]] : "")}</div>`;
          container.appendChild(fig);
          panels++;
        } else {
          const fig = mkPanel();
          const t = titleOf(s, rowsJson);
          if (t) fig.appendChild(mkTitle(t));
          // A selection that filters everything out renders a clean note, not a
          // broken/empty chart box.
          if (!JSON.parse(rowsJson).length) {
            fig.appendChild(mkNoData());
          } else {
            const ph = boxH || (role(s, "SPARKLINE") ? 90 : 300); // ::HEIGHT, else default
            const isMap = s.roles.some((r) => r[1] === "MAP");
            const holder = document.createElement("div");
            holder.className = "panel-svg";
            holder.innerHTML = render_panel(rowsJson, JSON.stringify(s.roles), 460, ph, dpPrimary || "", "");
            fig.appendChild(holder);
            // Stash the panel's data/roles so the toolbox (data view, chart-type
            // toggle) can reach them without re-querying.
            fig._dp = { rows: rowsJson, roles: s.roles, ph, isMap };
            // Maps + continuous cartesian charts are scroll-to-zoom / drag-to-pan
            // (double-click resets).
            if (isMap) attachMapZoom(holder, rowsJson, s.roles, ph);
            else attachCartZoom(holder, rowsJson, s.roles, ph); // self-skips discrete x
          }
          container.appendChild(fig);
          panels++;
        }
        nextSpan = 0;
        nextHeight = 0;
      }
    } catch (e) {
      showError(container, `${s.sql}\n\n${e}`);
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
  // Atomic swap: replace the visible content in one synchronous step (no flash).
  grid.replaceChildren(...newGrid.childNodes);
  const dash = document.querySelector(".dash");
  dash.querySelectorAll(".tabbar,.tabwrap").forEach((e) => e.remove());
  if (tabBar) dash.append(tabBar, tabWrap);
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
  attachHover();
  addExportButtons();
  if (fresh) animateIn(); // entrance animation on load/Run only — cross-filter stays instant
  status(`${panels} panel${panels === 1 ? "" : "s"}`);
  document.body.classList.remove("loading");
}

// Entrance animation on a fresh render: panels fade+rise in and line series
// "draw" left-to-right (stroke-dashoffset). Skipped for cross-filter re-runs so
// interactions feel instant, and honours prefers-reduced-motion via CSS.
function animateIn() {
  document.querySelectorAll(".dash .panel").forEach((panel) => {
    panel.classList.remove("dp-in");
    void panel.offsetWidth; // reflow so the animation restarts
    panel.classList.add("dp-in");
    panel.querySelectorAll("svg polyline").forEach((pl) => {
      let len;
      try {
        len = pl.getTotalLength();
      } catch (_) {
        return;
      }
      if (!len || !isFinite(len)) return;
      pl.style.strokeDasharray = len;
      pl.style.strokeDashoffset = len;
      pl.classList.add("dp-draw");
    });
  });
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

function makeControl(meta, bar) {
  const wrap = document.createElement("label");
  wrap.className = "control";
  wrap.textContent = (meta.varname || "date") + ":";
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
  let input;
  if (meta.kind === "DROPDOWN") {
    input = document.createElement("select");
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
    cap.textContent = (meta.varname || "") + ":";
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
      sum.textContent =
        sel.size === 0 ? "None" : sel.size === options.length ? "All" : `${sel.size} selected`;
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

function mkTitle(text) {
  const c = document.createElement("figcaption");
  c.className = "panel-title";
  c.textContent = text;
  return c;
}

function mkNoData() {
  const d = document.createElement("div");
  d.className = "nodata";
  d.textContent = "No data for this selection";
  return d;
}

const cleanNum = (v) => {
  if (v == null) return null;
  if (typeof v === "number") return v;
  const n = parseFloat(String(v).replace(/^"|"$/g, ""));
  return isNaN(n) ? null : n;
};

// A ::TABLE result → a sortable HTML table with in-cell bars + per-column
// formatting (fmtByIdx maps an output column index to a format role). Pass
// `server` = {total, page, pageSize, onPage, onSort, sortCol, sortDir} for
// SQL-driven pagination/sorting (::PAGED); otherwise it paginates client-side.
function renderTable(rows, skip = -1, fmtByIdx = {}, server = null) {
  const t = document.createElement("table");
  t.className = "dp-table";
  if (!rows.length) {
    t.textContent = "(no rows)";
    return t;
  }
  const allKeys = Object.keys(rows[0]);
  const colFmt = {}; // column key -> format role
  for (const [idx, f] of Object.entries(fmtByIdx)) colFmt[allKeys[idx]] = f;
  const cols = allKeys.filter((_, i) => i !== skip);
  const numeric = {};
  const maxAbs = {};
  const colMin = {};
  const colMax = {};
  for (const c of cols) {
    const nums = rows.map((r) => cleanNum(r[c]));
    const numFmt = ["MONEY", "PERCENT", "COMPACT", "METRIC", "COLORSCALE", "TREND", "PLAIN"].includes(colFmt[c]);
    numeric[c] = numFmt || (nums.some((v) => v != null) && nums.every((v) => v == null || !isNaN(v)));
    maxAbs[c] = Math.max(1, ...nums.map((v) => Math.abs(v) || 0));
    const fin = nums.filter((v) => v != null);
    colMin[c] = fin.length ? Math.min(...fin) : 0;
    colMax[c] = fin.length ? Math.max(...fin) : 1;
  }
  let sortCol = server ? server.sortCol : null;
  let dir = server ? server.sortDir || 1 : 1;
  const hr = t.createTHead().insertRow();
  cols.forEach((c) => {
    const th = document.createElement("th");
    th.style.cursor = "pointer";
    th.onclick = (e) => {
      if (server) {
        e.stopPropagation();
        server.onSort(c); // server re-queries with ORDER BY + reloads this panel
        return;
      }
      dir = sortCol === c ? -dir : 1;
      sortCol = c;
      head();
      body();
    };
    hr.appendChild(th);
  });
  const tb = t.createTBody();
  let page = 0;
  let pageSize = server ? server.pageSize : 10;
  let filtered = rows; // client-side text filter narrows this subset
  const sortedRows = () => {
    if (server) return rows; // already sorted + paged server-side
    const data = filtered.slice();
    if (sortCol) {
      const num = numeric[sortCol];
      data.sort((a, b) =>
        num
          ? ((cleanNum(a[sortCol]) || 0) - (cleanNum(b[sortCol]) || 0)) * dir
          : String(a[sortCol]).localeCompare(String(b[sortCol])) * dir
      );
    }
    return data;
  };
  const head = () => cols.forEach((c, i) => (hr.cells[i].textContent = c + (c === sortCol ? (dir > 0 ? " ▲" : " ▼") : "")));
  const body = () => {
    tb.innerHTML = "";
    const data = sortedRows();
    if (server) {
      updateFoot(server.total, Math.max(1, Math.ceil(server.total / pageSize)));
    } else {
      const pages = Math.max(1, Math.ceil(data.length / pageSize));
      page = Math.min(page, pages - 1);
      updateFoot(data.length, pages);
    }
    const pageRows = server ? data : data.slice(page * pageSize, (page + 1) * pageSize);
    for (const r of pageRows) {
      const tr = tb.insertRow();
      // A categorical first column makes the row a cross-filter source. Clicking
      // sets BOTH the generic getvariable('selected') AND a NAMED cross-filter
      // getvariable('<first column name>') — so two tables with different first
      // columns drive two independent live selections. Click again / the
      // background to clear. Each table highlights its OWN named selection.
      const key = cols[0];
      if (!numeric[key]) {
        const keyVal = String(r[key] ?? "").replace(/^"|"$/g, "");
        tr.style.cursor = "pointer";
        const own = dpXf[key] !== undefined ? dpXf[key] : dpSelected;
        if (own && keyVal === own) tr.classList.add("row-sel");
        tr.onclick = (e) => {
          if (document.body.classList.contains("mode-explore")) return; // preview tables aren't cross-filters
          e.stopPropagation();
          const on = (dpXf[key] ?? "") !== keyVal;
          dpXf[key] = on ? keyVal : "";
          dpFilter = on ? keyVal : "";
          dpSelected = dpFilter || null;
          run(false);
        };
      }
      for (const c of cols) {
        const td = tr.insertCell();
        let v = r[c];
        if (typeof v === "string" && /^"-?[\d.]+"$/.test(v)) v = v.slice(1, -1);
        const f = colFmt[c];
        if (f === "SPARKLINE") {
          td.className = "spark-cell";
          td.innerHTML = cellSpark(v);
          continue;
        }
        if (f === "BADGE") {
          td.innerHTML = v == null ? "" : badgeHtml(unq(v));
          continue;
        }
        if (f === "TREND") {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          if (n != null) {
            td.innerHTML =
              `<span class="trend ${n >= 0 ? "up" : "down"}">${n >= 0 ? "▲" : "▼"} ` +
              `${Math.abs(n).toLocaleString(undefined, { maximumFractionDigits: 1 })}</span>`;
          }
          continue;
        }
        if (f === "PLAIN") {
          // a numeric column with NO in-cell bar (::PLAIN / ::NOBAR)
          td.textContent = v == null ? "" : v;
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          continue;
        }
        if (["MONEY", "PERCENT", "COMPACT", "METRIC", "COLORSCALE"].includes(f)) {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          td.textContent =
            n == null ? (v == null ? "" : v) : f === "COLORSCALE" ? n.toLocaleString(undefined, { maximumFractionDigits: 2 }) : fmtNum(n, f);
          if (f === "COLORSCALE" && n != null) {
            td.style.background = heatColor((n - colMin[c]) / (colMax[c] - colMin[c] || 1));
            td.style.fontWeight = "600";
          }
          continue;
        }
        td.textContent = v == null ? "" : v;
        if (numeric[c]) {
          const n = cleanNum(v);
          td.style.textAlign = "right";
          td.style.fontVariantNumeric = "tabular-nums";
          if (n != null) {
            const pct = (Math.abs(n) / maxAbs[c]) * 100;
            td.style.background = `linear-gradient(90deg, rgba(42,157,143,.16) 0, rgba(31,140,166,.14) ${pct}%, transparent ${pct}%)`;
          }
        }
      }
    }
  };
  // Client-side pagination (50/page) so thousands of rows stay responsive; the
  // full result is kept for sorting and CSV export.
  const wrap = document.createElement("div");
  wrap.className = "table-wrap";
  // A per-table text filter across all columns. Client tables filter the held
  // rows here; ::PAGED tables filter server-side (handled by the caller), so we
  // only add the box for client tables big enough to warrant it.
  if (!server && rows.length > 10) {
    const flt = document.createElement("input");
    flt.className = "dp-table-filter";
    flt.type = "search";
    flt.placeholder = "Filter rows…";
    flt.onclick = (e) => e.stopPropagation();
    flt.oninput = () => {
      const q = flt.value.trim().toLowerCase();
      filtered = q
        ? rows.filter((r) => cols.some((c) => String(r[c] ?? "").toLowerCase().includes(q)))
        : rows;
      page = 0;
      body();
    };
    wrap.appendChild(flt);
  }
  wrap.appendChild(t);
  t._rows = rows;
  t._cols = cols;
  const foot = document.createElement("div");
  foot.className = "table-foot";
  wrap.appendChild(foot);
  function updateFoot(total, pages) {
    // Hide the pager only for genuinely small tables (≤ the smallest page size),
    // so the rows-per-page control stays available for paged tables.
    if (total <= 10) {
      foot.style.display = "none";
      return;
    }
    foot.style.display = "";
    foot.innerHTML = "";
    const cur = server ? server.page : page;
    const from = cur * pageSize + 1;
    const to = Math.min(total, (cur + 1) * pageSize);
    const mk = (label, disabled, fn) => {
      const btn = document.createElement("button");
      btn.className = "page-btn";
      btn.textContent = label;
      btn.disabled = disabled;
      btn.onclick = (e) => {
        e.stopPropagation();
        if (server) {
          fn(); // server.onPage handles the re-query + re-render
        } else {
          fn();
          body();
        }
      };
      return btn;
    };
    const info = document.createElement("span");
    info.className = "table-info";
    info.textContent = `${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()}`;
    // Rows-per-page selector.
    const sizeSel = document.createElement("select");
    sizeSel.className = "page-size";
    for (const nn of [10, 20, 50, 100]) {
      const o = document.createElement("option");
      o.value = String(nn);
      o.textContent = `${nn} / page`;
      if (nn === pageSize) o.selected = true;
      sizeSel.appendChild(o);
    }
    sizeSel.onchange = (e) => {
      e.stopPropagation();
      const nn = Number(sizeSel.value);
      if (server) server.onPageSize(nn);
      else {
        pageSize = nn;
        page = 0;
        body();
      }
    };
    foot.append(
      mk("◀", cur === 0, () => (server ? server.onPage(cur - 1) : (page = Math.max(0, page - 1)))),
      info,
      mk("▶", cur >= pages - 1, () => (server ? server.onPage(cur + 1) : (page = Math.min(pages - 1, page + 1)))),
      sizeSel
    );
  }
  // Arrow-key navigation while the pointer is over the table (no click/focus
  // needed, so browsing rows never triggers a re-run): ↑/↓ move the highlighted
  // row, ←/→ (PageUp/Down) page, Enter drills into the row (cross-filter),
  // Home/End jump. Registered on the shared dpNav handler on hover.
  t.classList.add("dp-kbd");
  let focusIdx = -1;
  const applyFocus = () => {
    [...tb.rows].forEach((r, i) => r.classList.toggle("row-focus", i === focusIdx));
    if (focusIdx >= 0) tb.rows[focusIdx]?.scrollIntoView({ block: "nearest" });
  };
  const totalPages = () =>
    server ? Math.max(1, Math.ceil((server.total || 0) / pageSize)) : Math.max(1, Math.ceil(rows.length / pageSize));
  const gotoPage = (p) => {
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
  const startFocus = () => {
    if (focusIdx < 0) {
      const sel = [...tb.rows].findIndex((r) => r.classList.contains("row-sel"));
      focusIdx = sel >= 0 ? sel : 0;
      applyFocus();
    }
  };
  const controller = {
    move: (d) => {
      const n = tb.rows.length;
      if (!n) return;
      if (d === "home") focusIdx = 0;
      else if (d === "end") focusIdx = n - 1;
      else focusIdx = Math.max(0, Math.min(n - 1, (focusIdx < 0 ? 0 : focusIdx) + d));
      applyFocus();
    },
    page: (d) => gotoPage((server ? server.page : page) + d),
    drill: () => tb.rows[focusIdx]?.click(),
  };
  t.addEventListener("mouseenter", () => {
    dpNav = controller;
    startFocus();
  });
  t.addEventListener("mouseleave", () => {
    if (dpNav === controller) dpNav = null;
  });
  // After a server page-change the panel is rebuilt; keep this table navigable.
  if (server && dpKbdActive) {
    dpKbdActive = false;
    dpNav = controller;
    focusIdx = 0;
    requestAnimationFrame(applyFocus);
  }

  head();
  body();
  return wrap;
}

// ::COLORSCALE cell background — a diverging green→amber→red scale by the
// normalized value t∈[0,1] (low = green, high = red). Soft tones keep the cell
// text readable.
function heatColor(t) {
  t = Math.max(0, Math.min(1, t));
  const stops = [
    [0x63, 0xc9, 0x7f], // green
    [0xff, 0xe0, 0x8a], // amber
    [0xff, 0x8a, 0x8a], // red
  ];
  const seg = t < 0.5 ? 0 : 1;
  const u = t < 0.5 ? t / 0.5 : (t - 0.5) / 0.5;
  const a = stops[seg];
  const b = stops[seg + 1];
  const m = (i) => Math.round(a[i] + (b[i] - a[i]) * u);
  return `rgb(${m(0)},${m(1)},${m(2)})`;
}

// ::BADGE — a coloured status pill; colour inferred from common status words.
function badgeHtml(text) {
  const t = text.toLowerCase();
  let cls = "badge-neutral";
  if (/\b(ok|good|on.?track|pass(ed)?|active|done|up|healthy|nominal|green|low)\b/.test(t)) cls = "badge-good";
  else if (/\b(warn(ing)?|risk|at.?risk|pending|review|amber|medium|hold|watch)\b/.test(t)) cls = "badge-warn";
  else if (/\b(bad|fail(ed)?|late|error|down|critical|red|overdue|stale|high|breach)\b/.test(t)) cls = "badge-bad";
  return `<span class="badge ${cls}">${escapeHtml(text)}</span>`;
}

// ::SPARKLINE cell — a tiny inline trend line from a numeric array (DuckDB list()).
function cellSpark(v) {
  let arr = Array.isArray(v) ? v : null;
  if (!arr && typeof v === "string") {
    try {
      const p = JSON.parse(v);
      if (Array.isArray(p)) arr = p;
    } catch (_) {}
  }
  const nums = (arr || []).map(Number).filter((x) => isFinite(x));
  if (nums.length < 2) return "";
  const w = 84;
  const h = 22;
  const pad = 2;
  const mn = Math.min(...nums);
  const rng = Math.max(...nums) - mn || 1;
  const xs = (i) => pad + (i * (w - 2 * pad)) / (nums.length - 1);
  const ys = (y) => h - pad - ((y - mn) / rng) * (h - 2 * pad);
  const pts = nums.map((y, i) => `${xs(i).toFixed(1)},${ys(y).toFixed(1)}`).join(" ");
  const lx = xs(nums.length - 1).toFixed(1);
  const ly = ys(nums[nums.length - 1]).toFixed(1);
  return (
    `<svg class="spark" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
    `<polyline points="${pts}" fill="none" stroke="#456481" stroke-width="0.9" stroke-linejoin="round" stroke-linecap="round"/>` +
    `<circle cx="${lx}" cy="${ly}" r="1.8" fill="#E8335D"/></svg>`
  );
}

// Format a KPI value. fmt: METRIC (plain), MONEY, PERCENT, COMPACT.
function fmtNum(v, fmt) {
  const n = typeof v === "number" ? v : parseFloat(v);
  if (v == null) return "–";
  if (Number.isNaN(n)) return String(v);
  if (fmt === "MONEY")
    return n.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 0 });
  if (fmt === "PERCENT") return n.toLocaleString(undefined, { maximumFractionDigits: 1 }) + "%";
  if (fmt === "COMPACT")
    return new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(n);
  return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function escapeHtml(s) {
  return String(s ?? "").replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
}

// Minimal, dependency-free Markdown → HTML (headings, bold/italic, inline +
// fenced code, links, blockquotes, ordered/unordered lists, rules, paragraphs).
// Raw HTML in the source is escaped, so it's safe to inject.
function renderMarkdown(src) {
  const esc = escapeHtml;
  const inline = (s) =>
    esc(s)
      .replace(/`([^`]+)`/g, (m, c) => `<code>${c}</code>`)
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/__([^_]+)__/g, "<strong>$1</strong>")
      .replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>")
      .replace(/\[([^\]]+)\]\(([^)]+)\)/g, (m, t, u) => `<a href="${esc(u)}" target="_blank" rel="noopener">${t}</a>`);
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
  const t = s.roles.find((r) => ["TEXT_SMALL", "TEXT_MEDIUM", "TEXT_LARGE"].includes(r[1]));
  return t ? t[1].split("_")[1].toLowerCase() : null;
}

const unq = (v) => String(v ?? "").replace(/^"|"$/g, "");

// CSV / .xls (HTML-table Excel) directly from JSON result rows.
function csvOfRows(rows, cols) {
  const esc = (v) => `"${unq(v).replace(/"/g, '""')}"`;
  return [cols.map(esc).join(","), ...rows.map((r) => cols.map((c) => esc(r[c])).join(","))].join("\n");
}
function xlsOfRows(rows, cols) {
  const cell = (v) => escapeHtml(unq(v));
  const head = "<tr>" + cols.map((c) => `<th>${cell(c)}</th>`).join("") + "</tr>";
  const body = rows.map((r) => "<tr>" + cols.map((c) => `<td>${cell(r[c])}</td>`).join("") + "</tr>").join("");
  return `<html><head><meta charset="utf-8"></head><body><table>${head}${body}</table></body></html>`;
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
    if (fmt === "xlsx") download(new Blob([xlsOfRows(rows, cols)], { type: "application/vnd.ms-excel" }), "data.xls");
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
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob((b) => download(b, name + ".png"));
  };
  img.src = "data:image/svg+xml;base64," + btoa(unescape(encodeURIComponent(xml)));
}

// A hover download button on every chart (PNG) / table (CSV) panel.
function addExportButtons() {
  document.querySelectorAll(".panel").forEach((fig) => {
    if (fig.querySelector(".dl")) return;
    const svg = fig.querySelector("svg");
    const table = fig.querySelector(".dp-table");
    if (!svg && !table) return;
    fig.style.position = "relative";
    const btn = document.createElement("button");
    btn.className = "dl";
    btn.textContent = "⤓";
    btn.title = table ? "download CSV" : "download PNG";
    btn.onclick = (e) => {
      e.stopPropagation();
      if (table) download(new Blob([csvOf(table)], { type: "text/csv" }), "data.csv");
      else svgToPng(svg, "chart");
    };
    fig.appendChild(btn);
    // A full-size button on charts — opens the plot large in an overlay.
    if (svg) {
      const exp = document.createElement("button");
      exp.className = "dl dp-expand";
      exp.textContent = "⤢";
      exp.title = "full size";
      exp.onclick = (e) => {
        e.stopPropagation();
        openFullSize(fig);
      };
      fig.appendChild(exp);
    }
  });
}

// Open a chart panel large in a centred overlay. The SVG is vector, so we clone
// it and let it scale to fill (Esc / backdrop / ✕ closes).
function openFullSize(fig) {
  const svg = fig.querySelector("svg");
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
  const url = editable
    ? base + "#sql=" + encoded
    : base + "?embed=1#sql=" + encoded;
  navigator.clipboard.writeText(url).then(
    () => status(editable ? "editable link copied ✓" : "view-only link copied ✓"),
    () => {
      location.hash = "sql=" + encoded;
      status("copy the link from the address bar");
    }
  );
}

// Download the current dashboard as a standalone, self-contained HTML file.
function downloadHtml() {
  const style = document.querySelector("style").textContent;
  const content = document.querySelector(".dash").innerHTML;
  const html =
    `<!doctype html><html><head><meta charset="utf-8"><title>anofox-visualization dashboard</title>` +
    `<style>${style}</style></head><body style="background:#f4f6f9;padding:1.5rem">` +
    `<div class="dash">${content}</div><div id="dp-tip" class="dp-tip"></div>` +
    `<script>${SNAPSHOT_JS}<\/script></body></html>`;
  download(new Blob([html], { type: "text/html" }), "dashboard.html");
}

// Self-contained hover + tab switching for the exported snapshot.
const SNAPSHOT_JS = `(function(){
  var tip=document.getElementById('dp-tip');
  document.querySelectorAll('.dp-hit').forEach(function(el){
    var t=el.getAttribute('data-tip'); if(!t)return;
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
  const W = 460,
    aspect = W / ph;
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
      holder.innerHTML = render_panel(rowsJson, JSON.stringify(roles), W, ph, dpPrimary || "", zoom);
      attachHover(); // re-wire hover + cross-filter clicks on the fresh marks
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

// Styled hover tooltips + click-to-highlight LINKING across all panels.
// Every mark carrying a `<title>` ("series: value") becomes hoverable; its series
// (the part before ": ") is stored on the element. Clicking a mark highlights
// that series everywhere and dims the rest; click again (or the background) clears.
let dpSelected = null;

function attachHover() {
  const tip = $("dp-tip");
  const marks = [
    ...document.querySelectorAll(".panel svg rect,.panel svg circle,.panel svg polygon,.panel svg polyline"),
  ].filter((el) => el.querySelector("title") && el.querySelector("title").textContent.trim());

  const apply = () => {
    if (!dpSelected) {
      for (const el of marks) el.style.opacity = "";
      return;
    }
    // Dim non-selected marks, but only inside a panel that actually contains the
    // selected series. A panel keyed by a different dimension (e.g. a line
    // filtered to one channel, keyed by week) has no matching mark, so it stays
    // fully visible instead of every point dimming away.
    const byPanel = new Map();
    for (const el of marks) {
      const fig = el.closest(".panel") || document.body;
      (byPanel.get(fig) || byPanel.set(fig, []).get(fig)).push(el);
    }
    for (const els of byPanel.values()) {
      const hasSel = els.some((el) => el.getAttribute("data-series") === dpSelected);
      for (const el of els) {
        el.style.opacity = !hasSel || el.getAttribute("data-series") === dpSelected ? "" : "0.15";
      }
    }
  };

  marks.forEach((el) => {
    const t = el.querySelector("title");
    const txt = t.textContent;
    const series = txt.includes(": ") ? txt.slice(0, txt.lastIndexOf(": ")) : txt;
    el.removeChild(t);
    el.setAttribute("data-series", series);
    el.setAttribute("data-tip", txt);
    el.classList.add("dp-hit");
    el.style.cursor = "pointer";
    el.addEventListener("mouseenter", () => {
      if (el.closest(".has-axis-pointer")) return; // the panel-level crosshair shows the tooltip
      const dx = el.getAttribute("data-x") || "";
      const i = txt.lastIndexOf(": ");
      const label = i >= 0 ? txt.slice(0, i) : txt;
      const val = i >= 0 ? txt.slice(i + 2) : "";
      const fill = el.getAttribute("fill") || getComputedStyle(el).fill || "#619cff";
      tip.innerHTML =
        (dx ? `<div class="tip-head">${escapeHtml(dx)}</div>` : "") +
        `<div class="tip-row"><span><span class="tip-dot" style="background:${fill}"></span>${escapeHtml(label)}</span><b>${escapeHtml(val)}</b></div>`;
      tip.classList.add("show");
    });
    el.addEventListener("mousemove", (e) => {
      if (el.closest(".has-axis-pointer")) return;
      tip.style.left = e.clientX + 14 + "px";
      tip.style.top = e.clientY + 14 + "px";
    });
    el.addEventListener("mouseleave", () => {
      if (el.closest(".has-axis-pointer")) return;
      tip.classList.remove("show");
    });
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      // Cross-filter: toggle `selected` to this value and re-query. Queries that
      // opt in (getvariable('selected')) filter; the rest just highlight it.
      dpFilter = dpFilter === series ? "" : series;
      dpSelected = dpFilter || null;
      run(false);
    });
  });
  apply();
  attachAxisPointer();
  attachLegendToggle();
  attachToolbox();
}

// ECharts-style toolbox: a hover-reveal toolbar per chart panel — chart-type
// toggle (line↔bar), data view, restore (reset zoom), and save-as-PNG.
function attachToolbox() {
  document.querySelectorAll(".panel").forEach((panel) => {
    if (panel.dataset.toolboxWired) return;
    const svg = panel.querySelector("svg");
    if (!svg || !svg.viewBox || !svg.viewBox.baseVal || !svg.viewBox.baseVal.width) return;
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
        fn();
      });
      bar.appendChild(b);
      return b;
    };
    const dp = panel._dp;
    // magicType: swap a line chart to bars and back (only for line/bar charts).
    if (dp && !dp.isMap && magicSwap(dp.roles)) {
      mkTool("Line / bar", "⇄", () => toggleMagicType(panel));
    }
    // dataView: show the panel's underlying rows as a table.
    if (dp) mkTool("Data view", "▤", () => showDataView(panel));
    // brush + value filter: only for charts with real marks (points or bars).
    if (dp && !dp.isMap && svg.querySelector(".dp-hit")) {
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
    mkTool("Save as PNG", "⭳", () => savePanelPng(panel));
    panel.appendChild(bar);
  });
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
  holder.innerHTML = render_panel(dp.rows, JSON.stringify(use), 460, dp.ph, dpPrimary || "", "");
  attachHover();
}

// A modal listing the panel's rows as a table (ECharts toolbox "data view").
function showDataView(panel) {
  const dp = panel._dp;
  if (!dp) return;
  let rows;
  try {
    rows = JSON.parse(dp.rows);
  } catch (_) {
    return;
  }
  const roleFor = (k) => dp.roles.find((x) => x[0] === +String(k).replace(/^c/, ""));
  // Drop non-data columns (the panel title, tooltips, hints) from the view.
  const SKIP = new Set(["TITLE", "HINT", "LABEL"]);
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
  const head = keys.map((k) => `<th>${esc(nameFor(k))}</th>`).join("");
  const body = rows
    .slice(0, 500)
    .map((r) => `<tr>${keys.map((k) => `<td>${esc(r[k])}</td>`).join("")}</tr>`)
    .join("");
  const title = panel.querySelector(".panel-title")?.textContent?.trim() || "Data";
  const back = document.createElement("div");
  back.className = "dp-modal-back";
  back.innerHTML =
    `<div class="dp-modal"><div class="dp-modal-head"><b>${esc(title)}</b>` +
    `<button class="dp-modal-x" title="Close">✕</button></div>` +
    `<div class="dp-modal-body"><table class="dp-dv"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>` +
    (rows.length > 500 ? `<div class="dp-modal-note">Showing 500 of ${rows.length} rows</div>` : "") +
    `</div></div>`;
  const close = () => back.remove();
  back.addEventListener("click", (e) => {
    if (e.target === back || e.target.classList.contains("dp-modal-x")) close();
  });
  document.body.appendChild(back);
}

// Parse the measure out of a mark's tooltip — the last number in "label: 22",
// "web: 22", or "(3, 22)" (else null).
function markValue(el) {
  const m = (el.getAttribute("data-tip") || "").match(/-?\d[\d,]*\.?\d*(?:[eE][+-]?\d+)?/g);
  if (!m) return null;
  const n = parseFloat(m[m.length - 1].replace(/,/g, ""));
  return isNaN(n) ? null : n;
}

// Every drawn data mark (scatter/line points AND bars carry class "dp-hit").
function marksOf(panel) {
  return panel.querySelectorAll("svg .dp-hit");
}

// Dim every data mark that fails `keep(el)`; pass null to clear the emphasis.
function filterMarks(panel, keep) {
  marksOf(panel).forEach((el) => {
    el.style.opacity = keep && !keep(el) ? "0.12" : "";
  });
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
      y = Math.min(start.y, p.y),
      w = Math.abs(p.x - start.x),
      h = Math.abs(p.y - start.y);
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
    filterMarks(panel, (el) => {
      const b = el.getBoundingClientRect();
      const cx = b.left + b.width / 2 - ovr.left,
        cy = b.top + b.height / 2 - ovr.top;
      return cx >= box.x0 && cx <= box.x1 && cy >= box.y0 && cy <= box.y1;
    });
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
  const svg = panel.querySelector("svg");
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
    ctx.fillStyle = document.body.classList.contains("dark") ? "#0f1729" : "#ffffff";
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
        if (e.swatch && e.fill) e.swatch.setAttribute("fill", off ? "#c4c9d2" : e.fill); // grey when off
      });
    };
    // ECharts-style focus: hovering a legend entry emphasises that series and
    // fades the others (visible ones only; hidden/toggled-off series stay off).
    const emphasize = (only) => {
      svg.querySelectorAll("[data-series]").forEach((el) => {
        const s = el.getAttribute("data-series");
        if (hidden.has(s)) return;
        el.style.opacity = only && s !== only ? "0.15" : "";
      });
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
    // Radar is polar and jitter/scatter has no shared-x series — a vertical
    // x-crosshair is wrong for both; their per-point (<title>) hover takes over.
    if (roles.some((r) => /^(RADAR|SPIDER|CANDLESTICK)$/.test(r[1]))) return;
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
        tip: el.getAttribute("data-tip") || "",
        dx: el.getAttribute("data-x") || "",
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
      cross.style.height = r.height + "px";
      cross.style.display = "";
      pts.forEach((p) => (p.el.style.transform = ""));
      colPts.forEach((p) => {
        p.el.style.transformBox = "fill-box";
        p.el.style.transformOrigin = "center";
        p.el.style.transform = "scale(1.7)";
      });
      const head = colPts[0] && colPts[0].dx ? `<div class="tip-head">${escapeHtml(colPts[0].dx)}</div>` : "";
      tip.innerHTML =
        head +
        colPts
          .map((p) => {
            const i = p.tip.lastIndexOf(": ");
            const label = i >= 0 ? p.tip.slice(0, i) : p.tip;
            const val = i >= 0 ? p.tip.slice(i + 2) : "";
            return `<div class="tip-row"><span><span class="tip-dot" style="background:${p.fill}"></span>${escapeHtml(label)}</span><b>${escapeHtml(val)}</b></div>`;
          })
          .join("");
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

// Scroll-to-zoom / drag-to-pan for a continuous cartesian chart (double-click
// resets). Uses the SVG's data-plot rect (panel area in viewBox units) to map
// the cursor accurately to data coords, and re-renders with a zoom window.
function attachCartZoom(holder, rowsJson, roles, ph) {
  let b;
  try {
    b = JSON.parse(panel_bounds(rowsJson, JSON.stringify(roles)));
  } catch (_) {
    b = [];
  }
  if (b.length !== 4) return; // not a continuous-x chart → no zoom
  const W = 460;
  const full = { x0: b[0], x1: b[1], y0: b[2], y1: b[3] };
  let view = null; // null = auto (full extent)
  let raf = 0;
  let syncSlider = () => {}; // set up below once the slider DOM exists
  const draw = () => {
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      const zoom = view ? JSON.stringify([view.x0, view.x1, view.y0, view.y1]) : "";
      holder.innerHTML = render_panel(rowsJson, JSON.stringify(roles), W, ph, dpPrimary || "", zoom);
      attachHover();
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
  const anyNamed = Object.values(dpXf).some((v) => v);
  if (dpFilter || dpSelected !== null || anyNamed) {
    dpFilter = "";
    dpSelected = null;
    for (const k in dpXf) dpXf[k] = "";
    run(false);
  }
});

boot().catch((e) => status("boot failed: " + e));
