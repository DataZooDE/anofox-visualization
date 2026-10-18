-- @title Why do customers cancel?
-- @group Sales & customers
-- @tags  statistics, churn
-- @description What makes customers cancel, and how many each retention lever would keep.
--
-- Drivers of churn — why do customers cancel, and which lever pays off?
--
-- LIVE: every number below is computed when the dashboard runs. The script
-- builds a synthetic, deterministic customer base with plain SQL (hash-based
-- noise, no files) and the anofox_statistics extension fits and checks the
-- model: logistic_tidy_by (drivers, odds ratios), logistic_glance_by (fit),
-- logistic_fit_predict_by (hold-out scores), logistic_effects_by (effect
-- curves), roc_curve_by / calibration_curve_by / auc_agg / brier_agg (is the
-- score any good?) and vif_agg (collinearity). The what-if tab re-scores every
-- customer with the fitted coefficients under the levers you set.
--   * Browser demo: DuckDB-Wasm + the anofox_statistics wasm extension.
--   * Natively: `dashboard --check examples/statistics-churn-drivers/showcase.sql`
--     with a duckdb that has anofox_statistics installed (see its README).
--
-- Tabs: Overview · Drivers · Model check · What-if · Action list.

LOAD anofox_statistics;

SELECT 'Why do customers cancel, and which lever pays off?'::DASHBOARD_TITLE;
SELECT 'anofox_statistics'::BRAND;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data (setup statements: plain SQL, no files)
-- ─────────────────────────────────────────────────────────────────────────────
-- Uniform(0,1) from the md5 of a key tuple; a standard normal via Box–Muller.
CREATE OR REPLACE MACRO ch_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO ch_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(ch_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * ch_unif(a, b, c || ':u2'));

-- ch_customers — the 5,000 customers of a made-up team-collaboration SaaS on
-- 1 July 2026, followed to 30 September 2026 (churned = cancelled in Q3).
-- Plans Starter / Team / Business at 49 / 149 / 399 € a month (annual
-- contracts 15% off). The planted log-odds of churning:
--   −1.55 − 0.022·tenure (months) − 1.25·annual − 0.55·onboarded
--   + 0.26·support tickets (90 d) − 0.035·usage change (%) + 1.15·failed payment
--   − 0.10 Team / − 0.35 Business
-- Mobile app, newsletter and region play no part (they should fade); tickets
-- rise when usage drops, so the two drivers are correlated.
CREATE OR REPLACE TABLE ch_customers AS
WITH c AS (
    SELECT i,
           CASE WHEN ch_unif('ch', i, 'plan') < 0.5 THEN 'Starter' WHEN ch_unif('ch', i, 'plan') < 0.85 THEN 'Team' ELSE 'Business' END AS plan,
           1 + floor(72 * power(ch_unif('ch', i, 'ten'), 1.3))::INTEGER AS tenure_m,
           ch_unif('ch', i, 'contract') < 0.42 AS annual,
           ch_unif('ch', i, 'onb') < 0.68 AS onboarded,
           ch_unif('ch', i, 'app') < 0.55 AS mobile_app,
           ch_unif('ch', i, 'news') < 0.35 AS newsletter,
           CASE WHEN ch_unif('ch', i, 'reg') < 0.5 THEN 'DACH' WHEN ch_unif('ch', i, 'reg') < 0.8 THEN 'Benelux' ELSE 'Nordics' END AS region,
           round(least(60, greatest(-70, 2 + 18 * ch_rnorm('ch', i, 'usage')))) AS usage_change_pct,
           ch_unif('ch', i, 'pay') < 0.07 AS payment_failed
    FROM range(1, 5001) r(i)
), f AS (
    SELECT c.*,
           CASE plan WHEN 'Starter' THEN 49 WHEN 'Team' THEN 149 ELSE 399 END * CASE WHEN annual THEN 0.85 ELSE 1 END AS mrr,
           floor(-ln(ch_unif('ch', i, 'tix')) * (0.8 + CASE plan WHEN 'Business' THEN 1.2 WHEN 'Team' THEN 0.5 ELSE 0 END
                                                 + greatest(0, -usage_change_pct) / 30.0))::INTEGER AS tickets_90d
    FROM c
)
SELECT 'C-' || lpad(i::VARCHAR, 5, '0') AS customer, 'all' AS segment, plan, region, tenure_m,
       CASE WHEN annual THEN 'annual' ELSE 'monthly' END AS contract, onboarded, mobile_app, newsletter,
       usage_change_pct::DOUBLE AS usage_change_pct, payment_failed, tickets_90d, round(mrr, 2) AS mrr,
       (ch_unif('ch', i, 'churn') < 1 / (1 + exp(-(
            -1.55 - 0.022 * tenure_m - 1.25 * annual::INTEGER - 0.55 * onboarded::INTEGER
            + 0.26 * tickets_90d - 0.035 * usage_change_pct + 1.15 * payment_failed::INTEGER
            + CASE plan WHEN 'Business' THEN -0.35 WHEN 'Team' THEN -0.1 ELSE 0 END)))) AS churned,
       CASE WHEN ch_unif('ch', i, 'split') < 0.25 THEN 'holdout' ELSE 'train' END AS split
