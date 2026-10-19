#!/usr/bin/env python3
"""Conformance harness: every anofox-forecast producer → our plots.

anofox_forecast does not tag its functions (`duckdb_functions().tags` is
empty, no `anofox_contract_version()`), so the producers are a curated
registry: each `ts_*` table macro, table function and aggregate the loaded
build exposes is either a producer with a recipe below or out of scope with a
reason. A function in neither list is reported MISS and fails the run, so a
new upstream function forces a decision; a function tagged
`anofox.output` upstream later needs a recipe too.

A producer is `direct` when its output plots as returned, `adapter` when it
needs the reshaping SQL shown in its recipe (that SQL is what a user writes
today; each adapter is a gap listed in the PR). Each output goes to
`anofox_plot(...)` and to its schema's macro; the run fails on a SQL error, a
missing SVG or an unexpected render warning. A multi-method test table must
draw one panel per method (PANELS). Every SVG is written to the output
directory, plus a PNG montage when `rsvg-convert` and Pillow exist.

    python3 scripts/forecast_conformance.py \
        --duckdb ./duckdb --ext ./anofox_visualization.duckdb_extension \
        --out /tmp/forecast-viz

The DuckDB CLI must be able to `LOAD anofox_forecast`: `INSTALL anofox_forecast
FROM community` (the release) or, in `duckdb -unsigned`, `FORCE INSTALL
anofox_forecast FROM 'https://get.erpl.io'` (main). Both are supported; the
recipes follow the build's API (the conformal macros changed between them).
Exit status 1 when any case fails or a function is not classified.
"""

from __future__ import annotations

import os
import re
import sys

from conformance_lib import check as _check, ext_path, known_bug, parser, run_all, run_duckdb, svg_names, write_montage

# ── demo data ────────────────────────────────────────────────────────────────
# Three daily series (A, B, C) of 84 days: trend, a weekly season, an
# exogenous driver x, and a level shift in B at day 50. Backtests (two folds
# of 7 days, two models) and a calibration table feed the evaluation and
# conformal producers.
SETUP = """
CREATE TABLE s AS
SELECT id, (TIMESTAMP '2024-01-01' + INTERVAL (i) DAY) AS ds,
       base + 0.3 * i + 5 * sin(2 * pi() * i / 7)
         + CASE WHEN id = 'B' AND i >= 50 THEN 15 ELSE 0 END
         + 1.6 * cos(i * 0.9) + 0.7 * sin(i * 1.7 + base) AS y,
       2 * cos(i * 0.9) AS x
FROM (VALUES ('A', 20.0), ('B', 40.0), ('C', 60.0)) t(id, base), range(84) r(i);
CREATE TABLE fut AS
SELECT id, (TIMESTAMP '2024-01-01' + INTERVAL (i) DAY) AS ds, 2 * cos(i * 0.9) AS x
FROM (VALUES ('A'), ('B'), ('C')) t(id), range(84, 98) r(i);
CREATE TABLE sa AS SELECT ds, y FROM s WHERE id = 'A';
CREATE TABLE sx AS SELECT ds, y, x FROM s WHERE id = 'A';
CREATE TABLE futa AS SELECT ds, x FROM fut WHERE id = 'A';
CREATE TABLE sv AS SELECT ds, y AS y1, x AS y2 FROM s WHERE id = 'A';
CREATE TABLE folds AS SELECT * FROM ts_cv_folds_by('s', id, ds, y, 2, 7, MAP{});
CREATE TABLE bt AS
SELECT * FROM ts_cv_forecast_by('folds', id, ds, y, 'SeasonalNaive', MAP{'seasonal_period': '7'})
UNION ALL BY NAME
SELECT * FROM ts_cv_forecast_by('folds', id, ds, y, 'AutoETS', MAP{'seasonal_period': '7'});
CREATE TABLE btc AS SELECT id, ds, y AS actual, yhat AS forecast, yhat AS point
FROM bt WHERE model_name = 'SeasonalNaive';
CREATE TABLE fc AS SELECT * FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', MAP{'seasonal_period': '7'});
"""
# The dated conformal API (main) calibrates per group into a table.
SETUP_DATED = "CREATE TABLE cal AS SELECT * FROM ts_conformal_calibrate('btc', id, actual, forecast, params := {'alpha': 0.1});"

PRED, SUMMARY, TERMS, OBS, CURVE, TEST, EVENTS, DECOMP = (
    "prediction", "summary", "terms", "obs", "curve", "test", "events", "decomposition")
