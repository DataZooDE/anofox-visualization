-- Forecast → optimize: plan inventory from a forecast — the native computation
-- behind the stored results of web/examples/optimize-replenishment.sql.
--
-- Uses two anofox extensions natively: anofox_forecast forecasts weekly demand
-- per SKU (ts_forecast_by, with 90 % prediction intervals, plus a backtest),
-- anofox_optimize picks which pallets to order under the week's purchasing
-- budget (opt_knapsack_*). anofox_optimize has no loadable DuckDB-Wasm build
-- yet, so the browser demo reads the tables this script produces from
-- web/data/optimize-replenishment/*.csv. Regenerate them with
-- scripts/gen-optimize-cases.sh replenishment.
--
-- A made-up cash & carry wholesaler places its weekly order on Mon 19 Oct
-- 2026 for 12 SKUs. Lead time 2 weeks + review period 1 week: this order
-- has to cover the next 3 weeks. Summer lines fade, winter lines ramp up —
-- exactly where "order what we sold last month" goes wrong. Every input is
-- synthetic and deterministic (hash-based noise, no files).
LOAD anofox_forecast;
LOAD anofox_optimize;

CREATE OR REPLACE MACRO ir_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
-- Standard normal pdf / cdf (Abramowitz & Stegun 26.2.17, |error| < 7.5e-8).
CREATE OR REPLACE MACRO ir_phi(z) AS exp(-z * z / 2) / sqrt(2 * pi());
CREATE OR REPLACE MACRO ir_cdf_pos(z) AS
    1 - ir_phi(z) * ((((1.330274429 / (1 + 0.2316419 * z) - 1.821255978) / (1 + 0.2316419 * z) + 1.781477937)
                     / (1 + 0.2316419 * z) - 0.356563782) / (1 + 0.2316419 * z) + 0.319381530) / (1 + 0.2316419 * z);
CREATE OR REPLACE MACRO ir_cdf(z) AS CASE WHEN z >= 0 THEN ir_cdf_pos(z) ELSE 1 - ir_cdf_pos(-z) END;
-- Expected sales E[min(D, x)] for D ~ Normal(mu, sigma).
CREATE OR REPLACE MACRO ir_esales(mu, sigma, x) AS
    CASE WHEN x <= 0 THEN 0.0
         ELSE mu - sigma * (ir_phi((x - mu) / sigma) - ((x - mu) / sigma) * (1 - ir_cdf((x - mu) / sigma))) END;

-- ── SKUs ─────────────────────────────────────────────────────────────────────
-- season: peak week of the year (0 = none), amp: seasonal swing.
CREATE OR REPLACE TABLE ir_skus AS
SELECT * FROM (VALUES
    ('S01', 'Charcoal 10 kg',        'garden & BBQ',  9.80, 4.10,  60, 160, 27, 0.85),
    ('S02', 'Ice tea 1.5 l (6-pack)', 'drinks',        4.20, 1.60, 120, 260, 29, 0.60),
    ('S03', 'Sparkling water (crate)', 'drinks',       3.10, 1.20, 144, 420,  0, 0.00),
    ('S04', 'Coffee beans 1 kg',     'grocery',      11.50, 4.80, 200, 210,  0, 0.00),
    ('S05', 'De-icing salt 25 kg',   'winter',        6.40, 3.20,  40,  90,  3, 0.90),
    ('S06', 'Mulled wine 1 l',       'drinks',        2.30, 1.40, 300, 340, 50, 0.95),
    ('S07', 'Pasta 5 kg',            'grocery',       5.60, 1.90, 180, 300,  0, 0.00),
    ('S08', 'Olive oil 5 l',         'grocery',      24.00, 7.50,  96, 110,  0, 0.00),
    ('S09', 'Toilet paper (48 rolls)', 'household',  13.20, 3.90,  48, 230,  0, 0.00),
    ('S10', 'Sunscreen 1 l',         'household',     8.70, 5.10, 160,  70, 26, 0.90),
    ('S11', 'Gingerbread 1 kg',      'grocery',       6.10, 3.30, 210, 280, 50, 0.95),
    ('S12', 'Candles (100 pcs)',     'household',    14.50, 6.20,  80, 120, 51, 0.70)
) t(sku, name, category, unit_cost, margin, pallet_units, base, peak_week, amp);

-- ── 104 weeks of sales, Mon 21 Oct 2024 … Mon 12 Oct 2026 ────────────────────
CREATE OR REPLACE MACRO ir_season(peak, amp, wk) AS
    CASE WHEN peak = 0 THEN 1.0
         ELSE 1 - amp + 2 * amp * exp(-((((wk - peak + 26) % 52 + 52) % 52 - 26) ^ 2) / (2 * 5.5 ^ 2)) END;
CREATE OR REPLACE TABLE ir_sales AS
SELECT s.sku, (DATE '2024-10-21' + INTERVAL (7 * w) DAY)::DATE AS week,
       greatest(0, round(s.base * (1 + 0.0015 * w)
                         * ir_season(s.peak_week, s.amp, weekofyear(DATE '2024-10-21' + INTERVAL (7 * w) DAY))
                         * (0.82 + 0.36 * ir_unif(s.sku, w, 'n'))))::DOUBLE AS units
FROM ir_skus s, range(104) t(w);

-- ── Forecast (anofox_forecast) ──────────────────────────────────────────────
-- Backtest first: the model forecasts the last 8 weeks from the data before
-- them, next to today's rule (the average of the last 4 weeks). Its relative
-- error per SKU sets the planning uncertainty below — the model's own
-- intervals assume one constant error for the whole year, far too wide for an
-- off-season line.
CREATE OR REPLACE TABLE ir_train AS FROM ir_sales WHERE week < DATE '2026-08-24';
CREATE OR REPLACE TABLE ir_backtest AS
WITH f AS (
    SELECT sku, ds::DATE AS week, greatest(0, yhat) AS forecast
    FROM ts_forecast_by('ir_train', sku, week, units, 'MFLES', 8, '1w', MAP{'seasonal_period': '52'})
), naive AS (
    SELECT sku, avg(units) AS naive FROM ir_train WHERE week >= DATE '2026-07-27' GROUP BY sku
)
SELECT s.sku, s.week, s.units AS actual, round(f.forecast, 1) AS forecast, round(n.naive, 1) AS naive
FROM ir_sales s JOIN f USING (sku, week) JOIN naive n USING (sku);

CREATE OR REPLACE TABLE ir_error AS
SELECT sku, greatest(0.06, sqrt(avg((actual - forecast) ^ 2)) / avg(actual)) AS cv
FROM ir_backtest GROUP BY sku;

-- The next 8 weeks from all 104 weeks, with a 90 % planning band
-- (yhat ± 1.645 σ, σ = the backtest's relative error × yhat).
CREATE OR REPLACE TABLE ir_forecast AS
SELECT f.sku, f.forecast_step AS step, f.ds::DATE AS week, round(greatest(0, f.yhat), 1) AS yhat,
       round(greatest(0, f.yhat) * e.cv, 2) AS sigma,
       round(greatest(0, f.yhat * (1 - 1.6449 * e.cv)), 1) AS band_lower,
       round(f.yhat * (1 + 1.6449 * e.cv), 1) AS band_upper, f.model_name AS model
FROM ts_forecast_by('ir_sales', sku, week, units, 'MFLES', 8, '1w', MAP{'seasonal_period': '52'}) f
JOIN ir_error e USING (sku);

-- ── Stock position and demand over the 3 covered weeks ───────────────────────
-- on_hand / on_order: about 1.5 weeks of recent sales, with noise.
CREATE OR REPLACE TABLE ir_position AS
SELECT s.sku,
       round((SELECT avg(units) FROM ir_sales x WHERE x.sku = s.sku AND x.week >= DATE '2026-09-21') * (0.6 + 0.9 * ir_unif(s.sku, 'oh', 0))) AS on_hand,
       round((SELECT avg(units) FROM ir_sales x WHERE x.sku = s.sku AND x.week >= DATE '2026-09-21') * (0.3 + 0.7 * ir_unif(s.sku, 'oo', 0))) AS on_order,
       (SELECT avg(units) FROM ir_sales x WHERE x.sku = s.sku AND x.week >= DATE '2026-09-21') AS naive_week,
       (SELECT stddev(units) FROM ir_sales x WHERE x.sku = s.sku AND x.week >= DATE '2026-07-27') AS naive_sd
FROM ir_skus s;

CREATE OR REPLACE TABLE ir_need AS
WITH cover AS (
    SELECT sku, sum(yhat) AS mu, sqrt(sum(sigma * sigma)) AS sigma FROM ir_forecast WHERE step <= 3 GROUP BY sku
)
SELECT c.sku, sl.service_level, sl.z, round(c.mu, 1) AS mu_cover, round(c.sigma, 1) AS sigma_cover,
       p.on_hand, p.on_order,
       round(c.mu + sl.z * c.sigma) AS order_up_to,
       greatest(0, round(c.mu + sl.z * c.sigma) - p.on_hand - p.on_order) AS need_units,
       -- today's rule: 3 × last 4 weeks' average + the same safety factor on the recent spread
       greatest(0, round(3 * p.naive_week + sl.z * sqrt(3) * p.naive_sd) - p.on_hand - p.on_order) AS naive_need_units
FROM cover c JOIN ir_position p USING (sku),
     (VALUES ('90 %', 1.2816), ('95 %', 1.6449), ('98 %', 2.0537)) sl(service_level, z);

-- ── Order lots: whole pallets, valued by the margin they are expected to earn ─
-- Lot k of a SKU is its k-th pallet; its value = margin × the extra units it is
-- expected to sell within the 3 covered weeks (diminishing: the first pallet
-- protects likely sales, the last one only the tail). A SKU gets as many lots
-- as reaching its order-up-to level needs at the service level. basis =
-- 'forecast' values demand with the forecast; 'last 4 weeks' with today's
-- belief (3 × the recent average, its spread), which is what today's rule sees.
CREATE OR REPLACE TABLE ir_lots AS
SELECT b.basis, n.service_level, n.sku, k AS lot, s.pallet_units AS units,
       round(s.pallet_units * s.unit_cost) AS cost,
       round(s.margin * (ir_esales(b.mu, b.sd, n.on_hand + n.on_order + k * s.pallet_units)
                       - ir_esales(b.mu, b.sd, n.on_hand + n.on_order + (k - 1) * s.pallet_units)), 2) AS value
FROM ir_need n JOIN ir_skus s USING (sku) JOIN ir_position p USING (sku),
     LATERAL (VALUES ('forecast', n.mu_cover, n.sigma_cover, n.need_units),
                     ('last 4 weeks', 3 * p.naive_week, sqrt(3) * p.naive_sd, n.naive_need_units)) b(basis, mu, sd, need),
     range(1, 13) t(k)
WHERE k <= ceil(b.need / s.pallet_units);

-- ── Budget sweep: which lots to buy (anofox_optimize knapsack) ───────────────
-- Three plans, each spending at most the week's budget:
--   today:     last-4-weeks quantities, "top sellers first" (greedy by value);
--   forecast + top sellers first: forecast quantities, same greedy rule;
--   forecast + exact: forecast quantities, opt_knapsack_exact (proven optimum).
CREATE OR REPLACE TABLE ir_choice AS
WITH l AS (
    SELECT basis, service_level, list(value ORDER BY sku, lot) AS v, list(cost ORDER BY sku, lot) AS w,
           list(sku ORDER BY sku, lot) AS skus
    FROM ir_lots GROUP BY ALL
), b AS (SELECT (2500 * i)::DOUBLE AS budget FROM range(4, 25) t(i))
SELECT l.service_level, b.budget, a.rule, a.ord, l.skus, a.res.selected AS selected
FROM l, b, LATERAL (VALUES
    ('today: last 4 weeks, top sellers first', 1, CASE WHEN l.basis = 'last 4 weeks' THEN opt_knapsack_greedy_value(l.v, l.w, b.budget) END),
    ('forecast, top sellers first',             2, CASE WHEN l.basis = 'forecast' THEN opt_knapsack_greedy_value(l.v, l.w, b.budget) END),
    ('forecast, exact (proven optimum)',        3, CASE WHEN l.basis = 'forecast' THEN opt_knapsack_exact(l.v, l.w, b.budget) END)
) a(rule, ord, res)
WHERE a.res IS NOT NULL;

-- Per (service level, budget, rule, SKU): pallets bought and the expected
-- outcome over the covered weeks — always judged with the forecast.
CREATE OR REPLACE TABLE ir_plan AS
WITH picked AS (
    SELECT c.service_level, c.budget::INTEGER AS budget, c.rule, c.ord, u.sku, count(*) FILTER (WHERE u.sel) AS pallets
    FROM ir_choice c, LATERAL (SELECT unnest(c.skus) AS sku, unnest(c.selected) AS sel) u
    GROUP BY ALL
), allsku AS (  -- SKUs a plan has no lots for still get a row (0 pallets)
    SELECT DISTINCT c.service_level, c.budget::INTEGER AS budget, c.rule, c.ord, s.sku FROM ir_choice c, ir_skus s
)
SELECT a.service_level, a.budget, a.rule, a.ord, a.sku, coalesce(p.pallets, 0) AS pallets,
       coalesce(p.pallets, 0) * s.pallet_units AS units,
       round(coalesce(p.pallets, 0) * s.pallet_units * s.unit_cost) AS cost,
       n.need_units, ceil(n.need_units / s.pallet_units)::INTEGER AS pallets_needed,
       round(n.on_hand + n.on_order + coalesce(p.pallets, 0) * s.pallet_units - n.mu_cover) AS exp_leftover_units,
       round(s.margin * ir_esales(n.mu_cover, n.sigma_cover, n.on_hand + n.on_order + coalesce(p.pallets, 0) * s.pallet_units)) AS exp_margin,
       round(100 * ir_esales(n.mu_cover, n.sigma_cover, n.on_hand + n.on_order + coalesce(p.pallets, 0) * s.pallet_units) / n.mu_cover, 1) AS fill_rate_pct,
       round(n.mu_cover - ir_esales(n.mu_cover, n.sigma_cover, n.on_hand + n.on_order + coalesce(p.pallets, 0) * s.pallet_units)) AS exp_lost_units
FROM allsku a LEFT JOIN picked p USING (service_level, budget, rule, ord, sku)
JOIN ir_skus s ON s.sku = a.sku JOIN ir_need n ON n.sku = a.sku AND n.service_level = a.service_level;

CREATE OR REPLACE TABLE ir_summary AS
SELECT p.service_level, p.budget, p.rule, p.ord, sum(p.pallets) AS pallets, sum(p.cost) AS spent, sum(p.exp_margin) AS exp_margin,
       sum(p.exp_lost_units) AS exp_lost_units,
       round(sum(greatest(0, p.exp_leftover_units) * s.unit_cost)) AS exp_overstock_eur,
       round(100 * sum(p.exp_margin / s.margin) / sum(n.mu_cover), 1) AS fill_rate_pct,
       count(*) FILTER (WHERE p.pallets >= p.pallets_needed) AS skus_covered
FROM ir_plan p JOIN ir_skus s USING (sku) JOIN ir_need n ON n.sku = p.sku AND n.service_level = p.service_level
GROUP BY ALL;

-- Enrich the SKU table with the stock position for the dashboard.
CREATE OR REPLACE TABLE ir_skus AS
SELECT s.*, p.on_hand, p.on_order, round(p.naive_week, 1) AS last4_avg FROM ir_skus s JOIN ir_position p USING (sku);
