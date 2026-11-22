-- The global PerpetualRegressor for all 30,490 series, fitted at a cutoff
-- (getvariable('cutoff')) on the year of 28-day blocks before it, forecasting
-- the 28 days after it, with one seed (getvariable('seed')). Settings are fixed:
-- Tweedie loss for the zero-heavy daily counts, perpetual's default budget, no
-- tuning. One fit on 11 million rows is a noisy draw (another seed, or a change
-- in the 13th decimal of the features, moves its WRMSSE by a few hundredths),
-- so the case fits five seeds and averages them (pf.sql).
LOAD anofox_ml;
SET threads = 16;
SET memory_limit = '24GB';
SET anofox_ml_max_fit_values = 0;

.read feats.sql

CREATE OR REPLACE TEMP TABLE model_at AS
SELECT getvariable('cutoff') AS cutoff, getvariable('seed') AS seed,
       ml_fit_agg('PerpetualRegressor', y::DOUBLE, feats(ex),
         '{"budget": 0.5, "objective": {"TweedieLoss": {"p": 1.5}}, "categorical_features": [19, 20, 21], "seed": '
           || getvariable('seed') || '}'
         ORDER BY id, o, h) AS m
FROM ex WHERE o < getvariable('cutoff') AND o >= getvariable('cutoff') - 28 * 13;

-- perpetual returns the log of the mean for Tweedie (DataZooDE/anofox-ml#107)
INSERT INTO tree_fc
SELECT m.cutoff, m.seed, e.id, e.t, e.h, exp(ml_predict(m.m, feats(e)))
FROM ex e, model_at m WHERE e.o = getvariable('cutoff');
INSERT INTO models SELECT * FROM model_at;