SPECIFIC = {
    PRED: "anofox_plot_prediction(t)",
    SUMMARY: "anofox_plot_summary(t)",
    TERMS: "anofox_plot_terms(t)",
    OBS: "anofox_plot_diagnostics(t)",
    CURVE: "anofox_plot_curve(t)",
    TEST: "anofox_plot_tests(t)",
    EVENTS: "anofox_plot_events(t)",
    DECOMP: "anofox_plot_decomposition(t)",
}

FC = "MAP{'seasonal_period': '7'}"
# A rarer changepoint prior than the default: the level shift in B, not every trough.
CP = "MAP{'hazard_lambda': '50'}"
MEMBERS = "['AutoETS', 'Theta', 'SeasonalNaive']"
# A backtest as the metric macros take it: one model, the actuals in y.
# model_name carries the selected spec (AutoETS(A,A,A)), hence LIKE.
BT = "(SELECT id, ds, y, yhat FROM bt WHERE model_name LIKE 'AutoETS%')"


def metric(call: str, key: str = "id") -> str:
    """A wide one-metric table (id, <metric>) → summary rows."""
    return (f"(UNPIVOT (SELECT {key} AS id, 'AutoETS' AS model_id, * EXCLUDE ({key}) FROM {call}) "
            "ON COLUMNS(* EXCLUDE (id, model_id)) INTO NAME metric VALUE value)")


def numbers(call: str, cols: str, key: str = "id") -> str:
    """Selected per-series numbers of a wide table → summary rows, one model per series."""
    return (f"(UNPIVOT (SELECT {key} AS model_id, {cols} FROM {call}) "
            "ON COLUMNS(* EXCLUDE (model_id)) INTO NAME metric VALUE value)")


def agg(call: str) -> str:
    """An aggregate per series, its STRUCT unnested to columns."""
    return f"(SELECT id, unnest({call}, recursive := true) FROM s GROUP BY id)"


def lags(call: str, prefix: str, curve: str) -> str:
    """The <prefix>1..k feature columns → an acf / pacf curve per series."""
    return (f"(SELECT id AS series, '{curve}' AS curve_type, "
            f"replace(name, '{prefix}', '')::INTEGER AS x, value AS y "
            f"FROM (UNPIVOT (SELECT id, COLUMNS('^{prefix}[0-9]+$') FROM {call}) "
            "ON COLUMNS(* EXCLUDE (id)) INTO NAME name VALUE value))")


