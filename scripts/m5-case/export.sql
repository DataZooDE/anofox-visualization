-- The stored results of the daily M5 case, written to data/m5/ (run in web/).
SET threads = 12;
SET memory_limit = '24GB';

CREATE OR REPLACE MACRO label(m) AS CASE m
  WHEN 'perpetual' THEN 'one global model (anofox_ml)'
  WHEN 'lgbm' THEN 'LightGBM, the winner''s settings (anofox_ml)'
  WHEN 'naive' THEN 'the last day (Naive)'
  WHEN 'sma28' THEN 'mean of the last 28 days'
  WHEN 'snaive7' THEN 'same weekday last week' END;
CREATE OR REPLACE MACRO level_name(l) AS ['total', 'state', 'store', 'category', 'department', 'state × category',
  'state × department', 'store × category', 'store × department', 'item', 'item × state', 'item × store'][l];

COPY (SELECT method, label(method) AS label, round(wrmsse, 4) AS wrmsse, round(wrmsse_public, 4) AS wrmsse_public,
             round(mae, 4) AS mae, round(crps, 4) AS crps, round(bias, 2) AS bias,
             round(100 * cov50, 1) AS cov50, round(100 * cov80, 1) AS cov80, round(100 * cov95, 1) AS cov95
      FROM methods ORDER BY wrmsse) TO 'data/m5/methods.parquet' (COMPRESSION zstd);

COPY (SELECT method, label(method) AS label, level, level_name(level) AS level_name, round(wrmsse, 4) AS wrmsse
      FROM wrmsse_test WHERE method NOT LIKE 'seed %' ORDER BY method, level) TO 'data/m5/levels.parquet' (COMPRESSION zstd);

-- each seed's model alone: how far single fits scatter (the case averages them)
COPY (SELECT replace(t.method, 'seed ', '')::INT AS seed, round(avg(t.wrmsse), 4) AS wrmsse,
             round((SELECT avg(p.wrmsse) FROM wrmsse_public p WHERE p.method = t.method), 4) AS wrmsse_public
      FROM wrmsse_test t WHERE t.method LIKE 'seed %' GROUP BY t.method ORDER BY seed) TO 'data/m5/seeds.parquet' (COMPRESSION zstd);

-- the competition: the organizers' scores (Mcompetitions/M5-methods, "Scores and Ranks.xlsx"),
-- the top 50 teams in final-rank order and the benchmarks, next to this case's methods
COPY (SELECT method AS name, wrmsse, kind, CASE kind WHEN 'team' THEN row_number() OVER (PARTITION BY kind ORDER BY ord) END AS rank
      FROM (SELECT *, row_number() OVER () AS ord FROM read_csv(getvariable('official')))
      UNION ALL SELECT label(method), round(wrmsse, 3), 'this case', NULL FROM methods
      ORDER BY wrmsse) TO 'data/m5/leaderboard.parquet' (COMPRESSION zstd);

-- the tests: pooled Diebold–Mariano, and how many series differ significantly (5 %)
COPY (WITH s AS (
        SELECT baseline, 'mae' AS metric, d_mae AS d, p_mae AS p FROM dm_series
        UNION ALL SELECT baseline, 'crps', d_crps, p_crps FROM dm_series
        UNION ALL SELECT baseline, 'wse', d_se, p_se FROM dm_series),
      c AS (SELECT baseline, metric, count(*) FILTER (WHERE p < 0.05 AND d < 0) AS series_better,
                   count(*) FILTER (WHERE p < 0.05 AND d > 0) AS series_worse,
                   count(*) FILTER (WHERE NOT (p < 0.05) OR p IS NULL) AS series_no_difference FROM s GROUP BY ALL)
      SELECT p.metric, CASE p.metric WHEN 'mae' THEN 'MAE' WHEN 'crps' THEN 'CRPS' ELSE 'WRMSSE (daily part)' END AS metric_label,
             p.baseline, label(p.baseline) AS baseline_label, round(p.tree_loss, 6) AS tree_loss, round(p.baseline_loss, 6) AS baseline_loss,
             round(p.gain_pct, 1) AS gain_pct, round(p.dm.statistic, 3) AS dm_statistic, round(p.dm.p_value, 6) AS dm_p_value, c.series_better, c.series_worse, c.series_no_difference
      FROM dm_pooled p JOIN c USING (baseline, metric) ORDER BY metric, baseline) TO 'data/m5/tests.parquet' (COMPRESSION zstd);

