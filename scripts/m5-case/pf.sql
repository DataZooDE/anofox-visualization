-- All point forecasts in one table: the global model (the mean of its five
-- seeds) and the baselines, at both cutoffs.
CREATE OR REPLACE TABLE pf AS
SELECT 'perpetual' AS method, cutoff, id, t, any_value(h) AS h, avg(f) AS f FROM tree_fc GROUP BY cutoff, id, t
UNION ALL SELECT b.method, 1941, b.id, c.t, b.forecast_step, b.yhat FROM base b JOIN cal c ON c.ds = b.ds::DATE
UNION ALL SELECT b.method, 1913, b.id, c.t, b.forecast_step, b.yhat FROM base_cal b JOIN cal c ON c.ds = b.ds::DATE;
-- each seed alone, for the spread of single fits
CREATE OR REPLACE TABLE fc_seeds AS SELECT cutoff, seed, id, t, f FROM tree_fc;