def producers(dated: bool) -> dict[str, dict]:
    """name → {schema, src, kind[, specific]}: `src` is FROM-able, `kind`
    direct | adapter, `specific` overrides the schema's macro call."""
    P: dict[str, dict] = {}

    def add(name, schema, src, kind="adapter", specific=None):
        P[name] = {"schema": schema, "src": src, "kind": kind, "specific": specific}

    # ── forecasts → prediction ──────────────────────────────────────────────
    add("ts_forecast_by", PRED, f"ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', {FC})", "direct")
    add("ts_forecast_exog_by", PRED,
        "ts_forecast_exog_by('s', id, ds, y, ['x'], 'fut', ds, ['x'], '1d', 'AutoARIMA', 14, MAP{})", "direct")
    add("ts_forecast_ensemble_by", PRED,
        f"ts_forecast_ensemble_by('s', id, ds, y, {MEMBERS}, 14, '1d', combination_method := 'mean', seasonal_period := 7)",
        "direct")
    add("ts_forecast_panel_by", PRED, f"ts_forecast_panel_by('s', id, ds, y, 'GlobalETS', 14, '1d', {FC})", "direct")
    add("ts_cv_forecast_by", PRED, f"ts_cv_forecast_by('folds', id, ds, y, 'SeasonalNaive', {FC})", "direct")
    add("ts_forecast", PRED,
        "(SELECT unnest(generate_series(1, len(point_forecasts))) AS period, unnest(point_forecasts) AS yhat, "
        "unnest(lower_bounds) AS yhat_lower, unnest(upper_bounds) AS yhat_upper, model_name "
        f"FROM ts_forecast('sa', ds, y, 'AutoETS', 14, {FC}))")
    add("ts_forecast_exog", PRED,
        "(SELECT unnest(generate_series(1, len(point_forecasts))) AS period, unnest(point_forecasts) AS yhat, "
        "unnest(lower_bounds) AS yhat_lower, unnest(upper_bounds) AS yhat_upper, model_name "
        "FROM ts_forecast_exog('sx', ds, y, ['x'], 'futa', ds, ['x'], 'AutoARIMA', 14, MAP{}))")
    add("ts_forecast_agg", PRED,
        "(SELECT id, unnest(f.forecast_timestamp) AS ds, unnest(f.point_forecast) AS yhat, "
        "unnest(f.lower_90) AS yhat_lower, unnest(f.upper_90) AS yhat_upper, f.model_name "
        f"FROM (SELECT id, ts_forecast_agg(ds, y, 'AutoETS', 14, {FC}) AS f FROM s GROUP BY id))")
    add("ts_forecast_var_by", PRED,
        "(SELECT variable AS id, ds, forecast_value AS yhat FROM ts_forecast_var_by('sv', 'ds', ['y1', 'y2'], 14, '1d'))")
    if dated:
        add("ts_conformal_by", PRED,
            "(SELECT group_col AS id, date_col AS ds, point AS yhat, lower AS yhat_lower, upper AS yhat_upper, "
            "nominal_level AS level FROM ts_conformal_by('btc', id, ds, actual, forecast, point, params := {'alpha': 0.1}))")
        add("ts_conformal_apply_by", PRED,
            "(SELECT group_col AS id, date_col AS ds, point AS yhat, lower AS yhat_lower, upper AS yhat_upper "
            "FROM ts_conformal_apply_by('fc', id, ds, yhat, 'cal'))")
    else:
        add("ts_conformal_by", PRED,
            "(SELECT group_col AS id, unnest(generate_series(1, len(point))) AS period, unnest(point) AS yhat, "
            "unnest(lower) AS yhat_lower, unnest(upper) AS yhat_upper "
            "FROM ts_conformal_by('btc', id, actual, forecast, point, MAP{'alpha': '0.1'}))")
        add("ts_conformal_apply_by", PRED,
            "(SELECT group_col AS id, unnest(generate_series(1, len(lower))) AS period, "
            "unnest(list_transform(list_zip(lower, upper), b -> (b[1] + b[2]) / 2)) AS yhat, "
            "unnest(lower) AS yhat_lower, unnest(upper) AS yhat_upper FROM ts_conformal_apply_by('fc', id, yhat, 3.0))")
    # trend over the data: the detrend fit as yhat, the series as y
    add("ts_detrend_by", PRED,
        "(SELECT d.id, unnest(generate_series(1, len(d.trend))) AS period, unnest(h.ys) AS y, unnest(d.trend) AS yhat, "
        "d.method AS model_name FROM ts_detrend_by('s', id, ds, y, 'auto') d "
        "JOIN (SELECT id, list(y ORDER BY ds) AS ys FROM s GROUP BY id) h USING (id))")
    # decompositions (#188): the list outputs plot as they come.
    add("ts_mstl_decomposition_by", DECOMP,
        "(SELECT * FROM ts_mstl_decomposition_by('s', id, ds, y, MAP{'seasonal_periods': '[7]'}) WHERE id = 'B')",
        "direct")
    add("ts_forecast_explain_by", DECOMP,
        "(SELECT * FROM ts_forecast_explain_by('s', id, ds, y, 'ETS', 14, {seasonal_period: 7}) WHERE id = 'A')",
        "direct")

    # ── in-sample fit → obs (the plot.lm 2×2) ───────────────────────────────
    add("ts_forecast_inspect_by", OBS,
        "(SELECT id AS model_id, unnest(generate_series(1, len((inspection).fitted_values))) AS row_id, "
        "unnest((inspection).fitted_values) AS fitted, unnest((inspection).residuals) AS residual "
        "FROM ts_forecast_inspect_by('s', id, ds, y, 'AutoETS', {seasonal_period: 7}) WHERE id = 'A')")

    # ── ensembles and periods → terms ───────────────────────────────────────
    for f in ("ts_ensemble_inspect_by", "ts_auto_ensemble_inspect_by"):
        call = (f"{f}('s', id, ds, y, {MEMBERS}, combination_method := 'mean', seasonal_period := 7)"
                if f == "ts_ensemble_inspect_by" else
                f"{f}('s', id, ds, y, top_k := 3, combination_method := 'mean', seasonal_period := 7)")
        add(f, TERMS, f"(SELECT id AS model_id, member_name AS term, weight AS estimate FROM {call})")
    add("ts_detect_periods_by", TERMS,
        "(SELECT id AS model_id, 'period ' || p.period AS term, p.strength AS estimate "
        "FROM (SELECT id, unnest(periods) AS p FROM ts_detect_periods_by('s', id, ds, y, MAP{})))")
    add("ts_detect_periods_agg", TERMS,
        "(SELECT id AS model_id, 'period ' || p.period AS term, p.strength AS estimate "
        "FROM (SELECT id, unnest(r.periods) AS p FROM (SELECT id, ts_detect_periods_agg(ds, y) AS r FROM s GROUP BY id)))")

    # ── changepoints and peaks → events ─────────────────────────────────────
    add("ts_detect_changepoints_by", EVENTS,
        "(SELECT id AS series, ds, 'changepoint' AS kind, 'p = ' || round(changepoint_probability, 2) AS label, "
        "CASE WHEN changepoint_probability >= 0.9 THEN 'alarm' ELSE 'warning' END AS severity, "
        f"changepoint_probability AS value FROM ts_detect_changepoints_by('s', id, ds, y, {CP}) WHERE is_changepoint)")
    add("ts_detect_changepoints_agg", EVENTS,
        "(SELECT id AS series, \"timestamp\" AS ds, 'changepoint' AS kind, changepoint_probability AS value "
        f"FROM (SELECT id, unnest(ts_detect_changepoints_agg(ds, y, {CP}), recursive := true) FROM s GROUP BY id) "
        "WHERE is_changepoint)")
    add("ts_detect_peaks_by", EVENTS,
        "(SELECT id AS series, p.index AS period, 'peak' AS kind, round(p.value, 1)::VARCHAR AS label, "
        "p.prominence AS value FROM (SELECT id, unnest(peaks) AS p FROM ts_detect_peaks_by('s', id, ds, y, MAP{})))")
    add("ts_analyze_peak_timing_by", EVENTS,
        "(SELECT id AS series, unnest(peak_times)::BIGINT AS period, 'peak' AS kind "
        "FROM ts_analyze_peak_timing_by('s', id, ds, y, 7.0, MAP{}))")

    # ── evaluation → summary ────────────────────────────────────────────────
    for m in ("mae", "mse", "rmse", "mape", "smape", "r2", "bias"):
        add(f"ts_{m}_by", SUMMARY, metric(f"ts_{m}_by({BT}, 'ds', 'y', 'yhat')"))
    # MASE scales by the in-sample naive error: the history rows carry no yhat.
    # Sorted: on an unsorted UNION ALL ts_mase_by returns NULL (main 5608238).
    hist = ("(SELECT * FROM (SELECT id, ds, y, NULL::DOUBLE AS yhat FROM s WHERE ds < (SELECT min(ds) FROM bt) "
            f"UNION ALL SELECT * FROM {BT}) ORDER BY id, ds)")
    add("ts_mase_by", SUMMARY, metric(
        f"ts_mase_by({hist}, 'ds', 'y', 'yhat', seasonal_period := 7)" if dated else
        f"ts_mase_by((SELECT *, lag(y, 7) OVER (PARTITION BY id ORDER BY ds) AS base FROM {BT}), 'ds', 'y', 'yhat', 'base')"))
    add("ts_rmae_by", SUMMARY, metric(
        "ts_rmae_by((SELECT a.id, a.ds, a.y, a.yhat AS p1, b.yhat AS p2 FROM bt a JOIN bt b USING (id, ds) "
        "WHERE a.model_name LIKE 'AutoETS%' AND b.model_name = 'SeasonalNaive'), 'ds', 'y', 'p1', 'p2')"))
    add("ts_quantile_loss_by", SUMMARY, metric(f"ts_quantile_loss_by({BT}, 'ds', 'y', 'yhat', 0.5)"))
    add("ts_coverage_by", SUMMARY, metric(
        "ts_coverage_by((SELECT id, ds, y, yhat_lower, yhat_upper FROM bt WHERE model_name LIKE 'AutoETS%'), "
        "'ds', 'y', 'yhat_lower', 'yhat_upper')"))
    add("ts_interval_width_by", SUMMARY, metric(
        "ts_interval_width_by('fc', id, ds, yhat_lower, yhat_upper)" if dated else
        "ts_interval_width_by('fc', id, yhat_lower, yhat_upper)", key="group_col"))
    add("ts_conformal_calibrate", SUMMARY,
        numbers("ts_conformal_calibrate('btc', id, actual, forecast, params := {'alpha': 0.1})",
                "conformity_score, coverage, nominal_level", key="group_col") if dated else
        "(UNPIVOT (SELECT 'pooled' AS model_id, conformity_score, coverage FROM "
        "ts_conformal_calibrate('btc', actual, forecast, MAP{'alpha': '0.1'})) "
        "ON COLUMNS(* EXCLUDE (model_id)) INTO NAME metric VALUE value)")

    # ── per-series characteristics → summary / curve ────────────────────────
    stats = "trend_strength, seasonality_strength, entropy, stability, coef_variation"
    add("ts_stats_by", SUMMARY, numbers("ts_stats_by('s', id, ds, y, '1d')", stats))
    add("ts_stats_agg", SUMMARY, numbers(agg("ts_stats_agg(ds, y)"), stats))
    quality = "structural_score, temporal_score, magnitude_score, behavioral_score, overall_score"
    add("ts_data_quality_by", SUMMARY, numbers("ts_data_quality_by('s', id, ds, y, 10, '1d')", quality, key="unique_id"))
    add("ts_data_quality_agg", SUMMARY, numbers(agg("ts_data_quality_agg(ds, y)"), quality))
    season = "seasonal_strength, timing_variability"
    add("ts_classify_seasonality_by", SUMMARY, numbers("ts_classify_seasonality_by('s', id, ds, y, 7.0)", season))
    add("ts_classify_seasonality_agg", SUMMARY, numbers(agg("ts_classify_seasonality_agg(ds, y, 7.0)"), season))
    add("ts_features_by", CURVE, lags("ts_features_by('s', id, ds, y)", "autocorrelation_lag", "acf"))
    add("ts_features_agg", CURVE, lags(agg("ts_features_agg(ds, y)"), "partial_autocorrelation_lag", "pacf"))

    # ── residual tests → test ───────────────────────────────────────────────
    for f in ("adf", "kpss", "ljung_box", "jarque_bera"):
        add(f"ts_{f}_by", TEST, f"ts_{f}_by('s', id, ds, y)", "direct")
    # no p_value and no method: name the test so the table reads as one
    add("ts_durbin_watson_by", TEST,
        "(SELECT id, 'Durbin-Watson' AS method, unnest(durbin_watson) FROM ts_durbin_watson_by('s', id, ds, y))")
    add("ts_stationarity_by", TEST,
        "(WITH r AS (SELECT id, unnest(stationarity) FROM ts_stationarity_by('s', id, ds, y)) "
        "SELECT id, 'ADF' AS method, adf_statistic AS statistic, adf_p_value AS p_value FROM r "
        "UNION ALL SELECT id, 'KPSS', kpss_statistic, kpss_p_value FROM r)")
    add("ts_residual_diagnostics_by", TEST,
        "(WITH r AS (SELECT id, unnest(residual_diagnostics) FROM ts_residual_diagnostics_by('s', id, ds, y)) "
        "SELECT id, 'Ljung-Box' AS method, lb_statistic AS statistic, lb_p_value AS p_value FROM r "
        "UNION ALL SELECT id, 'Jarque-Bera', jb_statistic, jb_p_value FROM r "
        "UNION ALL SELECT id, 'Durbin-Watson', dw_statistic, NULL FROM r)")
    return P


