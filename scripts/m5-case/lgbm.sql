-- The comparison model: anofox_ml's LgbmRegressor with the M5 winner's
-- (YJ_STU) non-recursive parameters (Mcompetitions/M5-methods, Code of
-- Winning Methods/A1, 2-1. nonrecursive_store_TRAIN.py), on the same rows and
-- features as the global PerpetualRegressor, fitted at getvariable('cutoff')
-- with getvariable('seed'). One model for all stores (the winner fitted one
-- per store); boost_from_average = False has no counterpart in anofox_ml
-- (DataZooDE/anofox-ml#108). Its predictions are already the mean.
LOAD anofox_ml;
SET threads = 16;
SET memory_limit = '24GB';
SET anofox_ml_max_fit_values = 0;

.read feats.sql

INSERT INTO lgbm_fc
SELECT getvariable('cutoff'), getvariable('seed'), e.id, e.t, e.h, ml_predict(m.m, feats(e))
FROM ex e, (SELECT ml_fit_agg('LgbmRegressor', y::DOUBLE, feats(ex),
         '{"objective": {"Tweedie": {"variance_power": 1.1}}, "n_estimators": 3000, "learning_rate": 0.015,
           "num_leaves": 255, "min_child_samples": 255, "subsample": 0.5, "subsample_freq": 1,
           "colsample_bytree": 0.5, "max_bins": 100, "categorical_features": [19, 20, 21], "seed": '
           || getvariable('seed') || '}'
         ORDER BY id, o, h) AS m
       FROM ex WHERE o < getvariable('cutoff') AND o >= getvariable('cutoff') - 28 * 13) m
WHERE e.o = getvariable('cutoff');
