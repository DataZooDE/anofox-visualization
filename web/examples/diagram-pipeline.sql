-- @title How does data move through our pipeline?
-- @group Data quality
-- @tags  diagram, pipeline, monitoring
-- @description Last night's run of the inventory pipeline, stage by stage: what landed, what was rejected, what ran late.
--
-- How does data move through our pipeline? The nightly run of an inventory
-- digital-twin pipeline (the stages of inventory-rs): three sources land,
-- are ingested and validated, build the twin, project stock, monitor drift
-- (calibrating the twin in a loop) and recommend the orders the stores need
-- by 08:00.
--
-- The question → answer flow:
--   1. pick a run: Wednesday's incident or a normal Tuesday (::BUTTONS); the
--      diagram keeps its layout and only what moves changes ('tween'::TRANSITION):
--      edge widths follow the records they carry, colours their health;
--   2. the KPI strip answers "did the stores get their orders on time?";
--   3. the diagram shows what moves where: stages appear one after another,
--      records travel along the edges (flow), and on Wednesday Step / Play
--      walk through the incident (::STORY), each step lighting a stage and
--      every connection touching it. The arrow keys follow the connections;
--   4. click a stage ('stage'::SELECT_AS) for its details: rows in / out, run
--      time against its usual time (with 30 runs as a trend), error rate,
--      records per 5 minutes against its deadline and the incident window,
--      and what went wrong — each error links to the stage that caused it.
--
-- Synthetic, deterministic data generated below (30 nightly runs, the
-- incident on Wednesday 14 October 2026).

-- The stages: kind, usual run time (minutes), deadline, and where they sit.
CREATE OR REPLACE TABLE dg_stage AS
SELECT * FROM (VALUES
  ( 1, 'pos',        'POS sales',        'source',  6, TIME '05:30', 1200000),
  ( 2, 'stock',      'Stock feed',       'source',  6, TIME '05:30',  480000),
  ( 3, 'supplier',   'Supplier orders',  'source',  4, TIME '05:30',   36000),
  ( 4, 'ingest',     'Ingest',           'step',   12, TIME '06:00', 1716000),
  ( 5, 'validate',   'Validate',         'step',    9, TIME '06:15', 1714300),
  ( 6, 'quarantine', 'Quarantine',       'sink',    1, TIME '06:15',    1700),
  ( 7, 'build',      'Build twin',       'step',   22, TIME '06:45',    9800),
  ( 8, 'project',    'Project stock',    'step',   18, TIME '07:15',    9800),
  ( 9, 'monitor',    'Monitor drift',    'step',    6, TIME '07:30',     412),
  (10, 'calibrate',  'Calibrate',        'step',    8, TIME '07:40',      29),
  (11, 'recommend',  'Recommend orders', 'step',   14, TIME '07:45',    1240),
  (12, 'stores',     'Store systems',    'sink',    2, TIME '08:00',    1240)
) t(seq, stage, title, kind, usual_min, due, usual_rows);

CREATE OR REPLACE TABLE dg_flow AS
SELECT * FROM (VALUES
  ('pos', 'ingest'), ('stock', 'ingest'), ('supplier', 'ingest'), ('ingest', 'validate'),
  ('validate', 'quarantine'), ('validate', 'build'), ('build', 'project'),
  ('project', 'monitor'), ('monitor', 'calibrate'), ('calibrate', 'build'),
  ('project', 'recommend'), ('recommend', 'stores')
) t(src, dst);