# Functions that are not a model result (data preparation, validation,
# bookkeeping) or a twin of a covered producer.
OUT_OF_SCOPE = {
    **{f: "data preparation (returns the series)" for f in (
        "ts_fill_forward_by", "ts_fill_gaps_by", "ts_fill_nulls_backward_by", "ts_fill_nulls_const_by",
        "ts_fill_nulls_forward_by", "ts_fill_nulls_mean_by", "ts_fill_unknown_by", "ts_mark_unknown_by",
        "ts_drop_constant_by", "ts_drop_edge_zeros_by", "ts_drop_gappy_by", "ts_drop_leading_zeros_by",
        "ts_drop_short_by", "ts_drop_trailing_zeros_by", "ts_drop_zeros_by", "ts_diff_by",
        "ts_fill_forward_operator")},
    **{f: "cross-validation split (the folds, not a result; plotted through ts_cv_forecast_by)" for f in (
        "ts_cv_folds_by", "ts_cv_split_by", "ts_cv_split_folds_by", "ts_cv_split_index_by", "ts_cv_hydrate_by",
        "ts_check_leakage")},
    **{f: "key / timestamp validation and hierarchy helpers" for f in (
        "ts_validate_timestamps_by", "ts_validate_timestamps_summary_by", "ts_validate_separator",
        "ts_combine_keys", "ts_split_keys", "ts_aggregate_hierarchy")},
    **{f: "configuration listing" for f in ("ts_features_list", "ts_features_config_template")},
    "ts_features_table": "feature names × values of one series (covered by ts_features_by)",
    "ts_features": "the aggregate behind ts_features_agg (same output)",
    "ts_stats": "alias of ts_stats_by",
    "ts_stats_summary": "dataset roll-up of a ts_stats table (one row)",
    "ts_quality_report": "dataset roll-up of a ts_stats table (one row)",
    "ts_data_quality": "alias of ts_data_quality_by",
    "ts_data_quality_summary": "dataset roll-up (one row per dimension)",
    "ts_detect_changepoints": "single-series twin of ts_detect_changepoints_by",
    "ts_detect_peaks": "single-series twin of ts_detect_peaks_by",
    "ts_detect_periods": "single-series twin of ts_detect_periods_by",
    "ts_analyze_peak_timing": "single-series twin of ts_analyze_peak_timing_by",
    "ts_classify_seasonality": "single-series twin of ts_classify_seasonality_by",
}

