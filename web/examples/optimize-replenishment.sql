-- Forecast → optimize: plan inventory from a forecast (anofox-forecast + anofox-optimize).
--
-- STORED RESULTS, NOT LIVE. Both steps ran natively
-- (examples/optimize-replenishment/compute.sql, written to
-- web/data/optimize-replenishment/*.csv by scripts/gen-optimize-cases.sh
-- replenishment): anofox-forecast forecasts weekly demand per SKU
-- (MFLES, with a backtest), anofox-optimize chooses which
-- pallets to buy under the week's budget (opt_knapsack_*). anofox_optimize's
-- DuckDB-Wasm build doesn't load yet (see examples/optimize-showcase/README.md),
-- so this dashboard only reads the stored results: the inputs pick among stored
-- budgets, service levels and rules; nothing is re-solved in the browser.
--
-- The question: what do we order this week, with a limited purchasing budget,
-- so we sell the most over the next three weeks? Asked by the category buyer
-- of a cash & carry wholesaler on Monday morning.
--
-- Tabs: Forecast · Accuracy · Order plan · Budget · Decision.
-- Natively, run from web/:  dashboard --check ../examples/optimize-replenishment/showcase.sql

-- ── Look ─────────────────────────────────────────────────────────────────────
SELECT 'anofox-forecast + anofox-optimize · stored results'::BRAND;
SELECT 'What do we order this week, within budget, to sell the most over the next three weeks?'::DASHBOARD_TITLE;

-- ── Data: the stored results (CSV) ───────────────────────────────────────────
CREATE OR REPLACE TABLE ir_skus AS FROM read_csv('data/optimize-replenishment/ir_skus.csv');
CREATE OR REPLACE TABLE ir_sales AS FROM read_csv('data/optimize-replenishment/ir_sales.csv');
CREATE OR REPLACE TABLE ir_forecast AS FROM read_csv('data/optimize-replenishment/ir_forecast.csv');
CREATE OR REPLACE TABLE ir_backtest AS FROM read_csv('data/optimize-replenishment/ir_backtest.csv');
CREATE OR REPLACE TABLE ir_need AS FROM read_csv('data/optimize-replenishment/ir_need.csv');
CREATE OR REPLACE TABLE ir_plan AS FROM read_csv('data/optimize-replenishment/ir_plan.csv');
CREATE OR REPLACE TABLE ir_summary AS FROM read_csv('data/optimize-replenishment/ir_summary.csv');
CREATE OR REPLACE TABLE ir_provenance AS FROM read_csv('data/optimize-replenishment/provenance.csv');

-- The inputs (each snaps to a stored sweep point) and helpers.
CREATE OR REPLACE MACRO ir_budget() AS
    least(60000, greatest(10000, round(coalesce(TRY_CAST(getvariable('budget_eur') AS DOUBLE), 25000) / 2500) * 2500))::INTEGER;
CREATE OR REPLACE MACRO ir_sl() AS coalesce(getvariable('service_level'), '95 %');
CREATE OR REPLACE MACRO ir_rule() AS coalesce(getvariable('rule'), 'forecast, exact (proven optimum)');
CREATE OR REPLACE MACRO ir_today() AS 'today: last 4 weeks, top sellers first';
CREATE OR REPLACE MACRO ir_sku() AS coalesce(getvariable('sku'), 'Mulled wine 1 l');
CREATE OR REPLACE MACRO ir_short(r) AS
    CASE WHEN r = ir_today() THEN 'today' WHEN r LIKE '%exact%' THEN 'forecast + exact' ELSE 'forecast + top sellers' END;
CREATE OR REPLACE MACRO ir_sum() AS TABLE
    SELECT * FROM ir_summary WHERE service_level = ir_sl() AND budget = ir_budget();
CREATE OR REPLACE MACRO ir_pl() AS TABLE
    SELECT p.*, s.name, s.category FROM ir_plan p JOIN ir_skus s USING (sku)
    WHERE p.service_level = ir_sl() AND p.budget = ir_budget();

-- ── Header: the question, who asks it, inputs, KPIs ──────────────────────────
SELECT 12::COL;
SELECT ('**The category buyer** of a cash & carry wholesaler places the weekly order on Monday 19 Oct 2026 for 12 SKUs. Goods arrive in two weeks and the next order goes out in one, so this order must cover **three weeks** — while summer lines fade and Christmas lines ramp up. Today the buyer orders three times the last four weeks'' sales and, when the budget runs short, funds the top sellers first. '
        || 'Here **two anofox extensions** work in sequence: anofox-forecast forecasts each SKU (MFLES), then anofox-optimize (`opt_knapsack_exact`) picks the pallets that earn the most expected margin within the budget. '
        || '**Stored results:** computed natively with ' || (SELECT string_agg(extension_name || ' `' || extension_version || '`', ' and ' ORDER BY extension_name) FROM ir_provenance)
        || ' on DuckDB ' || (SELECT any_value(duckdb_version) FROM ir_provenance) || ' (`examples/optimize-replenishment/compute.sql`); the inputs pick among stored runs, nothing is re-solved in your browser.')::MARKDOWN;

SELECT 'Planning inputs'::GROUP;
SELECT 25000 AS budget_eur ::NUMBER, 'Purchasing budget this week (€, 10,000–60,000)'::LABEL;
SELECT service_level ::DROPDOWN, 'Target service level'::LABEL FROM (VALUES (1, '95 %'), (2, '90 %'), (3, '98 %')) t(k, service_level) ORDER BY k;
SELECT rule ::DROPDOWN, 'Plan (vs. today''s rule)'::LABEL FROM (SELECT DISTINCT rule, ord FROM ir_summary WHERE rule <> ir_today()) ORDER BY ord DESC;
SELECT name AS sku ::DROPDOWN, 'SKU to drill into'::LABEL FROM ir_skus ORDER BY name = 'Mulled wine 1 l' DESC, name;
SELECT 1::ENDGROUP;

SELECT 'Selected plan vs. today''s rule at the chosen budget (arrow = change from today; up is better)'::GROUP;
SELECT (SELECT fill_rate_pct FROM ir_sum() WHERE rule = ir_rule())::PERCENT,
       (SELECT fill_rate_pct FROM ir_sum() WHERE rule = ir_today())::DELTA,
       'Expected demand served, next 3 weeks'::LABEL;
SELECT (SELECT exp_margin FROM ir_sum() WHERE rule = ir_rule())::METRIC,
       (SELECT exp_margin FROM ir_sum() WHERE rule = ir_today())::DELTA,
       'Expected margin, next 3 weeks (€)'::LABEL;
SELECT ((SELECT exp_lost_units FROM ir_sum() WHERE rule = ir_today()) - (SELECT exp_lost_units FROM ir_sum() WHERE rule = ir_rule()))::METRIC,
       'Lost sales avoided (units)'::LABEL;
SELECT ((SELECT exp_overstock_eur FROM ir_sum() WHERE rule = ir_today()) - (SELECT exp_overstock_eur FROM ir_sum() WHERE rule = ir_rule()))::METRIC,
       'Less stock left after 3 weeks (€)'::LABEL;
SELECT 1::ENDGROUP;

-- ── 1 · Forecast (anofox-forecast) ────────────────────────────────────────────
SELECT 'Forecast'::TAB;

SELECT 12::COL;
SELECT ('**Step 1, anofox-forecast.** One grouped forecast call (MFLES, weekly, seasonal period 52, see `compute.sql`) forecasts the next 8 weeks of every SKU from 104 weeks of sales. The band is the 90 % planning band from the backtest error (tab *Accuracy*). For **' || ir_sku() || '** the next three weeks need **' ||
        (SELECT round(sum(yhat))::INTEGER FROM ir_forecast f JOIN ir_skus s USING (sku) WHERE s.name = ir_sku() AND f.step <= 3) || '** units; three times the last four weeks says **' ||
        (SELECT round(3 * last4_avg)::INTEGER FROM ir_skus WHERE name = ir_sku()) || '**.')::MARKDOWN;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT week ::XAXIS, kind ::CATEGORY, units ::LINECHART, lo ::BAND_LOWER, hi ::BAND_UPPER,
       (ir_sku() || ': a year of weekly sales and the 8-week forecast with its 90 % band')::TITLE
FROM (
    SELECT x.week, 'sales' AS kind, x.units, x.units AS lo, x.units AS hi
    FROM ir_sales x JOIN ir_skus s USING (sku) WHERE s.name = ir_sku() AND x.week >= DATE '2025-10-13'
    UNION ALL
    SELECT f.week, 'forecast', f.yhat, f.band_lower, f.band_upper
    FROM ir_forecast f JOIN ir_skus s USING (sku) WHERE s.name = ir_sku()
) ORDER BY kind, week;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT name ::XAXIS, src ::CATEGORY, units ::BARCHART, 1 ::FLIP,
       'Demand for the 3 covered weeks: forecast vs. 3 × the last four weeks — winter lines need far more, summer lines far less'::TITLE
FROM (
    SELECT s.name, 'forecast (next 3 weeks)' AS src, round(n.mu_cover) AS units, n.mu_cover AS k
    FROM ir_need n JOIN ir_skus s USING (sku) WHERE n.service_level = ir_sl()
    UNION ALL
    SELECT s.name, 'today: 3 × last 4 weeks', round(3 * s.last4_avg), n.mu_cover
    FROM ir_need n JOIN ir_skus s USING (sku) WHERE n.service_level = ir_sl()
) ORDER BY k, src DESC;

-- ── 2 · Accuracy (backtest) ──────────────────────────────────────────────────
SELECT 'Accuracy'::TAB;

SELECT 12::COL;
SELECT '**Can we trust it?** The same model, trained on the data up to 17 Aug 2026, forecast the following 8 weeks; today''s rule used the average of the four weeks before. On the steady lines both are fine; on the seasonal ones the 4-week average misses by a multiple. The forecast''s error per SKU sets the safety stock.'::MARKDOWN;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT week ::XAXIS, kind ::CATEGORY, units ::LINECHART,
       (ir_sku() || ': backtest weeks — actual sales, forecast, 4-week average')::TITLE
FROM (
    SELECT b.week, 'actual' AS kind, b.actual AS units FROM ir_backtest b JOIN ir_skus s USING (sku) WHERE s.name = ir_sku()
    UNION ALL SELECT b.week, 'forecast', b.forecast FROM ir_backtest b JOIN ir_skus s USING (sku) WHERE s.name = ir_sku()
    UNION ALL SELECT b.week, 'today: 4-week average', b.naive FROM ir_backtest b JOIN ir_skus s USING (sku) WHERE s.name = ir_sku()
) ORDER BY kind, week;

SELECT 6::COL;
SELECT 'Forecast error: bottom 3'::HIGHLIGHT;
SELECT 'Today''s error > 50'::FLAG;
SELECT 'mean'::TOTALS;
SELECT s.name AS "SKU" ::TABLE, s.category AS "Category",
       round(100 * avg(abs(b.actual - b.forecast)) / avg(b.actual), 1) AS "Forecast error" ::COLORSCALE,
       round(100 * avg(abs(b.actual - b.naive)) / avg(b.actual), 1) AS "Today's error" ::COLORSCALE,
       list(b.actual ORDER BY b.week) AS "Actual, 8 weeks" ::SPARKLINE,
       'Backtest error per SKU (% of actual, lower is better)'::TITLE
FROM ir_backtest b JOIN ir_skus s USING (sku) GROUP BY ALL ORDER BY "Today's error" DESC;

-- ── 3 · Order plan (anofox-optimize) ──────────────────────────────────────────
SELECT 'Order plan'::TAB;

SELECT 12::COL;
SELECT ('**Step 2, anofox-optimize.** Each SKU''s need (forecast for 3 weeks + safety stock at ' || ir_sl() || ' − stock on hand and on order) is cut into whole pallets; each pallet is worth the margin it is expected to sell. With **€' || ir_budget() || '** not every pallet fits: `opt_knapsack_exact` picks the set worth the most; today''s rule funds the top sellers first — on last month''s numbers.')::MARKDOWN;

SELECT 12::COL;
SELECT 400::HEIGHT;
SELECT name ::XAXIS, CASE WHEN rule = ir_today() THEN 'today' ELSE 'selected plan' END ::CATEGORY, pallets ::BARCHART, 1 ::FLIP,
       ('Pallets ordered per SKU at €' || ir_budget() || ' — the plan moves money to the lines that will sell')::TITLE
FROM ir_pl() WHERE rule IN (ir_today(), ir_rule()) ORDER BY max(pallets_needed) OVER (PARTITION BY sku), name, 2 DESC;

SELECT 12::COL;
SELECT 'category'::SUBTOTAL;
SELECT 'Fill rate %: mean, Fill rate today %: mean'::TOTALS;
SELECT 'Fill rate % < 90'::FLAG;
SELECT 'Pallets: pallets_needed, pallets_today, pallets_selected | Expected over 3 weeks: Fill rate today %, Fill rate %, lost_units'::COLGROUP;
SELECT s.name AS "SKU" ::TABLE, s.category, s.pallets_needed, t.pallets AS pallets_today, s.pallets AS pallets_selected,
       s.cost AS "Cost (€)", t.fill_rate_pct AS "Fill rate today %", s.fill_rate_pct AS "Fill rate %", s.exp_lost_units AS lost_units,
       ('Per SKU at €' || ir_budget() || ', ' || ir_sl() || ' service level')::TITLE
FROM ir_pl() s JOIN ir_pl() t ON t.sku = s.sku AND t.rule = ir_today()
WHERE s.rule = ir_rule()
ORDER BY s.category, s.name;

-- ── 4 · Budget ───────────────────────────────────────────────────────────────
SELECT 'Budget'::TAB;

SELECT 12::COL;
SELECT '**How much budget does the order need?** Every rule was solved for budgets from €10,000 to €60,000 in €2,500 steps. The forecast-based plans reach full service with less money; the exact knapsack matters most when the budget is tight.'::MARKDOWN;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT budget ::XAXIS, ir_short(rule) ::CATEGORY, fill_rate_pct ::LINECHART, ir_budget() ::XLINE, '€'::XFORMAT,
       'Expected demand served (%) by budget (€)'::TITLE
FROM ir_summary WHERE service_level = ir_sl() ORDER BY rule, budget;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT budget ::XAXIS, ir_short(rule) ::CATEGORY, exp_overstock_eur ::LINECHART, ir_budget() ::XLINE, '€'::XFORMAT, '€'::YFORMAT,
       'Stock expected to be left after 3 weeks (€) — at tight budgets today''s rule ties money up in summer lines'::TITLE
FROM ir_summary WHERE service_level = ir_sl() ORDER BY rule, budget;

SELECT 12::COL;
SELECT 220::HEIGHT;
SELECT '€' || (budget / 1000) || 'k' ::XAXIS, service_level ::YAXIS, fill_rate_pct ::HEATMAP, ''::DATALABELS,
       'Demand served (%) by budget and service level, selected plan'::TITLE
FROM ir_summary WHERE rule = ir_rule() AND budget % 5000 = 0 ORDER BY service_level, budget;

-- ── 5 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

-- The smallest stored budget at which the selected plan serves ≥ 99 % of demand.
CREATE OR REPLACE MACRO ir_enough() AS (
    SELECT min(budget) FROM ir_summary WHERE service_level = ir_sl() AND rule = ir_rule() AND fill_rate_pct >= 99);

SELECT 12::COL;
SELECT ('### Recommendation' || chr(10) || chr(10) ||
        '**Place the ' || ir_rule() || ' order** below: with **€' || ir_budget() || '** it serves an expected **' || s.fill_rate_pct || ' %** of the next three weeks'' demand, against **' || t.fill_rate_pct ||
        ' %** with today''s rule, and leaves **€' || round(t.exp_overstock_eur - s.exp_overstock_eur)::INTEGER || '** less stock on the shelf. ' ||
        'The plan buys more ' || coalesce((SELECT string_agg(name, ', ' ORDER BY name) FROM ir_pl() a JOIN ir_pl() b USING (sku, name) WHERE a.rule = ir_rule() AND b.rule = ir_today() AND a.pallets > b.pallets), 'of nothing') ||
        ' and less ' || coalesce((SELECT string_agg(name, ', ' ORDER BY name) FROM ir_pl() a JOIN ir_pl() b USING (sku, name) WHERE a.rule = ir_rule() AND b.rule = ir_today() AND a.pallets < b.pallets), 'of nothing') || '.' || chr(10) || chr(10) ||
        CASE WHEN ir_enough() IS NULL THEN '**No stored budget** reaches 99 % service at ' || ir_sl() || '.'
             WHEN ir_enough() <= ir_budget() THEN 'The budget is enough: from **€' || ir_enough() || '** the plan already serves 99 % of demand.'
             ELSE 'To serve 99 % of demand the order needs **€' || ir_enough() || '** — **€' || (ir_enough() - ir_budget()) || '** more than this week''s budget; worth asking for.' END)::MARKDOWN
FROM ir_sum() s, ir_sum() t WHERE s.rule = ir_rule() AND t.rule = ir_today();

SELECT 12::COL;
SELECT 'Pallets: sum, Units: sum, Cost (€): sum, Expected fill rate %: mean'::TOTALS;
SELECT 'Cost (€): top 3'::HIGHLIGHT;
SELECT name AS "SKU" ::TABLE, category AS "Category", pallets AS "Pallets", units AS "Units", cost AS "Cost (€)", fill_rate_pct AS "Expected fill rate %",
       ('The order to place: ' || ir_rule() || ', €' || ir_budget())::TITLE
FROM ir_pl() WHERE rule = ir_rule() AND pallets > 0 ORDER BY cost DESC;