FROM f ORDER BY i;

-- The model matrix, in units a manager can act on.
CREATE OR REPLACE TABLE ch_model AS
SELECT customer, segment, split, churned::INTEGER::DOUBLE AS churn_d,
       tenure_m / 12.0 AS tenure_y, (contract = 'annual')::INTEGER::DOUBLE AS annual, onboarded::INTEGER::DOUBLE AS onboarded,
       tickets_90d::DOUBLE AS tickets, -usage_change_pct / 10.0 AS usage_drop_10, payment_failed::INTEGER::DOUBLE AS payment_failed,
       mrr / 100.0 AS mrr_100, mobile_app::INTEGER::DOUBLE AS mobile_app, newsletter::INTEGER::DOUBLE AS newsletter,
       (region = 'DACH')::INTEGER::DOUBLE AS dach
FROM ch_customers;

-- The drivers: one fit on all customers (term names for the charts).
CREATE OR REPLACE TABLE ch_terms AS
SELECT * FROM logistic_tidy_by('ch_model', segment, churn_d,
    [tenure_y, annual, onboarded, tickets, usage_drop_10, payment_failed, mrr_100, mobile_app, newsletter, dach],
    names := ['tenure (per year)', 'annual contract', 'onboarding done', 'support ticket (each)', 'usage drop (per 10 pp)',
              'failed payment', 'MRR (per 100 €)', 'mobile app', 'newsletter', 'DACH region']);

-- Hold-out scores: fitted on 75% of the customers, scored on the other 25%,
-- next to the customer-success team's rule of thumb (red flags).
CREATE OR REPLACE TABLE ch_scores AS
WITH f AS (
    SELECT customer, churn_d, split, yhat
    FROM logistic_fit_predict_by('ch_model', segment, churn_d,
        [tenure_y, annual, onboarded, tickets, usage_drop_10, payment_failed, mrr_100, mobile_app, newsletter, dach],
        split := split)
)
SELECT 'logistic model' AS model, f.customer, f.churn_d::INTEGER AS churned, f.yhat AS score
FROM f WHERE f.split = 'holdout'
UNION ALL
SELECT 'red-flag rule', c.customer, c.churned::INTEGER,
       (0.05 + 0.15 * (c.tickets_90d >= 3)::INTEGER + 0.25 * c.payment_failed::INTEGER
        + 0.10 * (c.contract = 'monthly')::INTEGER)::DOUBLE
FROM ch_customers c WHERE c.split = 'holdout';

-- Every customer's churn probability now (all-customer fit) and under the
-- what-if levers (inputs → expected values: a share s of the monthly
-- customers moved to annual has p = (1 − s)·p(monthly) + s·p(annual), …).
CREATE OR REPLACE MACRO ch_lever(name, dflt) AS
    least(100, greatest(0, coalesce(TRY_CAST(getvariable(name) AS DOUBLE), dflt))) / 100;