# Cases beyond the per-producer pair: what a forecast dashboard draws.
HISTORY = ("SELECT id, ds, y, NULL::DOUBLE AS yhat, NULL::DOUBLE AS yhat_lower, NULL::DOUBLE AS yhat_upper, "
           "NULL::VARCHAR AS model_name FROM s WHERE ds >= TIMESTAMP '2024-02-26'")
EXTRA = {
    "forecast_with_history": f"SELECT anofox_plot_prediction(p) FROM ({HISTORY} UNION ALL BY NAME "
                             f"SELECT * FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', {FC})) p",
    # two confidence levels → a nested fan; 95 and 0.8 both read as fractions
    "forecast_fan_levels": "SELECT anofox_plot_prediction(p) FROM ("
                           f"{HISTORY} UNION ALL BY NAME "
                           "SELECT *, 0.8 AS level FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', "
                           "MAP{'seasonal_period': '7', 'confidence_level': '0.8'}) UNION ALL BY NAME "
                           "SELECT *, 95 AS level FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', "
                           "MAP{'seasonal_period': '7', 'confidence_level': '0.95'})) p WHERE id = 'A'",
    "forecast_models": "SELECT anofox_plot_prediction(p) FROM ("
                       f"{HISTORY} UNION ALL BY NAME "
                       f"SELECT * FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', {FC}) UNION ALL BY NAME "
                       f"SELECT * FROM ts_forecast_by('s', id, ds, y, 'Theta', 14, '1d', {FC}) UNION ALL BY NAME "
                       f"SELECT * FROM ts_forecast_by('s', id, ds, y, 'SeasonalNaive', 14, '1d', {FC})) p WHERE id = 'B'",
    "forecast_per_series": f"SELECT id, anofox_plot_prediction(f) FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', {FC}) f "
                           "GROUP BY id ORDER BY id",
    "backtest_two_models": "SELECT anofox_plot_prediction(b) FROM bt b WHERE id = 'A'",
    "changepoints_over_history": "SELECT anofox_plot_prediction(p, events := (SELECT list(e) FROM ("
                                 "SELECT id, ds, 'changepoint' AS kind, 'alarm' AS severity "
                                 f"FROM ts_detect_changepoints_by('s', id, ds, y, {CP}) WHERE is_changepoint AND id = 'B') e)) "
                                 f"FROM ({HISTORY.replace('2024-02-26', '2024-01-01')} UNION ALL BY NAME "
                                 f"SELECT * FROM ts_forecast_by('s', id, ds, y, 'AutoETS', 14, '1d', {FC})) p WHERE id = 'B'",
    "forecast_agg_diagnostics": "SELECT anofox_plot_diagnostics(o) FROM (SELECT unnest(generate_series(1, len(f.insample_fitted))) AS row_id, "
                                "unnest(f.insample_fitted) AS fitted, unnest(f.residuals) AS residual "
                                f"FROM (SELECT ts_forecast_agg(ds, y, 'AutoETS', 14, {FC}) AS f FROM s WHERE id = 'C')) o",
    "metrics_two_models": "SELECT anofox_plot_summary(m) FROM (UNPIVOT (SELECT split_part(model_name, '(', 1) AS model_id, "
                          "ts_mae(list(y ORDER BY ds), list(yhat ORDER BY ds)) AS mae, "
                          "ts_rmse(list(y ORDER BY ds), list(yhat ORDER BY ds)) AS rmse, "
                          "ts_smape(list(y ORDER BY ds), list(yhat ORDER BY ds)) AS smape, "
                          "ts_coverage(list(y ORDER BY ds), list(yhat_lower ORDER BY ds), list(yhat_upper ORDER BY ds)) AS coverage "
                          "FROM bt GROUP BY 1) ON mae, rmse, smape, coverage INTO NAME metric VALUE value) m",
    "tests_struct_column": "SELECT anofox_plot_tests(r) FROM ts_ljung_box_by('s', id, ds, y) r",
    # the #190 repro: two tests' statistics → a panel per method …
    "tests_two_methods": "SELECT anofox_plot_tests(t) FROM (SELECT id, 'ADF' AS method, (adf).statistic AS statistic, "
                         "(adf).p_value AS p_value FROM ts_adf_by('s', id, ds, y) UNION ALL "
                         "SELECT id, 'KPSS', (kpss).statistic, (kpss).p_value FROM ts_kpss_by('s', id, ds, y)) t",
    # … and on −log10(p), one scale for every test, one panel
    "tests_two_methods_p": "SELECT anofox_plot_tests(t, value := 'p') FROM (SELECT id, 'ADF' AS method, "
                           "(adf).statistic AS statistic, (adf).p_value AS p_value FROM ts_adf_by('s', id, ds, y) "
                           "UNION ALL SELECT id, 'KPSS', (kpss).statistic, (kpss).p_value "
                           "FROM ts_kpss_by('s', id, ds, y)) t",
    # the long form, with the real dates back (#188): the same UNPIVOT
    # adapter as before, but into the decomposition schema's `value`.
    "mstl_long_with_dates": "SELECT anofox_plot_decomposition(t) FROM (UNPIVOT (SELECT m.id, unnest(h.dss) AS ds, "
                            "unnest(m.trend) AS trend, unnest(m.seasonal[1]) AS seasonal, unnest(m.remainder) AS remainder "
                            "FROM ts_mstl_decomposition_by('s', id, ds, y, MAP{'seasonal_periods': '[7]'}) m "
                            "JOIN (SELECT id, list(ds ORDER BY ds) AS dss FROM s GROUP BY id) h USING (id) WHERE m.id = 'B') "
                            "ON trend, seasonal, remainder INTO NAME component VALUE value) t",
    "mstl_per_series": "SELECT id, anofox_plot_decomposition(m) FROM "
                       "ts_mstl_decomposition_by('s', id, ds, y, MAP{'seasonal_periods': '[7]'}) m GROUP BY id ORDER BY id",
}

