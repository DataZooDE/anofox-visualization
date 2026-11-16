-- @title Which energy setup pays off for our household?
-- @group How it works
-- @tags  form, scenario, energy
-- @description Solar, a battery, a heat pump and a dynamic tariff against today's bill — 32 inputs in one form.
--
-- Which energy setup pays off for our household? One ::FORM holds every
-- input of the comparison (#1233): 32 fields in four sections shown as tabs
-- (::FORM_LAYOUT), a preset that fills the household in one go, details
-- under "More" (advanced), short fields two to a row (width 'half') and
-- fields that show only when they matter (show_if, with & between
-- conditions: the loan's rate only when the investment is counted AND paid
-- by a loan). The values come back as one JSON variable, cmp; the panels
-- below read it with getvariable('cmp')->>'field'.
--
-- The model is deliberately simple (yearly totals, no hourly profile):
-- solar covers part of the household's own use (more with a battery, more
-- when the home is used during the day), the rest is fed in; a heat pump
-- turns heat demand into electricity by its COP; a dynamic tariff is
-- cheaper on average, and a battery shifts use into its cheap hours.


-- the comparison, one row: today (fixed price, gas) against the choices
CREATE OR REPLACE TEMP VIEW energy AS
WITH v AS (
  SELECT
    (getvariable('cmp')->>'use_kwh')::DOUBLE AS use_kwh,
    (getvariable('cmp')->>'heat_kwh')::DOUBLE AS heat_kwh,
    (getvariable('cmp')->>'day_pct')::DOUBLE / 100 AS day,
    coalesce((getvariable('cmp')->>'pv')::BOOLEAN, false) AS pv,
    coalesce((getvariable('cmp')->>'battery')::BOOLEAN, false) AS battery,
    (getvariable('cmp')->>'pv_kwp')::DOUBLE AS pv_kwp,
    (getvariable('cmp')->>'pv_yield')::DOUBLE AS pv_yield,
    (getvariable('cmp')->>'batt_kwh')::DOUBLE AS batt_kwh,
    (getvariable('cmp')->>'batt_cycles')::DOUBLE AS batt_cycles,
    getvariable('cmp')->>'heating' = 'heat pump' AS hp,
    (getvariable('cmp')->>'hp_cop')::DOUBLE AS cop,
    getvariable('cmp')->>'tariff' = 'dynamic' AS dynamic,
    (getvariable('cmp')->>'fixed_ct')::DOUBLE / 100 AS fixed_eur,
    (getvariable('cmp')->>'dyn_ct')::DOUBLE / 100 AS dyn_eur,
    (getvariable('cmp')->>'dyn_spread')::DOUBLE / 100 AS spread_eur,
    (getvariable('cmp')->>'base_eur')::DOUBLE * 12 AS base_eur,
    (getvariable('cmp')->>'feed_ct')::DOUBLE / 100 AS feed_eur,
    (getvariable('cmp')->>'gas_ct')::DOUBLE / 100 AS gas_eur,
    (getvariable('cmp')->>'gas_base_eur')::DOUBLE * 12 AS gas_base_eur,
    (getvariable('cmp')->>'rise_pct')::DOUBLE / 100 AS rise,
    coalesce((getvariable('cmp')->>'capex')::BOOLEAN, false) AS capex,
    (getvariable('cmp')->>'pv_eur_kwp')::DOUBLE AS pv_eur_kwp,
    (getvariable('cmp')->>'batt_eur_kwh')::DOUBLE AS batt_eur_kwh,
    (getvariable('cmp')->>'hp_eur')::DOUBLE AS hp_eur,
    (getvariable('cmp')->>'subsidy_pct')::DOUBLE / 100 AS subsidy,
    getvariable('cmp')->>'fin' = 'loan' AS loan,
    (getvariable('cmp')->>'loan_pct')::DOUBLE / 100 AS loan_rate,
    (getvariable('cmp')->>'loan_years')::DOUBLE AS loan_years,
    (getvariable('cmp')->>'years')::DOUBLE AS years
), e AS (
  SELECT *,
    use_kwh + CASE WHEN hp THEN heat_kwh / cop ELSE 0 END AS need_kwh,
    CASE WHEN pv THEN pv_kwp * pv_yield ELSE 0 END AS pv_kwh,
    least(0.85, 0.2 + 0.3 * day + CASE WHEN battery THEN least(0.35, batt_kwh * 0.04) ELSE 0 END) AS self_share
  FROM v
), f AS (
  SELECT *,
    least(pv_kwh * self_share, need_kwh) AS self_kwh,
    CASE WHEN battery AND dynamic THEN least(batt_kwh * batt_cycles, need_kwh) ELSE 0 END AS shifted_kwh
  FROM e
), g AS (
  SELECT *,
    need_kwh - self_kwh AS grid_kwh,
    pv_kwh - self_kwh AS feed_kwh,
    use_kwh * fixed_eur + base_eur + heat_kwh * gas_eur + gas_base_eur AS today_eur,
    CASE WHEN capex THEN
      CASE WHEN pv THEN pv_kwp * pv_eur_kwp ELSE 0 END
      + CASE WHEN battery THEN batt_kwh * batt_eur_kwh ELSE 0 END
      + CASE WHEN hp THEN hp_eur * (1 - subsidy) ELSE 0 END
    ELSE 0 END AS invest_eur
  FROM f
)
SELECT *,
  (grid_kwh * CASE WHEN dynamic THEN dyn_eur ELSE fixed_eur END)
    - least(shifted_kwh, grid_kwh) * CASE WHEN dynamic THEN coalesce(spread_eur, 0) ELSE 0 END AS power_eur,
  CASE WHEN hp THEN 0 ELSE heat_kwh * gas_eur + gas_base_eur END AS gas_eur_yr,
  feed_kwh * coalesce(feed_eur, 0) AS feed_in_eur,
  CASE WHEN loan AND invest_eur > 0
       THEN invest_eur * (CASE WHEN loan_rate = 0 THEN 1 / loan_years
                               ELSE loan_rate / (1 - pow(1 + loan_rate, -loan_years)) END) * loan_years - invest_eur
       ELSE 0 END AS interest_eur