-- 30 nightly runs per stage. Normal nights vary a little (a deterministic
-- hash); on Wednesday 14 October the stock feed arrives with its columns in a
-- new order: Validate rejects 38 % of the stock rows, Build twin waits for
-- the re-sent feed until the parser is rolled back at 08:35, and the orders
-- reach the stores at 09:05.
CREATE OR REPLACE TABLE dg_run AS
WITH d AS (SELECT (DATE '2026-09-16' + INTERVAL (i) DAY)::DATE AS day FROM range(30) t(i)),
r AS (
  SELECT d.day, s.*, d.day = DATE '2026-10-14' AS incident,
         1 + ((hash(d.day::VARCHAR || s.stage) % 1000) / 1000.0 - 0.5) * 0.12 AS jitter
  FROM d, dg_stage s
)
SELECT day, seq, stage, title, kind, usual_min, due,
       round(CASE WHEN incident AND stage = 'quarantine' THEN 182400
                  WHEN incident AND stage = 'validate' THEN 1533900
                  WHEN incident AND stage = 'monitor' THEN 2960
                  WHEN incident AND stage = 'calibrate' THEN 214
                  ELSE usual_rows * jitter END) AS rows_out,
       round(CASE WHEN incident AND stage = 'validate' THEN 182400
                  WHEN incident AND stage = 'build' THEN 3100
                  WHEN incident AND stage = 'stores' THEN 1240
                  ELSE usual_rows * jitter * 0.0006 END) AS errors,
       round(CASE WHEN incident AND stage = 'build' THEN 164
                  WHEN incident AND stage IN ('project', 'recommend') THEN usual_min * 0.6
                  WHEN incident AND stage = 'calibrate' THEN 26
                  ELSE usual_min * jitter END, 1) AS minutes,
       CASE WHEN NOT incident THEN 'ok'
            WHEN stage = 'validate' THEN 'failed'
            WHEN stage = 'quarantine' THEN 'alarm'
            WHEN stage IN ('build', 'project', 'calibrate', 'monitor') THEN 'delayed'
            WHEN stage IN ('recommend', 'stores') THEN 'late'
            ELSE 'ok' END AS status
FROM r;

-- When each stage ran: one after another, the incident shifting the tail.
CREATE OR REPLACE TABLE dg_time AS
SELECT r.*,
       day + CASE WHEN day = DATE '2026-10-14' THEN
         CASE stage WHEN 'project' THEN TIME '08:35' WHEN 'monitor' THEN TIME '08:46'
                    WHEN 'calibrate' THEN TIME '08:52' WHEN 'recommend' THEN TIME '08:46'
                    WHEN 'stores' THEN TIME '09:03' ELSE NULL END END AS late_start
FROM dg_run r;
CREATE OR REPLACE TABLE dg_times AS
SELECT t.*,
       coalesce(late_start, t.day + CASE stage
         WHEN 'pos' THEN TIME '05:20' WHEN 'stock' THEN TIME '05:22' WHEN 'supplier' THEN TIME '05:24'
         WHEN 'ingest' THEN TIME '05:30' WHEN 'validate' THEN TIME '05:42' WHEN 'quarantine' THEN TIME '05:51'
         WHEN 'build' THEN TIME '05:51' WHEN 'project' THEN TIME '06:13' WHEN 'monitor' THEN TIME '06:31'
         WHEN 'calibrate' THEN TIME '06:37' WHEN 'recommend' THEN TIME '06:31' ELSE TIME '06:45' END) AS started
FROM dg_time t;
CREATE OR REPLACE TABLE dg_stage_run AS
SELECT day, seq, stage, title, kind, usual_min, due, rows_out, errors, minutes, status, started,
       started + INTERVAL (round(minutes * 60)) SECOND AS finished,
       strftime(day, '%a %-d %b') || CASE WHEN day = DATE '2026-10-14' THEN ' · incident' ELSE '' END AS run
FROM dg_times;

-- What flows along each connection in a run: the records the source handed
-- on (the rejected rows for quarantine), and the health of the receiving stage.
CREATE OR REPLACE TABLE dg_edge_run AS
SELECT s.run, f.src, f.dst,
       CASE WHEN f.dst = 'quarantine' THEN d.rows_out ELSE s.rows_out END AS n,
       CASE d.status WHEN 'ok' THEN 'ok' WHEN 'alarm' THEN 'failed' ELSE d.status END AS health