CREATE OR REPLACE VIEW ch_beta AS
SELECT max(estimate) FILTER (WHERE term = '(Intercept)') AS b0,
       max(estimate) FILTER (WHERE term = 'tenure (per year)') AS b_ten,
       max(estimate) FILTER (WHERE term = 'annual contract') AS b_ann,
       max(estimate) FILTER (WHERE term = 'onboarding done') AS b_onb,
       max(estimate) FILTER (WHERE term = 'support ticket (each)') AS b_tix,
       max(estimate) FILTER (WHERE term = 'usage drop (per 10 pp)') AS b_use,
       max(estimate) FILTER (WHERE term = 'failed payment') AS b_pay,
       max(estimate) FILTER (WHERE term = 'MRR (per 100 €)') AS b_mrr,
       max(estimate) FILTER (WHERE term = 'mobile app') AS b_app,
       max(estimate) FILTER (WHERE term = 'newsletter') AS b_news,
       max(estimate) FILTER (WHERE term = 'DACH region') AS b_dach
FROM ch_terms;
CREATE OR REPLACE MACRO ch_p(eta) AS 1 / (1 + exp(-eta));
-- Expected churn probability of one customer when a share of the monthly
-- customers moves to annual (s_ann), a share of the not-onboarded customers is
-- onboarded (s_onb), a share of the failed payments is recovered (s_dun) and
-- support tickets fall by s_tix — the average over the four lever outcomes.
CREATE OR REPLACE MACRO ch_whatif(eta, annual, onboarded, failed, tickets, b_ann, b_onb, b_pay, b_tix,
                                  s_ann, s_onb, s_dun, s_tix) AS
    list_sum(list_transform([[0, 0, 0], [0, 0, 1], [0, 1, 0], [0, 1, 1], [1, 0, 0], [1, 0, 1], [1, 1, 0], [1, 1, 1]],
        lambda k: (CASE WHEN k[1] = 1 THEN s_ann * (1 - annual) ELSE 1 - s_ann * (1 - annual) END)
                * (CASE WHEN k[2] = 1 THEN s_onb * (1 - onboarded) ELSE 1 - s_onb * (1 - onboarded) END)
                * (CASE WHEN k[3] = 1 THEN s_dun * failed ELSE 1 - s_dun * failed END)
                * ch_p(eta - b_tix * tickets * s_tix
                       + b_ann * greatest(annual, k[1]) + b_onb * greatest(onboarded, k[2])
                       + b_pay * CASE WHEN k[3] = 1 THEN 0 ELSE failed END)));
-- Each customer's linear predictor without the lever terms (contract,
-- onboarding, failed payment, tickets enter through ch_whatif).
CREATE OR REPLACE TABLE ch_base AS
SELECT c.customer, c.plan, c.contract, c.region, c.tenure_m, c.tickets_90d, c.usage_change_pct, c.mrr, c.churned,
       c.payment_failed AS failed_payment, c.onboarded AS onboarding_done,
       m.annual, m.onboarded, m.tickets, m.payment_failed,
       b0 + b_ten * m.tenure_y + b_tix * m.tickets + b_use * m.usage_drop_10
          + b_mrr * m.mrr_100 + b_app * m.mobile_app + b_news * m.newsletter + b_dach * m.dach AS eta_core,
       b_ann, b_onb, b_tix, b_pay
FROM ch_customers c JOIN ch_model m USING (customer), ch_beta;
CREATE OR REPLACE VIEW ch_risk AS
SELECT *, ch_whatif(eta_core, annual, onboarded, payment_failed, tickets, b_ann, b_onb, b_pay, b_tix, 0, 0, 0, 0) AS p_now,
       ch_whatif(eta_core, annual, onboarded, payment_failed, tickets, b_ann, b_onb, b_pay, b_tix,
                 ch_lever('move_annual_pct', 20), ch_lever('onboard_pct', 30),
                 ch_lever('dunning_pct', 50), ch_lever('tickets_cut_pct', 25)) AS p_whatif