FROM g;

CREATE OR REPLACE TEMP VIEW energy_yearly AS
SELECT *, power_eur + base_eur + gas_eur_yr - feed_in_eur AS new_eur
FROM energy;

SELECT 'The comparison'::GROUP;
SELECT 3::COL;
SELECT round(today_eur) ::MONEY, 'Today, per year'::LABEL, 'EUR' ::CURRENCY_CODE FROM energy_yearly;
SELECT 3::COL;
SELECT round(new_eur) ::MONEY, 'With the choices, per year'::LABEL, 'EUR' ::CURRENCY_CODE FROM energy_yearly;
SELECT 3::COL;
SELECT round(today_eur - new_eur) ::MONEY, 'Saved per year'::LABEL, 'EUR' ::CURRENCY_CODE FROM energy_yearly;
SELECT 3::COL;
SELECT CASE WHEN invest_eur > 0 AND today_eur > new_eur
            THEN round((invest_eur + interest_eur) / (today_eur - new_eur), 1) END ::METRIC, 'Pays back after, years'::LABEL
FROM energy_yearly;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT field AS cmp ::FORM, section, type, value, options, min, max, step, label, unit, help, show_if, advanced, width,
       'Your household and the options' ::TITLE,
       'Start from a preset, then change what differs.' ::SUBTITLE,
       'tabs' ::FORM_LAYOUT