FROM dg_flow f
JOIN dg_stage_run s ON s.stage = f.src
JOIN dg_stage_run d ON d.stage = f.dst AND d.day = s.day;

-- The stage the detail panels show: the clicked one, else the first that is
-- not ok in this run.
CREATE OR REPLACE MACRO dg_pick() AS coalesce(nullif(getvariable('stage'), ''),
  (SELECT stage FROM dg_stage_run WHERE run = getvariable('run') AND status <> 'ok' ORDER BY seq LIMIT 1), 'ingest');

-- What went wrong, per stage and run, and where it started.
CREATE OR REPLACE TABLE dg_error AS
SELECT * FROM (VALUES
  (DATE '2026-10-14', 'validate', 'Column order changed: quantity and price swapped', 182400, TIME '05:44', 'stock'),
  (DATE '2026-10-14', 'quarantine', 'Rows held for the re-sent stock feed', 182400, TIME '05:51', 'validate'),
  (DATE '2026-10-14', 'build', 'Twins waiting for stock: no current on-hand', 3100, TIME '05:52', 'validate'),
  (DATE '2026-10-14', 'project', 'Projection started 2 h 22 min late', 9800, TIME '08:35', 'build'),
  (DATE '2026-10-14', 'monitor', 'Drift alarms on stale on-hand', 2960, TIME '08:47', 'build'),
  (DATE '2026-10-14', 'calibrate', 'Calibrations on stale on-hand, re-run after the fix', 214, TIME '08:52', 'monitor'),
  (DATE '2026-10-14', 'recommend', 'Orders computed after the 07:45 cut-off', 1240, TIME '08:54', 'build'),
  (DATE '2026-10-14', 'stores', 'Orders delivered 65 min after the 08:00 deadline', 1240, TIME '09:05', 'recommend'),
  (DATE '2026-10-13', 'validate', 'Duplicate receipt numbers', 412, TIME '05:44', 'pos'),
  (DATE '2026-10-13', 'ingest', 'Late file from one supplier, retried', 1, TIME '05:31', 'supplier')
) t(day, stage, error, n, seen, cause);

SELECT 'How does data move through our pipeline?'::DASHBOARD_TITLE;
SELECT max(finished)::ASOF FROM dg_stage_run WHERE day = DATE '2026-10-15';

SELECT run ::BUTTONS, hint::HINT, 'Nightly run'::LABEL
FROM (VALUES (1, 'Wed 14 Oct · incident', 'The stock feed changed its column order'),
             (2, 'Tue 13 Oct', 'A normal night')) t(k, run, hint)
ORDER BY k;

-- 2. Did the stores get their orders on time?
SELECT ('Did the stores get their orders on time? ' || getvariable('run'))::GROUP;
SELECT greatest(0, round(date_diff('second', day + due, finished) / 60))::METRIC,
       'Minutes past the 08:00 store deadline'::LABEL,
       '<=0:good; <30:warn; bad'::THRESHOLDS,
       'When the order lines reached the store systems, against their 08:00 deadline.'::INFO
FROM dg_stage_run WHERE run = getvariable('run') AND stage = 'stores';
SELECT sum(rows_out) FILTER (WHERE kind = 'source')::COMPACT, 'Records landed'::LABEL
FROM dg_stage_run WHERE run = getvariable('run');
SELECT max(rows_out) FILTER (WHERE stage = 'quarantine')::COMPACT, 'Rows rejected into quarantine'::LABEL,
       '<5000:good; <50000:warn; bad'::THRESHOLDS
FROM dg_stage_run WHERE run = getvariable('run');
SELECT count(*) FILTER (WHERE status <> 'ok')::METRIC, 'Stages not ok'::LABEL, '<1:good; <3:warn; bad'::THRESHOLDS
FROM dg_stage_run WHERE run = getvariable('run');
SELECT 1::ENDGROUP;