COPY (SELECT metric, label(models[i]) AS method, list_contains(mcs.included_models, i) AS in_set, mcs.mcs_p_values[i] AS mcs_p_value
      FROM mcs, range(1, len(models) + 1) r(i) ORDER BY metric, mcs_p_value DESC) TO 'data/m5/mcs.parquet' (COMPRESSION zstd);

-- one row per series: its sales in the test days and each method's errors
COPY (WITH e AS (SELECT method, id, avg(ae) AS mae, avg(crps) AS crps FROM loss GROUP BY ALL),
      r AS (SELECT method, split_part(key, '|', 1) || '_' || split_part(key, '|', 2) AS id, rmsse FROM rmsse_test WHERE level = 12),
      w AS (SELECT split_part(key, '|', 1) || '_' || split_part(key, '|', 2) AS id, w FROM weight WHERE level = 12),
      u AS (SELECT id, sum(y) AS units FROM sales WHERE t > 1941 GROUP BY 1)
      SELECT m.id, m.item_id AS item, m.dept_id AS dept, m.cat_id AS category, m.store_id AS store, m.state_id AS state,
             u.units::INT AS units_test, round(w.w, 8) AS weight,
             round(max(e.mae) FILTER (WHERE e.method = 'perpetual'), 3) AS mae_tree,
             round(max(e.mae) FILTER (WHERE e.method = 'naive'), 3) AS mae_naive,
             round(max(e.mae) FILTER (WHERE e.method = 'sma28'), 3) AS mae_mean28,
             round(max(e.crps) FILTER (WHERE e.method = 'perpetual'), 3) AS crps_tree,
             round(max(e.crps) FILTER (WHERE e.method = 'naive'), 3) AS crps_naive,
             round(max(e.crps) FILTER (WHERE e.method = 'sma28'), 3) AS crps_mean28,
             round(max(r.rmsse) FILTER (WHERE r.method = 'perpetual'), 3) AS rmsse_tree,
             round(max(r.rmsse) FILTER (WHERE r.method = 'naive'), 3) AS rmsse_naive,
             round(max(r.rmsse) FILTER (WHERE r.method = 'sma28'), 3) AS rmsse_mean28,
             round(any_value(dn.p_mae), 4) AS p_mae_vs_naive, round(any_value(dm.p_mae), 4) AS p_mae_vs_mean28
      FROM meta m JOIN u USING (id) JOIN w USING (id) JOIN e USING (id) JOIN r ON r.id = m.id AND r.method = e.method
      LEFT JOIN dm_series dn ON dn.id = m.id AND dn.baseline = 'naive'
      LEFT JOIN dm_series dm ON dm.id = m.id AND dm.baseline = 'sma28'
      GROUP BY ALL ORDER BY m.id) TO 'data/m5/series.parquet' (COMPRESSION zstd);

-- every series' sales over the 8 weeks before the test days and in them, and the four forecasts
COPY (SELECT s.id, c.ds, 'actual' AS method, s.y::DOUBLE AS units FROM sales s JOIN cal c USING (t) WHERE s.t > 1885
      UNION ALL SELECT p.id, c.ds, p.method, round(p.f, 2) FROM pfa p JOIN cal c USING (t) WHERE p.cutoff = 1941
      ORDER BY 1, 3, 2) TO 'data/m5/forecasts.parquet' (COMPRESSION zstd, ROW_GROUP_SIZE 100000);