FROM (VALUES
  -- Household
  (1,  'Household', 'size',          'preset',   NULL,       '{"1 person, flat": {"people": 1, "use_kwh": 1500, "heat_kwh": 6000, "day_pct": 20}, "2 people, flat": {"people": 2, "use_kwh": 2500, "heat_kwh": 8000, "day_pct": 30}, "4 people, house": {"people": 4, "use_kwh": 4000, "heat_kwh": 18000, "day_pct": 40}}', NULL, NULL, NULL, 'Preset', NULL, 'Fills people, use, heat and daytime share; change any of them afterwards.', NULL, false, 'full'),
  (2,  'Household', 'people',        'number',   '4',        NULL, 1, 12, 1,        'People', NULL, NULL, NULL, false, 'half'),
  (3,  'Household', 'use_kwh',       'number',   '4000',     NULL, 500, 20000, 100, 'Electricity use', 'kWh/yr', 'Without heating: what the meter shows today.', NULL, false, 'half'),
  (4,  'Household', 'heat_kwh',      'number',   '18000',    NULL, 0, 60000, 500,   'Heat demand', 'kWh/yr', 'Space heating and hot water: last year''s gas use is a good guess.', NULL, false, 'half'),
  (5,  'Household', 'day_pct',       'slider',   '40',       NULL, 0, 100, 5,       'Use during the day', '%', 'The share used while the sun shines: higher when someone is at home.', NULL, false, 'half'),
  (6,  'Household', 'use_sd',        'number',   '10',       NULL, 0, 50, 1,        'Use varies by ±', '%', 'How much a year can differ from the average: the range shown with the result.', NULL, true, 'half'),
  -- Systems
  (7,  'Systems',   'pv',            'toggle',   'true',     NULL, NULL, NULL, NULL, 'Solar panels', NULL, NULL, NULL, false, 'half'),
  (8,  'Systems',   'battery',       'toggle',   'true',     NULL, NULL, NULL, NULL, 'Battery', NULL, NULL, NULL, false, 'half'),
  (9,  'Systems',   'pv_kwp',        'number',   '8',        NULL, 1, 30, 0.5,      'Solar size', 'kWp', NULL, 'pv=true', false, 'half'),
  (10, 'Systems',   'pv_yield',      'number',   '950',      NULL, 600, 1300, 10,   'Solar yield', 'kWh/kWp', 'Per kWp and year: about 950 in central Europe, more in the south.', 'pv=true', true, 'half'),
  (11, 'Systems',   'batt_kwh',      'number',   '8',        NULL, 1, 30, 0.5,      'Battery size', 'kWh', NULL, 'battery=true', false, 'half'),
  (12, 'Systems',   'heating',       'dropdown', 'heat pump', 'gas, heat pump', NULL, NULL, NULL, 'Heating', NULL, NULL, NULL, false, 'half'),
  (13, 'Systems',   'hp_cop',        'number',   '3.2',      NULL, 2, 5, 0.1,       'Heat pump COP', NULL, 'Heat out per kWh in, over the year: 3–4 for a well-insulated house.', 'heating=heat pump', false, 'half'),
  (14, 'Systems',   'batt_cycles',   'number',   '250',      NULL, 50, 365, 5,      'Battery cycles', '/yr', 'Full cycles a year: how much the battery shifts.', 'battery=true', true, 'half'),
  -- Prices
  (15, 'Prices',    'tariff',        'dropdown', 'dynamic',  'fixed, dynamic', NULL, NULL, NULL, 'Tariff', NULL, 'Dynamic: the price follows the exchange hour by hour.', NULL, false, 'half'),
  (16, 'Prices',    'fixed_ct',      'number',   '32',       NULL, 10, 80, 0.1,     'Fixed price', 'ct/kWh', 'Also today''s price, for the comparison.', NULL, false, 'half'),
  (17, 'Prices',    'dyn_ct',        'number',   '27',       NULL, 5, 80, 0.1,      'Dynamic price, average', 'ct/kWh', NULL, 'tariff=dynamic', false, 'half'),
  (18, 'Prices',    'dyn_spread',    'number',   '12',       NULL, 0, 40, 0.5,      'Cheap hours save', 'ct/kWh', 'How much cheaper the hours a battery charges in are.', 'tariff=dynamic & battery=true', true, 'half'),
  (19, 'Prices',    'base_eur',      'number',   '12',       NULL, 0, 50, 0.5,      'Electricity base fee', '€/month', NULL, NULL, false, 'half'),
  (20, 'Prices',    'feed_ct',       'number',   '7.9',      NULL, 0, 30, 0.1,      'Feed-in pay', 'ct/kWh', NULL, 'pv=true', false, 'half'),
  (21, 'Prices',    'gas_ct',        'number',   '11',       NULL, 3, 40, 0.1,      'Gas price', 'ct/kWh', 'Also today''s, for the comparison.', NULL, false, 'half'),
  (22, 'Prices',    'gas_base_eur',  'number',   '15',       NULL, 0, 50, 0.5,      'Gas base fee', '€/month', NULL, NULL, false, 'half'),
  (23, 'Prices',    'rise_pct',      'number',   '2',        NULL, -5, 15, 0.5,     'Prices rise by', '%/yr', 'Energy prices each year from now, for the totals over the years.', NULL, true, 'half'),
  -- Money
  (24, 'Money',     'capex',         'toggle',   'true',     NULL, NULL, NULL, NULL, 'Count the investment', NULL, NULL, NULL, false, 'full'),
  (25, 'Money',     'pv_eur_kwp',    'number',   '1400',     NULL, 500, 3000, 50,   'Solar, cost', '€/kWp', NULL, 'capex=true & pv=true', false, 'half'),
  (26, 'Money',     'batt_eur_kwh',  'number',   '600',      NULL, 200, 1500, 25,   'Battery, cost', '€/kWh', NULL, 'capex=true & battery=true', false, 'half'),
  (27, 'Money',     'hp_eur',        'number',   '25000',    NULL, 5000, 60000, 500, 'Heat pump, cost', '€', NULL, 'capex=true & heating=heat pump', false, 'half'),
  (28, 'Money',     'subsidy_pct',   'number',   '30',       NULL, 0, 70, 5,        'Subsidy', '%', 'Of the heat pump''s cost.', 'capex=true & heating=heat pump', false, 'half'),
  (29, 'Money',     'fin',           'dropdown', 'cash',     'cash, loan', NULL, NULL, NULL, 'Paid by', NULL, NULL, 'capex=true', false, 'half'),
  (30, 'Money',     'loan_pct',      'number',   '5',        NULL, 0, 15, 0.1,      'Loan interest', '%/yr', NULL, 'capex=true & fin=loan', false, 'half'),
  (31, 'Money',     'loan_years',    'number',   '15',       NULL, 3, 30, 1,        'Loan term', 'years', NULL, 'capex=true & fin=loan', true, 'half'),
  (32, 'Money',     'years',         'number',   '20',       NULL, 5, 30, 1,        'Compare over', 'years', 'How long the systems run: the totals below add up this many years.', NULL, false, 'half')
) t(ord, section, field, type, value, options, min, max, step, label, unit, help, show_if, advanced, width)
ORDER BY ord;

SELECT 6::COL;
SELECT item ::XAXIS, today AS "Today" ::BARCHART, new AS "With the choices" ::BARCHART,
       'Where the money goes, € per year' ::TITLE
FROM energy_yearly, LATERAL (VALUES
  ('Electricity', use_kwh * fixed_eur + base_eur, power_eur + base_eur),
  ('Gas', heat_kwh * gas_eur + gas_base_eur, gas_eur_yr),
  ('Feed-in pay', 0, -feed_in_eur)
) t(item, today, new)
ORDER BY today DESC;

SELECT 12::COL;
SELECT y AS year ::XAXIS,
       round(sum(today_eur * pow(1 + rise, y - 1)) OVER (ORDER BY y)) AS "Today's setup" ::LINECHART,
       round(invest_eur + interest_eur + sum(new_eur * pow(1 + rise, y - 1)) OVER (ORDER BY y)) AS "With the choices" ::LINECHART,
       'Total cost over the years, investment included' ::TITLE
FROM energy_yearly, range(1, CAST(years AS BIGINT) + 1) r(y)
ORDER BY y;
