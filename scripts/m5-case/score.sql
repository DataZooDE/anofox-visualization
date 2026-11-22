-- WRMSSE of every method in fc(method, id, t, f): bottom forecasts summed up the 12 levels
CREATE OR REPLACE TABLE fc_agg AS SELECT f.method, l.level, l.key, f.t, sum(f.f) AS f FROM fc f JOIN lvl l USING (id) GROUP BY ALL;
CREATE OR REPLACE TABLE rmsse AS
SELECT f.method, f.level, f.key, sqrt(avg((a.y - f.f) ^ 2) / any_value(s.scale)) AS rmsse
FROM fc_agg f JOIN actual a USING (level, key, t) JOIN scale s USING (level, key) GROUP BY ALL;
CREATE OR REPLACE TABLE wrmsse_level AS
SELECT method, level, sum(w * rmsse) AS wrmsse FROM rmsse JOIN weight USING (level, key) GROUP BY ALL;
SELECT method, round(avg(wrmsse), 4) AS wrmsse FROM wrmsse_level GROUP BY 1 ORDER BY 2;