FROM ch_base;
-- Each lever on its own, at its input value.
CREATE OR REPLACE VIEW ch_levers AS
SELECT lever, sum(p_now - p_lever) AS churners_saved, sum((p_now - p_lever) * mrr) * 12 AS mrr_saved_year,
       -- the annual contract's 15% discount, on the moved customers who stay
       sum(s_ann * (1 - annual) * (1 - p_lever) * mrr * 0.15) * 12 AS discount_cost_year
FROM (SELECT b.mrr, b.annual, l.s_ann,
             ch_whatif(eta_core, annual, onboarded, payment_failed, tickets, b_ann, b_onb, b_pay, b_tix, 0, 0, 0, 0) AS p_now,
             l.lever,
             ch_whatif(eta_core, annual, onboarded, payment_failed, tickets, b_ann, b_onb, b_pay, b_tix,
                       l.s_ann, l.s_onb, l.s_dun, l.s_tix) AS p_lever
      FROM ch_base b,
           (SELECT 'move monthly customers to annual' AS lever, ch_lever('move_annual_pct', 20) AS s_ann, 0 AS s_onb, 0 AS s_dun, 0 AS s_tix
            UNION ALL SELECT 'finish onboarding', 0, ch_lever('onboard_pct', 30), 0, 0
            UNION ALL SELECT 'recover failed payments (dunning)', 0, 0, ch_lever('dunning_pct', 50), 0
            UNION ALL SELECT 'fewer support tickets', 0, 0, 0, ch_lever('tickets_cut_pct', 25)) l)
GROUP BY lever;