-- the top levels (total, states, stores, categories): 16 weeks of sales and the forecasts
COPY (SELECT a.level, level_name(a.level) AS level_name, a.key, c.ds, 'actual' AS method, a.y AS units
      FROM agg a JOIN cal c USING (t) WHERE a.level <= 4 AND a.t > 1857
      -- summed exactly (DECIMAL), so the rounding can't flip with the order of a parallel sum
      UNION ALL SELECT l.level, level_name(l.level), l.key, c.ds, p.method, round(sum(round(p.f, 6)::DECIMAL(38, 6)), 1)::DOUBLE
      FROM pfa p JOIN lvl l USING (id) JOIN cal c USING (t) WHERE p.cutoff = 1941 AND l.level <= 4 GROUP BY ALL
      ORDER BY 1, 3, 5, 4) TO 'data/m5/totals.parquet' (COMPRESSION zstd);

-- six items to look at, with the global model's 50 / 80 / 95 % intervals
CREATE OR REPLACE TABLE picks AS
WITH x AS (SELECT l.id, any_value(m.cat_id) AS cat, sum(l.y) AS units, any_value(r.zero28) AS z, any_value(r.age) AS age
           FROM loss l JOIN meta m USING (id) JOIN roll r ON r.id = l.id AND r.t = 1941 WHERE l.method = 'perpetual' GROUP BY 1)
SELECT * FROM (
  (SELECT 'best seller' AS why, id FROM x ORDER BY units DESC LIMIT 1)
  UNION ALL (SELECT 'household best seller', id FROM x WHERE cat = 'HOUSEHOLD' ORDER BY units DESC LIMIT 1)
  UNION ALL (SELECT 'hobbies best seller', id FROM x WHERE cat = 'HOBBIES' ORDER BY units DESC LIMIT 1)
  UNION ALL (SELECT 'sells on few days', id FROM x WHERE z >= 0.6 ORDER BY units DESC LIMIT 1)
  UNION ALL (SELECT 'new item', id FROM x WHERE age < 120 ORDER BY units DESC LIMIT 1)
  UNION ALL (SELECT 'a typical item', id FROM x ORDER BY abs(units - (SELECT median(units) FROM x)), id LIMIT 1));
COPY (SELECT p.why, q.id, c.ds, round(max(q.q) FILTER (WHERE q.tau = 0.025), 2) AS lo95, round(max(q.q) FILTER (WHERE q.tau = 0.1), 2) AS lo80,
             round(max(q.q) FILTER (WHERE q.tau = 0.25), 2) AS lo50, round(max(q.q) FILTER (WHERE q.tau = 0.75), 2) AS hi50,
             round(max(q.q) FILTER (WHERE q.tau = 0.9), 2) AS hi80, round(max(q.q) FILTER (WHERE q.tau = 0.975), 2) AS hi95
      FROM picks p JOIN qf q ON q.id = p.id AND q.method = 'perpetual' JOIN cal c ON c.t = q.t
      GROUP BY ALL ORDER BY 1, 3) TO 'data/m5/examples.parquet' (COMPRESSION zstd);

COPY (SELECT n.feature, n.source, round(100 * (i.mae / b.mae - 1), 2) AS mae_rise_pct
      FROM imp i JOIN fnames n USING (k), imp_base b ORDER BY mae_rise_pct DESC) TO 'data/m5/importance.parquet' (COMPRESSION zstd);

COPY (SELECT (SELECT count(DISTINCT id) FROM pf) AS series,
             (SELECT count(*) FROM ex WHERE o < 1941 AND o >= 1941 - 28 * 13) AS training_rows,
             (SELECT count(DISTINCT seed) FROM models) AS seeds,
             (SELECT min(ds) FROM cal WHERE t = 1942) AS first_day, (SELECT max(ds) FROM cal WHERE t = 1969) AS last_day,
             (SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_ml') AS anofox_ml,
             (SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_forecast') AS anofox_forecast,
             (SELECT extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_statistics') AS anofox_statistics,
             version() AS duckdb) TO 'data/m5/provenance.parquet' (COMPRESSION zstd);
