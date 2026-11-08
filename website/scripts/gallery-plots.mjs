// The Gallery's extension pages: which plots of a conformance run
// (scripts/<ext>_conformance.py --out) are shown, under which name.
// `from` is the run's file, `file` the committed copy, `call` the macro and
// the producer it plots.
const p = (from, title, text, call) => ({ from, file: from.replace(/\[|\]/g, "-").replace(/\.(auto|specific|extra|generic|view)(\.\d)?\.svg$/, "$2.svg").replace(/^producer\.|^statistics\./, ""), title, text, call });

export const plots = {
  statistics: [
    p("terms_two_models.extra.svg", "Coefficients of two models", "A coefficient forest with 95 % intervals; two estimators dodged, a term whose interval crosses 0 fades.", "anofox_plot_terms · ols_tidy_by ∪ huber_tidy_by"),
    p("prediction_two_models.extra.svg", "Fit and prediction band", "Observed points, each model's fit and the reference model's prediction band.", "anofox_plot_prediction · ols_prediction_by ∪ huber_prediction_by"),
    p("ols_augment_by.specific.svg", "Regression diagnostics", "The plot.lm 2×2 in one picture: residuals, normal Q-Q, scale-location, leverage.", "anofox_plot_diagnostics · ols_augment_by"),
    p("kaplan_meier_by.specific.svg", "Survival curves", "Kaplan–Meier steps with bands, censor marks, the median line and a number-at-risk table.", "anofox_plot_curve · kaplan_meier_by"),
    p("ridge_path_by.specific.svg", "Regularisation path", "Every coefficient along λ on a log axis, labelled at its line end.", "anofox_plot_terms · ridge_path_by"),
    p("elasticnet_cv_by.specific.svg", "Cross-validated λ", "CV error ± one SE with λ.min and λ.1se marked.", "anofox_plot_curve · elasticnet_cv_by"),
    p("quantile_process_by.specific.svg", "Quantile process", "A coefficient across quantiles τ with its band, one panel per term and group.", "anofox_plot_terms · quantile_process_by"),
    p("logistic_effects_by.specific.svg", "Effects on the probability", "P(y = 1) along a focal variable with its interval, one panel per group.", "anofox_plot_prediction · logistic_effects_by"),
    p("curve_roc_calibration_grouped.extra.0.svg", "ROC curves", "True against false positive rate with the AUC in the legend.", "anofox_plot_curve · roc_curve_by"),
    p("curve_roc_calibration_grouped.extra.1.svg", "Calibration", "Observed rate against predicted probability, dots sized by the bin count.", "anofox_plot_curve · calibration_curve_by"),
    p("compare_box.extra.svg", "Groups with significance brackets", "Box plots per group with brackets from a post-hoc test.", "anofox_plot_compare · tukey_hsd_agg"),
    p("tukey_hsd_by.specific.svg", "Pairwise comparisons", "Differences with 95 % intervals and p-values, one panel per group.", "anofox_plot_tests · tukey_hsd_by"),
    p("summary_two_models.extra.svg", "Model metrics side by side", "One small panel per metric, the models in one order, the best highlighted.", "anofox_plot_summary · *_glance_by"),
    p("partial_component.extra.svg", "Component + residual", "A partial-residual plot per term with its component line and a smoother.", "anofox_plot_partial · ols_partial_residuals_by"),
    p("acf_by.specific.svg", "Autocorrelation", "ACF lollipops with significance bounds, one panel per series.", "anofox_plot_curve · acf_by"),
    p("obs_leverage.extra.svg", "Leverage and outliers", "Standardised residuals against leverage, the most influential rows labelled.", "anofox_plot_obs · ols_augment_by"),
  ],
  forecast: [
    p("forecast_fan_levels.extra.svg", "A forecast fan", "History, the forecast and nested 80 % / 95 % intervals from the forecast start.", "anofox_plot_prediction · ts_forecast_by"),
    p("forecast_models.extra.svg", "Several methods", "One line per method beside the reference method's band.", "anofox_plot_prediction · ts_forecast_by × 3"),
    p("forecast_with_history.extra.svg", "One panel per series", "Forecasts for every series of a panel, each with its history.", "anofox_plot_prediction · ts_forecast_by"),
    p("backtest_two_models.extra.svg", "Backtest", "Two methods' backtest forecasts against the actuals.", "anofox_plot_prediction · ts_cv_forecast_by"),
    p("changepoints_over_history.extra.svg", "Changepoints", "Detected changepoints as events over the history and the forecast.", "anofox_plot_prediction · ts_detect_changepoints_by"),
    p("mstl_long_with_dates.extra.svg", "Decomposition", "Trend, seasonal and remainder as stacked panels on one time axis.", "anofox_plot_decomposition · ts_mstl_decomposition_long_by"),
    p("cv_folds_per_series.extra.0.svg", "Cross-validation folds", "Train and test windows per fold.", "anofox_plot_cv · ts_cv_folds_by"),
    p("ts_conformal_by.specific.svg", "Conformal intervals", "Forecasts with calibrated conformal bands, one panel per series.", "anofox_plot_prediction · ts_conformal_by"),
    p("ts_forecast_explain_by.specific.svg", "What the model sees", "Observed, level, trend, seasonal and residual of a fitted model.", "anofox_plot_decomposition · ts_forecast_explain_by"),
    p("ts_metrics_by.specific.svg", "Accuracy metrics", "RMSE, MAE, MAPE, bias and more per series, one panel per metric.", "anofox_plot_summary · ts_metrics_by"),
    p("tests_two_methods.extra.svg", "Stationarity tests", "ADF and KPSS per series, each on its own scale, coloured by significance.", "anofox_plot_tests · ts_adf_by ∪ ts_kpss_by"),
    p("ts_residual_diagnostics_by.specific.svg", "Residual diagnostics", "Durbin–Watson, Jarque–Bera and Ljung–Box per series.", "anofox_plot_tests · ts_residual_diagnostics_by"),
  ],
  tabular: [
    p("checks_board.extra.svg", "A data-quality board", "One panel per check: the value against its acceptance band, coloured by status.", "anofox_plot_checks · anofox_tab_run_checks"),
    p("null_rate.specific.svg", "One check", "A single check's value against its threshold.", "anofox_plot_checks · null_rate"),
    p("checks_side_by_side.extra.svg", "Checks compared", "Several one-row checks as one summary.", "anofox_plot_summary · compliance, match_rate, null_rate, …"),
    p("profile_correlations.specific.svg", "Correlation matrix", "A profile's correlations on a diverging scale, values in the cells.", "anofox_plot_matrix · anofox_tab_profile_correlations"),
    p("profile_summary.specific.svg", "Table profile", "Row and column counts, null rates and duplicates at a glance.", "anofox_plot_summary · anofox_tab_profile_summary"),
    p("isolation_forest[scores].generic.svg", "Anomaly scores", "Isolation-forest scores per row, anomalies coloured.", "anofox_render · anofox_tab_isolation_forest"),
    p("dbscan[clusters].generic.svg", "Clusters and noise", "DBSCAN core points and noise per cluster.", "anofox_render · anofox_tab_dbscan"),
    p("pii_scan_table.generic.svg", "Personal data found", "PII matches per column and kind (e-mail, phone, …).", "anofox_render · anofox_tab_pii_scan_table"),
  ],
  optimize: [
    p("pack_best_of.view.svg", "Bin packing", "Items stacked per container against the capacity line, drawn with roles from the solver's STRUCT.", "anofox_render (stacked bars) · anofox_optimize_pack_best_of"),
    p("wave_best_of.view.svg", "Pick waves", "Order lines per wave, coloured by priority, against the wave size.", "anofox_render (stacked bars) · anofox_optimize_wave_best_of"),
    p("schedule_best_of.view.svg", "A schedule", "Tasks per resource as a Gantt chart, late tasks red with a connector to the due date.", "anofox_plot_schedule · anofox_optimize_schedule_best_of"),
    p("sequence_best_of.view.svg", "A route", "The optimised visiting order drawn on a map.", "anofox_render (::MAP) · anofox_optimize_sequence_best_of"),
    p("knapsack_best_of.view.svg", "What fits the carrier", "Selected shipments against the ones left out.", "anofox_render (bars) · anofox_optimize_knapsack_best_of"),
    p("assortment_best_of.view.svg", "Which products to list", "Listed against unlisted products by expected margin.", "anofox_render (bars) · anofox_optimize_assortment_best_of"),
    p("portfolio_best_of.view.svg", "Portfolio weights", "The weight per asset of the optimised portfolio.", "anofox_render (bars) · anofox_optimize_portfolio_best_of"),
    p("schedule_compare.extra.svg", "Algorithms compared", "Makespan and weighted tardiness for each scheduling rule.", "anofox_plot_summary · six anofox_optimize_schedule_* rules"),
  ],
  decide: [
    p("producer.decide_roc_curve_by.specific.svg", "ROC per model", "Two decision models' ROC curves with their AUC.", "anofox_plot_curve · decide_roc_curve_by"),
    p("producer.decide_calibration_curve_by.specific.svg", "Calibration per model", "Are the model's probabilities honest? Observed against predicted, with intervals.", "anofox_plot_curve · decide_calibration_curve_by"),
    p("pr_base_rate.extra.svg", "Precision and recall", "Precision against recall with the base rate as a dashed line.", "anofox_plot_curve · pr_curve_by"),
    p("summary_metrics.extra.svg", "Brier score and ECE", "Quality metrics per model, one panel each.", "anofox_plot_summary · decide_metrics_by"),
    p("choice_distribution.specific.extra.svg", "A choice distribution", "The probability of each option a model gives for one text.", "anofox_plot_distribution · decide_choice_distribution"),
  ],
};