-- Quarterly churn history (the four quarters before Q3 2026 come from last
-- year's board reports — synthetic, fixed).
CREATE OR REPLACE TABLE ch_history AS
SELECT * FROM (VALUES ('Q3 2025', 6.9), ('Q4 2025', 7.2), ('Q1 2026', 7.6), ('Q2 2026', 8.0)) t(quarter, churn_pct)
UNION ALL
SELECT 'Q3 2026', round(100 * avg(churned::INTEGER), 1) FROM ch_customers;

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 12::COL;
SELECT '**Who asks:** the head of customer success, planning the Q4 retention budget — *churn has crept up for five quarters: what drives it, and which lever is worth paying for?* **The data:** the 5,000 customers on 1 July 2026 and whether they cancelled by 30 September (Q3). **The model:** a logistic regression of churn on ten customer attributes (`logistic_tidy_by`), checked on a 25% hold-out (Model check), then used to size four levers (What-if) and rank the accounts to call (Action list).'::MARKDOWN,
       'The question'::TITLE;

SELECT 'Q3 2026 at a glance'::GROUP;
SELECT (SELECT churn_pct FROM ch_history WHERE quarter = 'Q3 2026')::PERCENT,
       (SELECT churn_pct FROM ch_history WHERE quarter = 'Q2 2026')::DELTA_LOWER_GOOD, 'vs Q2'::DELTA_LABEL, 'Churn in Q3'::LABEL,
       'Customers who cancelled in Q3 as a share of the 5,000 customers on 1 July.'::INFO;
SELECT count(*) FILTER (WHERE churned)::METRIC, 'customers lost'::LABEL FROM ch_customers;
SELECT round(sum(mrr) FILTER (WHERE churned))::METRIC, 'MRR lost (€ / month)'::LABEL FROM ch_customers;
SELECT round(auc_agg(churned, score), 2)::METRIC, 'hold-out AUC of the model'::LABEL, '<0.65:bad; <0.75:warn; good'::THRESHOLDS,
       'AUC: the chance that a customer who churned gets a higher score than one who stayed, on the 25 % of customers the model was not fitted on. 0.5 is a coin flip, 1 a perfect ranking.'::INFO
FROM ch_scores WHERE model = 'logistic model';
SELECT term::TEXT_LARGE, 'strongest driver (odds × ' || printf('%.1f', exp(estimate)) || ')'::LABEL
FROM ch_terms WHERE term <> '(Intercept)' AND estimate > 0 ORDER BY estimate DESC LIMIT 1;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 420::HEIGHT;
SELECT anofox_plot_terms(t, reference := 1, x_label := 'odds ratio of churning in Q3 (95% CI; 1 = no effect)'),
       'Drivers of churn — failed payments and monthly contracts weigh most; app, newsletter, region and price fade (n.s.)'::TITLE
FROM (SELECT * REPLACE (exp(estimate) AS estimate, exp(conf_low) AS conf_low, exp(conf_high) AS conf_high)
      FROM ch_terms WHERE term <> '(Intercept)') t;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT quarter::XAXIS, churn_pct AS "churn (%)" ::LINECHART, 'Churn per quarter, % — up five quarters in a row'::TITLE
FROM ch_history ORDER BY quarter[4:], quarter;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT contract::XAXIS, CASE WHEN tickets_90d >= 3 THEN '3+ tickets' ELSE '0–2 tickets' END::CATEGORY,
       round(100 * avg(churned::INTEGER), 1) AS "churn (%)" ::BARCHART,
       'Churn by contract and support load, % — monthly customers with 3+ tickets churn most'::TITLE
FROM ch_customers GROUP BY ALL ORDER BY 3 DESC;

-- ── 2 · Drivers ──────────────────────────────────────────────────────────────
SELECT 'Drivers'::TAB;

SELECT 12::COL;
SELECT '**How to read the drivers.** The model links the log-odds of churning to the attributes, $\log\frac{p}{1-p} = \beta_0 + \beta_1 x_1 + \dots + \beta_k x_k$, so one unit more of $x_j$ multiplies the odds by $\text{OR}_j = e^{\beta_j}$. Each odds ratio compares two customers who differ in one attribute only: an **annual contract** multiplies the odds of churning by about 0.27 (a 73% cut), a **failed payment** by 3.7, each **support ticket** in the last 90 days by 1.3, each **10-point drop in usage** by 1.37. **What to look for:** faded terms have a 95% interval that includes 1 — *mobile app*, *newsletter*, *DACH region* and *MRR* do not predict churn once the rest is known, so money spent on pushing the app or the newsletter will not retain anyone. The effect curves turn two drivers into probabilities with the other attributes held at their average. Tickets and usage drop are correlated (struggling customers file tickets): the VIF table in Model check shows the overlap is small enough to read them separately.'::MARKDOWN,
       'What to look for'::TITLE;

SELECT 12::COL;
SELECT 'p > 0.05'::FLAG;
SELECT 'Odds ratio: top 3'::HIGHLIGHT;
SELECT 'Odds ratios (fit on all 5,000 customers)'::TITLE,
       term AS "Driver" ::TABLE, round(exp(estimate), 2) AS "Odds ratio" ::COLORSCALE,
       printf('%.2f – %.2f', exp(conf_low), exp(conf_high)) AS "95% CI", round(p_value, 4) AS p,
       CASE WHEN conf_low > 0 OR conf_high < 0 THEN CASE WHEN estimate > 0 THEN 'raises churn' ELSE 'protects' END ELSE 'no effect' END AS "Reading" ::BADGE
FROM ch_terms WHERE term <> '(Intercept)' ORDER BY abs(estimate) * (p_value < 0.05)::INTEGER DESC, p_value;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT anofox_plot_prediction(p, x_label := 'support tickets in the last 90 days', y_label := 'P(churn in a quarter)'),
       'Each ticket adds risk — about 4% with none, 14% with five'::TITLE
FROM logistic_effects_by('ch_model', segment, churn_d,
        [tickets, tenure_y, annual, onboarded, usage_drop_10, payment_failed, mrr_100, mobile_app, newsletter, dach],
        focal := 1, n_points := 25) p;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT anofox_plot_prediction(p, x_label := 'usage drop over 3 months (per 10 pp; < 0 = usage grew)', y_label := 'P(churn in a quarter)'),
       'Falling usage is the early warning — 5% when usage is flat, about 17% after a 40% drop'::TITLE
FROM logistic_effects_by('ch_model', segment, churn_d,
        [usage_drop_10, tenure_y, annual, onboarded, tickets, payment_failed, mrr_100, mobile_app, newsletter, dach],
        focal := 1, n_points := 25) p;

-- ── 3 · Model check ──────────────────────────────────────────────────────────
SELECT 'Model check'::TAB;

SELECT 12::COL;
SELECT '**Can we trust the score?** The model is refitted on 75% of the customers and scored on the other 25% it has not seen (`logistic_fit_predict_by` with `split`), next to the customer-success team''s **red-flag rule** (points for 3+ tickets, a failed payment and a monthly contract). **What to look for:** the **ROC** curve further from the diagonal ranks churners better (AUC); in the **calibration** plot a trustworthy model''s points lie on the diagonal — the what-if tab needs *calibrated* probabilities, not just a ranking. The **VIF** table checks that no two drivers measure the same thing (VIF above 5 would blur their separate effects).'::MARKDOWN,
       'What to look for'::TITLE;

SELECT 'Hold-out scores (1,250 customers)'::GROUP;
SELECT round(auc_agg(churned, score), 3)::METRIC, 'AUC — logistic model'::LABEL FROM ch_scores WHERE model = 'logistic model';
SELECT round(auc_agg(churned, score), 3)::METRIC, 'AUC — red-flag rule'::LABEL FROM ch_scores WHERE model = 'red-flag rule';
SELECT round(brier_agg(churned, score), 4)::METRIC, 'Brier — logistic (lower is better)'::LABEL,
       'Brier score: the mean squared difference between the predicted churn probability and what happened (1 = churned, 0 = stayed). It rewards probabilities that are both sharp and calibrated.'::INFO
FROM ch_scores WHERE model = 'logistic model';
SELECT round(brier_agg(churned, score), 4)::METRIC, 'Brier — red-flag rule'::LABEL FROM ch_scores WHERE model = 'red-flag rule';
SELECT round(max(value) FILTER (WHERE metric = 'pseudo_r_squared'), 3)::METRIC, 'pseudo R² (all customers)'::LABEL
FROM logistic_glance_by('ch_model', segment, churn_d,
        [tenure_y, annual, onboarded, tickets, usage_drop_10, payment_failed, mrr_100, mobile_app, newsletter, dach]);
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_curve(c), 'ROC on the hold-out — the model ranks churners clearly better than the rule'::TITLE
FROM roc_curve_by('ch_scores', model, churned, score) c;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_curve(c), 'Calibration — the model sits on the diagonal where most customers are (sparse top bins are noisy); the rule over-promises'::TITLE
FROM calibration_curve_by('ch_scores', model, churned, score, n_bins := 8) c;

SELECT 12::COL;
SELECT 'VIF > 5'::FLAG;
WITH v AS (
    SELECT vif_agg([tenure_y, annual, onboarded, tickets, usage_drop_10, payment_failed, mrr_100, mobile_app, newsletter, dach]) AS vif
    FROM ch_model
)
SELECT 'Collinearity check — every VIF is near 1, the drivers can be read one by one'::TITLE,
       (['tenure (per year)', 'annual contract', 'onboarding done', 'support ticket (each)', 'usage drop (per 10 pp)',
         'failed payment', 'MRR (per 100 €)', 'mobile app', 'newsletter', 'DACH region'])[k] AS "Driver" ::TABLE,
       round(vif[k], 2) AS "VIF" ::PLAIN
FROM v, range(1, 11) r(k)
ORDER BY vif[k] DESC;

-- ── 4 · What-if ──────────────────────────────────────────────────────────────
SELECT 'What-if'::TAB;

SELECT 'Retention levers for Q4 (re-scores all 5,000 customers)'::GROUP;
SELECT 20 AS move_annual_pct ::NUMBER, 'Monthly customers moved to annual (%)'::LABEL;
SELECT 50 AS dunning_pct ::NUMBER, 'Failed payments recovered (%)'::LABEL;
SELECT 30 AS onboard_pct ::NUMBER, 'Not-onboarded customers onboarded (%)'::LABEL;
SELECT 25 AS tickets_cut_pct ::NUMBER, 'Support tickets avoided (%)'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT '**How the what-if works.** Each customer keeps their attributes; a lever changes one of them for a share of the customers (as an expected value), and the fitted logistic model turns the new attributes into a churn probability. Summed over the customers this gives the expected churners in a quarter like Q3 — with the levers and without. **Read with care:** the coefficients describe who churned, not what happens when we intervene. Customers who *choose* an annual contract may differ from those we *talk into* one, so treat these numbers as an upper bound for sizing, and confirm the winning lever with an A/B test (see the A/B test example). The annual contract is not free: its 15% discount is counted against it.'::MARKDOWN,
       'Read this first'::TITLE;

SELECT 'A quarter with the levers (arrow = change vs. doing nothing)'::GROUP;
SELECT round(sum(p_whatif))::METRIC, 'expected churners per quarter'::LABEL FROM ch_risk;
SELECT round(sum(p_now - p_whatif))::METRIC, 'customers saved per quarter'::LABEL FROM ch_risk;
SELECT round(100 * avg(p_whatif), 1)::PERCENT, round(100 * avg(p_now), 1)::DELTA_LOWER_GOOD, 'churn per quarter'::LABEL FROM ch_risk;
SELECT round(sum((1 - p_whatif) * mrr))::METRIC, round(sum((1 - p_now) * mrr))::DELTA, 'MRR kept (€ / month, before discounts)'::LABEL FROM ch_risk;
SELECT round(sum(mrr_saved_year - discount_cost_year))::METRIC, 'net € a year (the levers one by one, summed)'::LABEL, '<0:bad; good'::THRESHOLDS FROM ch_levers;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT lever::XAXIS, round(mrr_saved_year - discount_cost_year) AS "net € a year" ::BARCHART, 1::FLIP, 0::REFLINE, '€'::YFORMAT,
       'tween'::TRANSITION,
       'Net value per lever — the annual discount costs more than it saves'::TITLE
FROM ch_levers ORDER BY 2;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT plan::XAXIS, scenario::CATEGORY, round(churners, 1) AS "expected churners" ::BARCHART,
       'tween'::TRANSITION,
       'Expected churners per quarter by plan — now vs. with the levers'::TITLE
FROM (SELECT plan, 'now' AS scenario, sum(p_now) AS churners, 1 AS k FROM ch_risk GROUP BY plan
      UNION ALL SELECT plan, 'with the levers', sum(p_whatif), 2 FROM ch_risk GROUP BY plan)
ORDER BY CASE plan WHEN 'Starter' THEN 1 WHEN 'Team' THEN 2 ELSE 3 END, k;

SELECT 12::COL;
SELECT 'net_per_year < 0'::FLAG;
SELECT 'Levers one by one (at the input values)'::TITLE,
       lever AS "Lever" ::TABLE, round(churners_saved, 1) AS "Churners saved / quarter",
       round(mrr_saved_year) AS "MRR saved (€ / year)", round(discount_cost_year) AS "Discount cost (€ / year)",
       round(mrr_saved_year - discount_cost_year) AS net_per_year ::COLORSCALE_DIVERGING
FROM ch_levers ORDER BY net_per_year DESC;

-- ── 5 · Action list ──────────────────────────────────────────────────────────
SELECT 'Action list'::TAB;

SELECT 'Customers still with us on 1 October'::GROUP;
SELECT count(*) FILTER (WHERE p_now >= 0.2)::METRIC, 'high-risk accounts (churn probability ≥ 20%)'::LABEL FROM ch_risk WHERE NOT churned;
SELECT round(sum(p_now * mrr) FILTER (WHERE p_now >= 0.2))::METRIC, 'expected MRR loss in them (€ / month)'::LABEL FROM ch_risk WHERE NOT churned;
SELECT round(100 * sum(p_now * mrr) FILTER (WHERE p_now >= 0.2) / sum(p_now * mrr))::PERCENT, 'share of all expected MRR loss'::LABEL FROM ch_risk WHERE NOT churned;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 'contract'::SUBTOTAL;
SELECT 'Share of MRR at risk (%): none; sum'::TOTALS;
SELECT 'Risk bands by contract — a few high-risk monthly accounts hold most of the expected loss'::TITLE,
       contract AS "Contract" ::TABLE, band AS "Risk band", count(*) AS "Customers",
       round(sum(p_now), 1) AS "Expected churners", round(sum(p_now * mrr)) AS "MRR at risk",
       round(100 * sum(p_now * mrr) / sum(sum(p_now * mrr)) OVER (), 1) AS "Share of MRR at risk (%)" ::COLORSCALE
FROM (SELECT *, CASE WHEN p_now >= 0.2 THEN '1 high (≥ 20%)' WHEN p_now >= 0.1 THEN '2 elevated (10–20%)'
                     WHEN p_now >= 0.05 THEN '3 watch (5–10%)' ELSE '4 low (< 5%)' END AS band
      FROM ch_risk WHERE NOT churned)
GROUP BY contract, band ORDER BY contract DESC, band;

SELECT 12::COL;
SELECT 'Failed payment = yes'::FLAG;
SELECT 'MRR at risk: top 5'::HIGHLIGHT;
SELECT 'The 25 accounts to call first (expected MRR loss) — and the lever each one needs'::TITLE,
       customer AS "Customer" ::TABLE, plan AS "Plan", contract AS "Contract",
       round(100 * p_now, 1) AS "Churn probability (%)" ::COLORSCALE,
       tickets_90d AS "Tickets (90 d)", usage_change_pct AS "Usage change (%)" ::COLORSCALE_DIVERGING,
       CASE WHEN failed_payment THEN 'yes' ELSE 'no' END AS "Failed payment",
       round(p_now * mrr, 1) AS "MRR at risk",
       CASE WHEN failed_payment THEN 'fix the payment'
            WHEN tickets_90d >= 3 THEN 'senior CSM call'
            WHEN NOT onboarding_done THEN 'onboarding session'
            WHEN usage_change_pct <= -20 THEN 'usage review'
            ELSE 'check-in' END AS "Next step" ::BADGE
FROM ch_risk WHERE NOT churned ORDER BY p_now * mrr DESC LIMIT 25;

SELECT 12::COL;
WITH l AS (SELECT * FROM ch_levers ORDER BY mrr_saved_year - discount_cost_year DESC),
     t AS (SELECT round(sum(p_now))::INTEGER AS now_, round(sum(p_whatif))::INTEGER AS whatif FROM ch_risk)
SELECT ('### Recommendation' || E'\n\n'
        || '**Fund ' || (SELECT string_agg(lever, ', ' ORDER BY mrr_saved_year - discount_cost_year DESC) FROM l WHERE mrr_saved_year - discount_cost_year > 0)
        || '** — together with the other levers they take the expected churners per quarter from **' || now_ || '** to **' || whatif || '**. '
        || coalesce('**Do not** push annual contracts with the 15% discount: it saves ' || (SELECT printf('%,.0f', mrr_saved_year) FROM l WHERE lever LIKE 'move%')
                    || ' € a year but gives away ' || (SELECT printf('%,.0f', discount_cost_year) FROM l WHERE lever LIKE 'move%' AND discount_cost_year > mrr_saved_year)
                    || ' € — offer annual terms at a smaller discount, or only to high-risk accounts. ', '')
        || 'Start with the 25 accounts above: a failed payment is the single largest driver and the cheapest to fix. '
        || 'Skip the app and newsletter campaigns — neither predicts churn. Validate the winning lever with a randomised test before scaling it: the model shows association, not effect.')::MARKDOWN,
       'Recommendation for the head of customer success'::TITLE
FROM t;