# Build warnings the harness accepts (none today).
ALLOWED_WARNINGS: list[str] = []

# Cases whose plot must have this many grid panels (`data-panel`, 0 = one
# plot): several tests' statistics get a panel per method (#190).
PANELS: dict[str, int] = {
    "ts_stationarity_by": 2,          # ADF, KPSS
    "ts_residual_diagnostics_by": 3,  # Ljung-Box, Jarque-Bera, Durbin-Watson (no p)
    "tests_two_methods": 2,
    "tests_two_methods_p": 0,
}

# Producer bugs of a released anofox_forecast build (by extension_version),
# reported XFAIL instead of failing; an XFAIL case that passes says XPASS.
KNOWN: dict[str, dict[str, str]] = {
    "e7f3138": {
        "ts_detect_periods_by": "returns NULL periods for every series (fixed on main)",
        "ts_classify_seasonality_by": "seasonal_strength / timing_variability are NaN (fixed on main)",
        "ts_classify_seasonality_agg": "seasonal_strength / timing_variability are NaN (fixed on main)",
        "ts_detect_changepoints_agg": "flags no changepoint though changepoint_probability reaches 0.996 (fixed on main)",
        "forecast_agg_diagnostics": "ts_forecast_agg has no insample_fitted / residuals yet",
    },
}