-- A small legend for the diagram's edge colours (#514: ::IMAGE, a data: SVG).
SELECT 4::COL;
SELECT 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyMjAgNjAiPjxjaXJjbGUgY3g9IjIwIiBjeT0iMjAiIHI9IjgiIGZpbGw9IiMzYWE4NzYiLz48dGV4dCB4PSIzNiIgeT0iMjUiIGZvbnQtc2l6ZT0iMTIiIGZvbnQtZmFtaWx5PSJzYW5zLXNlcmlmIiBmaWxsPSIjMzM0Ij5vazwvdGV4dD48Y2lyY2xlIGN4PSIyMCIgY3k9IjQwIiByPSI4IiBmaWxsPSIjZThhMzNkIi8+PHRleHQgeD0iMzYiIHk9IjQ1IiBmb250LXNpemU9IjEyIiBmb250LWZhbWlseT0ic2Fucy1zZXJpZiIgZmlsbD0iIzMzNCI+ZGVsYXllZDwvdGV4dD48Y2lyY2xlIGN4PSIxMjAiIGN5PSIyMCIgcj0iOCIgZmlsbD0iI2Q2NDU0NSIvPjx0ZXh0IHg9IjEzNiIgeT0iMjUiIGZvbnQtc2l6ZT0iMTIiIGZvbnQtZmFtaWx5PSJzYW5zLXNlcmlmIiBmaWxsPSIjMzM0Ij5mYWlsZWQ8L3RleHQ+PGNpcmNsZSBjeD0iMTIwIiBjeT0iNDAiIHI9IjgiIGZpbGw9IiNkNjQ1NDUiLz48dGV4dCB4PSIxMzYiIHk9IjQ1IiBmb250LXNpemU9IjEyIiBmb250LWZhbWlseT0ic2Fucy1zZXJpZiIgZmlsbD0iIzMzNCI+bGF0ZTwvdGV4dD48L3N2Zz4='::IMAGE,
       'Legend: a green dot means the stage ran on time, amber delayed, red failed or late'::ALT,
       'How to read the diagram below'::LABEL;

-- 3. What moves where, and where it got stuck.
SELECT 12::COL;
SELECT 360::HEIGHT;
SELECT 'stage'::SELECT_AS;
WITH cur AS (SELECT * FROM dg_stage_run WHERE run = getvariable('run'))
SELECT stage::DIAGRAM,
       title::LABEL,
       rows_out::COMPACT,
       status::BADGE,
       CASE WHEN run LIKE '%incident' THEN CASE stage
         WHEN 'stock' THEN '05:22 · The stock feed lands on time, but with its columns in a new order'
         WHEN 'validate' THEN '05:44 · Validate rejects 182K stock rows (38 %) into quarantine'
         WHEN 'build' THEN '08:35 · The parser is rolled back and stock re-sent: Build twin finishes after 2 h 44 min, not 22 min'
         WHEN 'recommend' THEN '08:54 · Recommend orders runs on the repaired twin, an hour late'
         WHEN 'stores' THEN '09:05 · The stores get their orders 65 min after the 08:00 deadline'
       END END::STORY,
       (SELECT list({'from': src, 'to': dst, 'value': n, 'status': health,
                     'style': CASE WHEN dst IN ('ingest', 'build') THEN 'flow-dots' ELSE 'flow' END}
                    ORDER BY src, dst)
        FROM dg_edge_run WHERE run = getvariable('run'))::EDGES,
       'tween'::TRANSITION,
       'Records per stage: edges as wide as what they carry, coloured by health'::TITLE,
       'Each box is a stage with the records it handed on and its status; each edge carries the records into the next stage. Red: failed or late; amber: delayed.'::INFO
FROM cur ORDER BY seq;

-- 4. The clicked stage (else the first one that is not ok).
SELECT (title || ' · ' || run)::SECTION
FROM dg_stage_run WHERE run = getvariable('run') AND stage = dg_pick();

SELECT 'This stage in this run'::GROUP;
SELECT coalesce((SELECT sum(n) FROM dg_edge_run e WHERE e.run = r.run AND e.dst = r.stage), r.rows_out)::COMPACT,
       'Rows in'::LABEL
