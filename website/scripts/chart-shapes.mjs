// #1022: the data shape of each chart kind — the key section of its page (for
// a SQL-first tool the question is "which rows and columns?") — and a minimal
// panel on inline data that `node scripts/gen-charts.mjs --check` renders with
// the dashboard CLI. Keyed by the canonical role; `with` folds a companion
// role's page into this one (::LON into ::LAT).
const v = (rows, cols) => `(VALUES ${rows}) t(${cols})`;
const months = "range(DATE '2026-01-01', DATE '2026-07-01', INTERVAL 1 MONTH) r(m)";

export const shapes = {
  BARCHART: {
    shape: "One row per bar: `::XAXIS` names the bar, the measure is its height. Add `::CATEGORY` for bars side by side within each x.",
    sql: `SELECT region ::XAXIS, channel ::CATEGORY, revenue ::BARCHART
FROM ${v("('North','Web',120),('North','Store',80),('South','Web',90),('South','Store',110)", "region, channel, revenue")}`,
  },
  BARCHART_STACKED: {
    shape: "One row per segment: `::XAXIS` and `::CATEGORY`; the segments of an x stack to its total.",
    sql: `SELECT region ::XAXIS, channel ::CATEGORY, revenue ::BARCHART_STACKED
FROM ${v("('North','Web',120),('North','Store',80),('South','Web',90),('South','Store',110)", "region, channel, revenue")}`,
  },
  BARCHART_PERCENT: {
    shape: "As `::BARCHART`; each bar is drawn as its share of the x's total (values ≤ 1 are taken as fractions already).",
    sql: `SELECT region ::XAXIS, channel ::CATEGORY, revenue ::BARCHART_PERCENT
FROM ${v("('North','Web',120),('North','Store',80),('South','Web',90),('South','Store',110)", "region, channel, revenue")}`,
  },
  BARCHART_STACKED_PERCENT: {
    shape: "As `::BARCHART_STACKED`; every bar is scaled to 100 %, so the segments read as shares.",
    sql: `SELECT region ::XAXIS, channel ::CATEGORY, revenue ::BARCHART_STACKED_PERCENT
FROM ${v("('North','Web',120),('North','Store',80),('South','Web',90),('South','Store',110)", "region, channel, revenue")}`,
  },
  LINECHART: {
    shape: "One row per point: `::XAXIS` (usually a date), the measure as y; `::CATEGORY` draws one line per value. Rows need not be sorted.",
    sql: `SELECT m ::XAXIS, s ::CATEGORY, 100 + 10 * month(m) * (s = 'B')::INT + 5 * month(m) ::LINECHART
FROM ${months}, (VALUES ('A'), ('B')) c(s)`,
  },
  LINECHART_PERCENT: {
    shape: "As `::LINECHART`, with the y axis in percent (a value of 0.25 reads 25 %).",
    sql: `SELECT m ::XAXIS, 0.1 + 0.05 * month(m) ::LINECHART_PERCENT FROM ${months}`,
  },
  STEP: {
    shape: "As `::LINECHART`, drawn as steps — for values that hold until they change (prices, stock levels, survival curves; `::CENSOR` marks censored times).",
    sql: `SELECT m ::XAXIS, [5, 5, 8, 8, 6, 9][month(m)] ::STEP FROM ${months}`,
  },
  SMOOTH: {
    shape: "One row per observation: `::XAXIS` and the measure, both numeric. Draws the points and a LOESS trend (`'lm'::SMOOTH_METHOD` for a straight line).",
    sql: `SELECT x ::XAXIS, x * 0.5 + sin(x) * 3 ::SMOOTH FROM range(0, 40) r(x)`,
  },
  AREACHART: {
    shape: "As `::LINECHART`, filled down to zero.",
    sql: `SELECT m ::XAXIS, 50 + 8 * month(m) ::AREACHART FROM ${months}`,
  },
  AREACHART_STACKED: {
    shape: "One row per x and `::CATEGORY`; the areas stack to the total.",
    sql: `SELECT m ::XAXIS, s ::CATEGORY, (CASE s WHEN 'Web' THEN 40 ELSE 25 END) + 4 * month(m) ::AREACHART_STACKED
FROM ${months}, (VALUES ('Web'), ('Store')) c(s)`,
  },
  SCATTER: {
    shape: "One row per point: a numeric `::XAXIS` and the measure as y. `::CATEGORY` colours, `::SIZE` makes it a bubble chart, `::LABEL` names points.",
    sql: `SELECT price ::XAXIS, units ::SCATTER, segment ::CATEGORY
FROM ${v("(9.5,120,'A'),(12,95,'A'),(15,70,'B'),(18,64,'B'),(22,40,'B'),(11,100,'A')", "price, units, segment")}`,
  },
  BUBBLE: {
    shape: "As `::SCATTER`, with a `::SIZE` column for each point's area.",
    sql: `SELECT price ::XAXIS, units ::BUBBLE, revenue ::SIZE
FROM ${v("(9.5,120,1140),(12,95,1140),(15,70,1050),(18,64,1152),(22,40,880)", "price, units, revenue")}`,
  },
  JITTER: {
    shape: "Raw rows: `::XAXIS` groups, the measure as y; points are spread sideways so overlapping values show.",
    sql: `SELECT ['Mon','Tue','Wed'][1 + i % 3] ::XAXIS, (i * 37) % 20 ::JITTER FROM range(0, 60) r(i)`,
  },
  PIE: {
    shape: "One row per slice: `::CATEGORY` names it, the measure sizes it. Keep it to a handful of slices.",
    sql: `SELECT channel ::CATEGORY, revenue ::PIE FROM ${v("('Web',120),('Store',80),('Partner',40)", "channel, revenue")}`,
  },
  DONUTCHART: {
    shape: "As `::PIE`, with a hole.",
    sql: `SELECT channel ::CATEGORY, revenue ::DONUTCHART FROM ${v("('Web',120),('Store',80),('Partner',40)", "channel, revenue")}`,
  },
  GAUGE: {
    shape: "One row, one value; `::RANGE 'min,max'` sets the arc (default 0–100).",
    sql: `SELECT 72 ::GAUGE, '0,100' ::RANGE, 'Utilisation' ::TITLE`,
  },
  RADAR: {
    shape: "One row per axis and `::CATEGORY`: `::XAXIS` names the axis, each category is one polygon.",
    sql: `SELECT k ::XAXIS, p ::CATEGORY, s ::RADAR
FROM ${v("('Speed','A',4),('Price','A',3),('Range','A',5),('Comfort','A',2),('Speed','B',3),('Price','B',5),('Range','B',2),('Comfort','B',4)", "k, p, s")}`,
  },
  WATERFALL: {
    shape: "One row per step in order: `::XAXIS` names it, the measure is the signed change; rows with a NULL measure or `::CATEGORY 'total'` show the running total.",
    sql: `SELECT step ::XAXIS, delta ::WATERFALL, kind ::CATEGORY
FROM ${v("(1,'Start',100,'total'),(2,'Price',20,NULL),(3,'Volume',-35,NULL),(4,'Mix',10,NULL),(5,'End',NULL,'total')", "o, step, delta, kind")} ORDER BY o`,
  },
  BULLET: {
    shape: "One row per item: `::XAXIS` names it, the measure is the bar, `::GOAL` the tick, `::RANGE '60,80,100'` the bands.",
    sql: `SELECT team ::XAXIS, score ::BULLET, 85 ::GOAL, '60,80,100' ::RANGE
FROM ${v("('Support',78),('Sales',91),('Ops',64)", "team, score")}`,
  },
  FUNNEL: {
    shape: "One row per stage in order: `::XAXIS` names it, the measure is the count; each stage shows its conversion from the one before.",
    sql: `SELECT stage ::XAXIS, n ::FUNNEL
FROM ${v("(1,'Visit',1000),(2,'Sign-up',420),(3,'Trial',180),(4,'Paid',60)", "o, stage, n")} ORDER BY o`,
  },
  SANKEY: {
    shape: "One row per link: `::ORIGIN` → `::DEST`, the measure is its width. Nodes that are both a destination and an origin chain into columns.",
    sql: `SELECT a ::ORIGIN, b ::DEST, n ::SANKEY
FROM ${v("('Ads','Visit',600),('Search','Visit',400),('Visit','Trial',300),('Visit','Gone',700),('Trial','Paid',120)", "a, b, n")}`,
  },
  TREEMAP: {
    shape: "One row per tile: `::XAXIS` is its key, the measure its area; `::CATEGORY` nests tiles in groups, `::LABEL` shows a name instead of the key.",
    sql: `SELECT product ::XAXIS, family ::CATEGORY, revenue ::TREEMAP
FROM ${v("('Tea','Drinks',40),('Coffee','Drinks',70),('Juice','Drinks',25),('Bread','Bakery',50),('Cake','Bakery',30)", "product, family, revenue")}`,
  },
  PARETO: {
    shape: "One row per category: `::XAXIS` and the measure. The bars are sorted largest first, with the cumulative share as a line.",
    sql: `SELECT cause ::XAXIS, n ::PARETO
FROM ${v("('Late',42),('Damaged',18),('Wrong item',11),('Lost',6),('Other',3)", "cause, n")}`,
  },
  GANTT: {
    shape: "One row per task: this column names it, `::XMIN` and `::XMAX` are its start and end (dates or numbers); `::CATEGORY` colours.",
    sql: `SELECT task ::GANTT, s ::XMIN, e ::XMAX, owner ::CATEGORY
FROM ${v("('Design',DATE '2026-01-05',DATE '2026-01-20','A'),('Build',DATE '2026-01-15',DATE '2026-02-20','B'),('Test',DATE '2026-02-10',DATE '2026-03-01','A')", "task, s, e, owner")}`,
  },
  DUMBBELL: {
    shape: "One row per item: this column names it, `::XMIN` and `::XMAX` are the two values (before/after, plan/actual).",
    sql: `SELECT store ::DUMBBELL, y2025 ::XMIN, y2026 ::XMAX
FROM ${v("('Berlin',80,95),('Hamburg',70,66),('Munich',60,88)", "store, y2025, y2026")}`,
  },
  HISTOGRAM: {
    shape: "Raw rows: one per observation, the measure only. The bins are computed for you.",
    sql: `SELECT (i * 7919) % 100 / 10.0 + (i % 7) ::HISTOGRAM FROM range(0, 300) r(i)`,
  },
  DENSITY: {
    shape: "Raw rows: one per observation; `::CATEGORY` draws one curve per group.",
    sql: `SELECT g ::CATEGORY, (i * 7919) % 100 / 10.0 + (g = 'B')::INT * 3 ::DENSITY
FROM range(0, 200) r(i), (VALUES ('A'), ('B')) c(g)`,
  },
  RIDGELINE: {
    shape: "Raw rows: one per observation, `::CATEGORY` per ridge (the first category on top).",
    sql: `SELECT month ::CATEGORY, (i * 7919) % 100 / 10.0 + mo ::RIDGELINE
FROM range(0, 150) r(i), ${v("(1,'Jan'),(2,'Feb'),(3,'Mar'),(4,'Apr')", "mo, month")}`,
  },
  BOXPLOT: {
    shape: "Raw rows: `::XAXIS` groups, the measure as y; the quartiles are computed for you. Already have the statistics? Use `::Q1`/`::MEDIAN`/`::Q3` instead.",
    sql: `SELECT ['A','B','C'][1 + i % 3] ::XAXIS, (i * 7919) % 50 + (i % 3) * 10 ::BOXPLOT FROM range(0, 150) r(i)`,
  },
  VIOLIN: {
    shape: "As `::BOXPLOT`: raw rows, `::XAXIS` groups, the measure as y.",
    sql: `SELECT ['A','B','C'][1 + i % 3] ::XAXIS, (i * 7919) % 50 + (i % 3) * 10 ::VIOLIN FROM range(0, 150) r(i)`,
  },
  QQ: {
    shape: "Raw rows: one per observation (residuals, say); plotted against normal quantiles with a 95 % band.",
    sql: `SELECT (i * 7919) % 100 / 25.0 - 2 ::QQ FROM range(0, 100) r(i)`,
  },
  HEATMAP: {
    shape: "One row per tile: `::XAXIS` × `::YAXIS`, the measure is its colour (a text measure with `::COLORS` maps status words to colours).",
    sql: `SELECT d ::XAXIS, h ::YAXIS, (d * 3 + h * 5) % 11 ::HEATMAP
FROM range(1, 8) a(d), range(8, 18) b(h)`,
  },
  CALENDAR: {
    shape: "One row per day: a date `::XAXIS`, the measure is the day's colour.",
    sql: `SELECT d::DATE ::XAXIS, (dayofyear(d) * 37) % 10 ::CALENDAR
FROM range(DATE '2026-01-01', DATE '2026-07-01', INTERVAL 1 DAY) r(d)`,
  },
  CANDLESTICK: {
    shape: "One row per period: `::XAXIS`, `::OPEN`, `::HIGH`, `::LOW`; the measure is the close.",
    sql: `SELECT d ::XAXIS, o ::OPEN, h ::HIGH, l ::LOW, c ::CANDLESTICK
FROM ${v("(DATE '2026-03-02',10,12,9,11),(DATE '2026-03-03',11,13,10,12.5),(DATE '2026-03-04',12.5,13,11,11.2),(DATE '2026-03-05',11.2,12,10.5,11.8)", "d, o, h, l, c")}`,
  },
  SPARKLINE: {
    shape: "In a table: one row per item with a `list(v ORDER BY t)` column (a KPI tile takes the same list under its `::METRIC`). On a map it becomes a mini chart in each feature's tooltip.",
    sql: `SELECT store ::TABLE, trend AS "Last 4 weeks" ::SPARKLINE
FROM (SELECT store, list(v ORDER BY t) AS trend
      FROM ${v("('Berlin',1,5),('Berlin',2,7),('Berlin',3,6),('Berlin',4,9),('Hamburg',1,4),('Hamburg',2,3),('Hamburg',3,5),('Hamburg',4,4)", "store, t, v")} GROUP BY store)`,
  },

  SPARKBAR: {
    shape: "As `::SPARKLINE`, drawn as bars; a list of `{'ref': r, 'cur': c}` overlays two series on one scale.",
    sql: `SELECT store ::TABLE, trend AS "Last 4 weeks" ::SPARKBAR
FROM (SELECT store, list(v ORDER BY t) AS trend
      FROM ${v("('Berlin',1,5),('Berlin',2,7),('Berlin',3,6),('Berlin',4,9),('Hamburg',1,4),('Hamburg',2,3),('Hamburg',3,5),('Hamburg',4,4)", "store, t, v")} GROUP BY store)`,
  },

  MAP: {
    shape: "One row per feature: its geometry as WKT text (`ST_AsText(geom)`), a measure to colour it (any chart cast on a numeric column), `::LABEL` to name it.",
    sql: `SELECT wkt ::MAP, name ::LABEL, v AS "Sales" ::BARCHART
FROM ${v("('POLYGON((0 0, 4 0, 4 3, 0 3, 0 0))','West',10),('POLYGON((4 0, 8 0, 8 3, 4 3, 4 0))','East',25)", "wkt, name, v")}`,
  },
  REGION: {
    shape: "One row per region: a code (`'DE'`, `'DE-BY'`, an AGS `'09162'`, NUTS `'DE212'`) — the boundary is built in — and a measure to colour it.",
    sql: `SELECT code ::REGION, v AS "Sales" ::BARCHART
FROM ${v("('DE-BY',120),('DE-BW',100),('DE-NW',140),('DE-NI',70),('DE-HE',80),('DE-BE',60)", "code, v")}`,
  },
  LAT: {
    with: "LON",
    title: "::LAT + ::LON",
    shape: "One row per point: `::LAT` and `::LON` in degrees (WGS84); a measure colours or sizes (`::SIZE`) it, `::LABEL` names it, `::BASEMAP` draws a backdrop.",
    sql: `SELECT lat ::LAT, lon ::LON, city ::LABEL, orders ::BARCHART, 'de-states' ::BASEMAP
FROM ${v("(52.52,13.40,'Berlin',120),(53.55,9.99,'Hamburg',80),(48.14,11.58,'Munich',95),(50.94,6.96,'Cologne',60)", "lat, lon, city, orders")}`,
  },
  ORIGIN: {
    with: "DEST",
    title: "::ORIGIN + ::DEST",
    shape: "One row per flow: `::ORIGIN` → `::DEST` as region codes, WKT points or `{'lon', 'lat', 'name'}` structs; `::SIZE` sets the width. In a `::SANKEY`, the same pair names the links.",
    sql: `SELECT a ::ORIGIN, b ::DEST, n ::SIZE, 'de-states' ::BASEMAP
FROM ${v("('DE-HH','DE-BY',40),('DE-HH','DE-BE',25),('DE-NW','DE-BY',30)", "a, b, n")}`,
  },
  BASEMAP: {
    shape: "A constant on a map panel: a built-in layer (`'world'`, `'de-states'`, `'de-districts'`) or your own WKT, drawn under the features.",
    sql: `SELECT 52.52 ::LAT, 13.40 ::LON, 'Berlin' ::LABEL, 'de-states' ::BASEMAP`,
  },
  DIAGRAM: {
    shape: "One row per node: this column is its id, `::LABEL` its text, `::UPSTREAM` its input id(s) — or all edges at once with `::EDGES`; `::LANE` groups nodes in swimlanes.",
    sql: `SELECT id ::DIAGRAM, label ::LABEL, up ::UPSTREAM
FROM ${v("('src','Orders CSV',NULL),('clean','Clean','src'),('agg','Daily totals','clean'),('dash','Dashboard','agg')", "id, label, up")}`,
  },
};