def run(duckdb: str, ext: str, sql: str, pre: str, mode: str = "-list") -> tuple[bool, str]:
    return run_duckdb(duckdb, f"{pre}LOAD anofox_forecast; SET anofox_telemetry_enabled = false; "
                               f"LOAD '{ext}';\n{sql};", mode, timeout=600)


def check(ok: bool, out: str) -> tuple[str, list[str], str]:
    return _check(ok, out, ALLOWED_WARNINGS, no_data=True)


def main() -> int:
    a = parser(__doc__, "forecast-conformance",
               extension_directory="DuckDB extension directory holding anofox_forecast").parse_args()
    ext = ext_path(a.ext)
    if ext is None:
        return 2
    os.makedirs(a.out, exist_ok=True)
    pre = f"SET extension_directory = '{a.extension_directory}'; " if a.extension_directory else ""

    ok, out = run(a.duckdb, ext, "SELECT 'version=' || anofox_contract_version(); "
                  "SELECT 'forecast=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_forecast'; "
                  "SELECT 'visualization=' || extension_version FROM duckdb_extensions() WHERE extension_name = 'anofox_visualization'; "
                  "SELECT 'dated=' || list_contains(parameters, 'date_col') FROM duckdb_functions() "
                  "WHERE function_name = 'ts_conformal_by' LIMIT 1", pre)
    print(out.strip())
    if not ok or "version=1" not in out:
        print("FAIL: anofox_contract_version() must be '1'")
        return 1
    dated = "dated=true" in out
    version = m.group(1) if (m := re.search(r"forecast=(\S+)", out)) else ""
    known = KNOWN.get(version, {})

    # Every ts_* table macro / table function / aggregate (the anofox_fcst_*
    # names are aliases), with its contract tag if the build sets one.
    ok, out = run(a.duckdb, ext, "SELECT DISTINCT function_name || '|' || coalesce(tags['anofox.output'], '') "
                  "FROM duckdb_functions() WHERE function_name LIKE 'ts\\_%' ESCAPE '\\' "
                  "AND function_type IN ('table', 'table_macro', 'aggregate') ORDER BY 1", pre)
    found = dict(l.split("|", 1) for l in out.splitlines() if "|" in l)
    if not ok or not found:
        print("FAIL: cannot list the anofox_forecast functions\n" + out)
        return 1
    reg = producers(dated)
    setup = SETUP + (SETUP_DATED if dated else "")
    missing = [f for f in found if f not in reg and (f not in OUT_OF_SCOPE or found[f])]
    absent = [f for f in reg if f not in found]
    print(f"{len(found)} ts_* functions: {len(reg) - len(absent)} producers, "
          f"{sum(f in OUT_OF_SCOPE for f in found)} out of scope, {len(missing)} unclassified "
          f"({'dated' if dated else 'legacy'} conformal API)")

    runs = []
    for name, p in reg.items():
        if name in absent:
            continue
        for which, call in (("auto", "anofox_plot(t)"), ("specific", p["specific"] or SPECIFIC[p["schema"]])):
            runs.append({"name": name, "which": which, "schema": p["schema"], "kind": p["kind"],
                         "sql": f"{setup}\nSELECT {call} FROM {p['src']} t"})
    for k, sql in EXTRA.items():
        runs.append({"name": k, "which": "extra", "schema": "", "kind": "", "sql": f"{setup}\n{sql}"})
    if a.only:
        runs = [c for c in runs if re.search(a.only, c["name"] + "." + c["which"])]

    def one(c):
        ok, out = run(a.duckdb, ext, c["sql"], pre, "-json")
        status, svgs, msg = check(ok, out)
        want = PANELS.get(c["name"])
        if status == "OK" and want is not None:
            got = sum(s.count('data-panel="') for s in svgs)
            if got != want:
                status, msg = "FAIL", f"{got} panels, expected {want} (a panel per test method, #190)"
        return c, (status, svgs, msg)

    post = lambda c, s, m: known_bug(s, m, known[c["name"]]) if c["name"] in known else (s, m)
    results = run_all(runs, one, a.jobs, a.out, post=post)
    names = svg_names(results)
    for key, status, msg, n, c in results:
        tag = f"{c['schema']}/{c['kind']}" if c["schema"] else "extra"
        print(f"{status:5s} {key:44s} {tag:20s} {msg}")
    for m in missing:
        why = f"tagged anofox.output = {found[m]}" if found[m] else "neither a producer nor out of scope"
        print(f"MISS {m}: {why} — add a recipe or an OUT_OF_SCOPE reason")
    for m in absent:
        print(f"SKIP {m}: not in this anofox_forecast build")
    passed = sum(1 for r in results if r[1] in ("OK", "XFAIL", "XPASS"))
    xfail = sum(1 for r in results if r[1] == "XFAIL")
    covered = sorted({r[4]["name"] for r in results if r[4]["schema"]})
    direct = sum(1 for n in covered if reg[n]["kind"] == "direct")
    print(f"\n{len(covered)} producers covered ({direct} direct, {len(covered) - direct} through an adapter), "
          f"{passed}/{len(results)} plot calls OK" + (f" ({xfail} known producer bugs of {version})" if xfail else ""))
    keep = lambda n: not n.endswith(".auto")
    if not a.no_montage:
        write_montage(a.out, names, keep)
    return 0 if passed == len(results) and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