FROM dg_stage_run r WHERE r.run = getvariable('run') AND r.stage = dg_pick();
SELECT r.rows_out::COMPACT, prev.rows_out::DELTA, 'vs the night before'::DELTA_LABEL, 'Rows out'::LABEL
FROM dg_stage_run r LEFT JOIN dg_stage_run prev ON prev.stage = r.stage AND prev.day = r.day - 1
WHERE r.run = getvariable('run') AND r.stage = dg_pick();
SELECT r.minutes::METRIC, 'Run time (min)'::LABEL,
       ('<=' || r.usual_min * 1.2 || ':good; <=' || r.usual_min * 2 || ':warn; bad')::THRESHOLDS,
       (SELECT list(h.minutes ORDER BY h.day) FROM dg_stage_run h WHERE h.stage = r.stage AND h.day <= r.day)::SPARKLINE,
       'Minutes from start to finish; amber above 1.2× its usual time, red above 2×. The line: its last 30 runs.'::INFO
FROM dg_stage_run r WHERE r.run = getvariable('run') AND r.stage = dg_pick();
SELECT round(100 * errors / greatest(rows_out + errors, 1), 2)::PERCENT, 'Error rate'::LABEL,
       '<0.5:good; <5:warn; bad'::THRESHOLDS
FROM dg_stage_run WHERE run = getvariable('run') AND stage = dg_pick();
SELECT 1::ENDGROUP;

SELECT 12::COL;
WITH s AS (SELECT * FROM dg_stage_run WHERE run = getvariable('run') AND stage = dg_pick()),
b AS (SELECT s.day + TIME '05:00' + INTERVAL (5 * i) MINUTE AS t FROM s, range(0, 61) r(i))
SELECT round(5 + date_diff('minute', s.day + TIME '05:00', b.t) / 60.0, 3)::XAXIS,
       round(CASE WHEN b.t + INTERVAL 5 MINUTE <= s.started OR b.t >= s.finished THEN 0
                  ELSE s.rows_out / greatest(1, date_diff('minute', s.started, s.finished) / 5.0)
                       * (1 + 0.25 * sin(epoch(b.t) / 900.0)) END)::LINECHART,
       CASE WHEN b.t = (SELECT min(t) FROM b) THEN hour(s.due) + minute(s.due) / 60.0 END::XLINE,
       CASE WHEN b.t = (SELECT min(t) FROM b) THEN 'deadline ' || strftime(DATE '2000-01-01' + s.due, '%H:%M') END::XLINE_LABEL,
       CASE WHEN b.t = (SELECT min(t) FROM b) AND s.day = DATE '2026-10-14' THEN 5 + 22 / 60.0 END::XBAND_FROM,
       CASE WHEN b.t = (SELECT min(t) FROM b) AND s.day = DATE '2026-10-14' THEN 8 + 35 / 60.0 END::XBAND_TO,
       CASE WHEN b.t = (SELECT min(t) FROM b) AND s.day = DATE '2026-10-14' THEN 'incident' END::XBAND_LABEL,
       ' h'::XFORMAT, 'comma'::YFORMAT,
       'tween'::TRANSITION,
       ('Records per 5 minutes at ' || s.title || ' (hour of the night), against its deadline')::TITLE
FROM b, s ORDER BY b.t;

SELECT 12::COL;
SELECT 'No errors at this stage in this run'::EMPTY;
SELECT e.error AS "What went wrong"::PAGED, e.n AS "Rows"::COMPACT, strftime(DATE '2000-01-01' + e.seen, '%H:%M') AS "Seen",
       c.title AS "Started at", '?v.stage=' || e.cause AS "Go there"::LINK
FROM dg_error e JOIN dg_stage c ON c.stage = e.cause
WHERE e.day = (SELECT day FROM dg_stage_run WHERE run = getvariable('run') LIMIT 1) AND e.stage = dg_pick()
ORDER BY e.seen;
