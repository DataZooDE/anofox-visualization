-- How does data move through our pipeline? One run of an inventory
-- digital-twin pipeline (the stages of inventory-rs: ingest → build the twin
-- → step & project → monitor drift → calibrate → recommend, with a replay
-- check against history), drawn as a diagram from two small tables:
--   * dg_stages: one row per stage — its records and a status;
--   * dg_edges: what flows between stages, with a count.
-- ::DIAGRAM lays the stages out left to right; the loop calibrate → build
-- twin runs back. The stages appear one after another, records travel into
-- the twin as moving dots ('flow-dots'::LINESTYLE on the row they arrive at)
-- and the loop flows; Step / Play walk through three stages (::STORY), each
-- lighting its stage and every connection touching it. Click a stage to see
-- its details in the table below; with the keyboard, the arrow keys follow
-- the connections.
--
-- Synthetic, deterministic data.

CREATE OR REPLACE TABLE dg_stages AS
SELECT * FROM (VALUES
  (1, 'ingest',    'Ingest',            10000, 'ok',    'Read sales, stock and orders; 200 malformed rows rejected'),
  (2, 'build',     'Build twin',         9800, 'ok',    'One twin per SKU and store'),
  (3, 'step',      'Step & project',     9800, 'ok',    'Simulate stock 12 weeks ahead'),
  (4, 'replay',    'Replay history',     9800, 'ok',    'Re-run the last 26 weeks against recorded stock'),
  (5, 'monitor',   'Monitor drift',       412, 'alarm', '412 twins drift from recorded stock'),
  (6, 'calibrate', 'Calibrate',            37, 'warn',  '37 parameter changes proposed, 29 applied'),
  (7, 'recommend', 'Recommend orders',   1200, 'ok',    '1,200 order lines for next week')
) t(seq, stage, title, records, status, detail);

CREATE OR REPLACE TABLE dg_edges AS
SELECT * FROM (VALUES
  ('ingest',    'build',     9800),
  ('build',     'step',      9800),
  ('build',     'replay',    9800),
  ('step',      'monitor',   9800),
  ('replay',    'monitor',   9800),
  ('monitor',   'calibrate',  412),
  ('calibrate', 'build',       29),
  ('step',      'recommend', 1200)
) t(from_stage, to_stage, n);

SELECT 'How does data move through our pipeline?'::DASHBOARD_TITLE;

SELECT 'This run'::GROUP;
SELECT (SELECT records FROM dg_stages WHERE stage = 'ingest')::COMPACT, 'Records ingested'::LABEL;
SELECT (SELECT records FROM dg_stages WHERE stage = 'monitor')::METRIC, 'Twins drifting'::LABEL;
SELECT (SELECT n FROM dg_edges WHERE from_stage = 'calibrate')::METRIC, 'Calibrations applied'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT 'stage'::SELECT_AS;
SELECT stage::DIAGRAM,
       title::LABEL,
       records::COMPACT,
       status::BADGE,
       CASE stage WHEN 'build' THEN 'flow-dots' END::LINESTYLE,
       CASE stage
         WHEN 'ingest' THEN '10,000 records arrive; 200 malformed rows are rejected'
         WHEN 'monitor' THEN '412 twins drift away from the recorded stock'
         WHEN 'calibrate' THEN '29 calibrations go back into the twin'
       END::STORY,
       (SELECT list({'from': from_stage, 'to': to_stage, 'value': n,
                     'style': CASE WHEN to_stage = 'build' AND from_stage = 'calibrate' THEN 'flow' END}
                    ORDER BY from_stage, to_stage)
        FROM dg_edges)::EDGES,
       'One run: drift in 412 twins sends 29 calibrations back to the twin'::TITLE
FROM dg_stages ORDER BY seq;

SELECT 12::COL;
SELECT title AS "Stage"::TABLE, records AS "Records", status AS "Status"::BADGE, detail AS "What happened"
FROM dg_stages
WHERE coalesce(getvariable('stage'), '') IN ('', stage)
ORDER BY seq;
