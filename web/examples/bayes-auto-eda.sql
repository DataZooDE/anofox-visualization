-- @title What sets churners apart?
-- @group Sales & customers
-- @tags  bayesian network, automatic EDA, churn
-- @description A Bayesian network's automatic analysis: what drives a target, what hangs together, what to ask next, one case explained.
--
-- What sets churners apart? An automatic exploratory analysis of a Bayesian
-- network learned from a customer table (#1277). A producer — any engine or
-- library that learns the network and queries it (pgmpy, bnlearn, a DuckDB
-- extension) — writes plain tables, and this dashboard draws them. To use it
-- as a template, replace the tables below with your producer's output; the
-- column names are the contract:
--   * bn_insight: target, target_state, node, state, p_state, p_given_target,
--     p_given_not_target, p_target_given_state, difference, lift, distance,
--     rank — one row per state of every other variable, per target state;
--   * bn_associations: a, b, mutual_information, normalized — one row per
--     pair of variables;
--   * bn_value_of_information: hypothesis, test, mutual_information, cost,
--     score — what each still unobserved variable would tell about the
--     hypothesis, and what finding it out costs;
--   * bn_explanations: rank, probability, explanation (a LIST of
--     {node, state}) — the most probable explanations of a case;
--   * bn_retracted: node, observed_state, p_observed_given_rest, surprise —
--     each observation predicted from all the others;
--   * bn_impact: hypothesis_state, evidence_subset, p_without, p_with — the
--     hypothesis as each piece of evidence is added;
--   * bn_conflict: subset, conflict — the evidence's conflict measure (bits).
--
-- Tabs: Drivers (pick a target) · One customer.
-- Made-up, hand-written numbers of a churn network over 5,000 customers of
-- a team-collaboration SaaS (no files).

-- ─────────────────────────────────────────────────────────────────────────────
-- The producer's tables
-- ─────────────────────────────────────────────────────────────────────────────
-- What sets a target state apart: P(state | target) and P(state | not target)
-- per state of every other variable; the rest follows from them and P(target).
CREATE OR REPLACE TABLE bn_insight AS
WITH g(target, target_state, p_target, node, state, p_given_target, p_given_not_target) AS (VALUES
    ('churned', 'yes', 0.18, 'contract', 'monthly', 0.86, 0.47),
    ('churned', 'yes', 0.18, 'contract', 'annual', 0.14, 0.53),
    ('churned', 'yes', 0.18, 'tenure', 'under 1 year', 0.52, 0.21),
    ('churned', 'yes', 0.18, 'tenure', '1–3 years', 0.33, 0.41),
    ('churned', 'yes', 0.18, 'tenure', 'over 3 years', 0.15, 0.38),
    ('churned', 'yes', 0.18, 'support tickets', 'none', 0.18, 0.55),
    ('churned', 'yes', 0.18, 'support tickets', '1–2', 0.37, 0.33),
    ('churned', 'yes', 0.18, 'support tickets', '3 or more', 0.45, 0.12),
    ('churned', 'yes', 0.18, 'usage', 'falling', 0.58, 0.17),
    ('churned', 'yes', 0.18, 'usage', 'steady', 0.31, 0.55),
    ('churned', 'yes', 0.18, 'usage', 'rising', 0.11, 0.28),
    ('churned', 'yes', 0.18, 'payment failed', 'yes', 0.21, 0.05),
    ('churned', 'yes', 0.18, 'payment failed', 'no', 0.79, 0.95),
    ('churned', 'yes', 0.18, 'onboarded', 'no', 0.49, 0.27),
    ('churned', 'yes', 0.18, 'onboarded', 'yes', 0.51, 0.73),
    ('churned', 'yes', 0.18, 'plan', 'Starter', 0.57, 0.47),
    ('churned', 'yes', 0.18, 'plan', 'Team', 0.33, 0.37),
    ('churned', 'yes', 0.18, 'plan', 'Business', 0.10, 0.16),
    ('churned', 'yes', 0.18, 'region', 'DACH', 0.49, 0.50),
    ('churned', 'yes', 0.18, 'region', 'Benelux', 0.30, 0.30),
    ('churned', 'yes', 0.18, 'region', 'Nordics', 0.21, 0.20),
    ('plan', 'Business', 0.15, 'contract', 'monthly', 0.28, 0.56),
    ('plan', 'Business', 0.15, 'contract', 'annual', 0.72, 0.44),
    ('plan', 'Business', 0.15, 'tenure', 'under 1 year', 0.12, 0.27),
    ('plan', 'Business', 0.15, 'tenure', '1–3 years', 0.38, 0.40),
    ('plan', 'Business', 0.15, 'tenure', 'over 3 years', 0.50, 0.33),
    ('plan', 'Business', 0.15, 'support tickets', 'none', 0.30, 0.51),
    ('plan', 'Business', 0.15, 'support tickets', '1–2', 0.38, 0.34),
    ('plan', 'Business', 0.15, 'support tickets', '3 or more', 0.32, 0.15),
    ('plan', 'Business', 0.15, 'usage', 'falling', 0.20, 0.25),
    ('plan', 'Business', 0.15, 'usage', 'steady', 0.52, 0.50),
    ('plan', 'Business', 0.15, 'usage', 'rising', 0.28, 0.25),
    ('plan', 'Business', 0.15, 'payment failed', 'yes', 0.03, 0.08),
    ('plan', 'Business', 0.15, 'payment failed', 'no', 0.97, 0.92),
    ('plan', 'Business', 0.15, 'onboarded', 'no', 0.15, 0.33),
    ('plan', 'Business', 0.15, 'onboarded', 'yes', 0.85, 0.67),
    ('plan', 'Business', 0.15, 'churned', 'yes', 0.12, 0.19),
    ('plan', 'Business', 0.15, 'churned', 'no', 0.88, 0.81),
    ('plan', 'Business', 0.15, 'region', 'DACH', 0.55, 0.49),
    ('plan', 'Business', 0.15, 'region', 'Benelux', 0.28, 0.30),
    ('plan', 'Business', 0.15, 'region', 'Nordics', 0.17, 0.21)
)
SELECT target, target_state, node, state,
       p_target * p_given_target + (1 - p_target) * p_given_not_target AS p_state,
       p_given_target, p_given_not_target,
       p_target * p_given_target / (p_target * p_given_target + (1 - p_target) * p_given_not_target) AS p_target_given_state,
       p_given_target - p_given_not_target AS difference,
       p_given_target / (p_target * p_given_target + (1 - p_target) * p_given_not_target) AS lift,
       abs(p_given_target - p_given_not_target) AS distance,
       row_number() OVER (PARTITION BY target, target_state ORDER BY abs(p_given_target - p_given_not_target) DESC) AS rank
FROM g;

-- How strongly each pair of variables hangs together: mutual information
-- (bits) and normalized to 0–1.
CREATE OR REPLACE TABLE bn_associations AS
SELECT a, b, mutual_information, round(mutual_information / 0.25, 3) AS normalized FROM (VALUES
    ('churned', 'usage', 0.142), ('churned', 'support tickets', 0.118), ('churned', 'contract', 0.087),
    ('churned', 'tenure', 0.061), ('churned', 'payment failed', 0.032), ('churned', 'onboarded', 0.024),
    ('churned', 'plan', 0.006), ('churned', 'region', 0.001),
    ('usage', 'support tickets', 0.165), ('usage', 'onboarded', 0.071), ('usage', 'contract', 0.018),
    ('usage', 'tenure', 0.012), ('usage', 'payment failed', 0.004), ('usage', 'plan', 0.009), ('usage', 'region', 0.002),
    ('support tickets', 'onboarded', 0.083), ('support tickets', 'plan', 0.041), ('support tickets', 'contract', 0.011),
    ('support tickets', 'tenure', 0.008), ('support tickets', 'payment failed', 0.003), ('support tickets', 'region', 0.001),
    ('contract', 'tenure', 0.204), ('contract', 'plan', 0.131), ('contract', 'payment failed', 0.027),
    ('contract', 'onboarded', 0.006), ('contract', 'region', 0.004),
    ('tenure', 'plan', 0.096), ('tenure', 'payment failed', 0.019), ('tenure', 'onboarded', 0.005), ('tenure', 'region', 0.003),
    ('plan', 'payment failed', 0.012), ('plan', 'onboarded', 0.022), ('plan', 'region', 0.016),
    ('payment failed', 'onboarded', 0.002), ('payment failed', 'region', 0.001), ('onboarded', 'region', 0.002)
) t(a, b, mutual_information);

-- What each unobserved variable would tell about the hypothesis (bits), and
-- what finding it out costs (minutes of an account manager's time).
CREATE OR REPLACE TABLE bn_value_of_information AS
SELECT hypothesis, test, mutual_information, cost, round(mutual_information / cost, 4) AS score FROM (VALUES
    ('churned = yes', 'usage', 0.142, 2.0), ('churned = yes', 'support tickets', 0.118, 1.0),
    ('churned = yes', 'contract', 0.087, 0.5), ('churned = yes', 'tenure', 0.061, 0.5),
    ('churned = yes', 'payment failed', 0.032, 1.0), ('churned = yes', 'onboarded', 0.024, 5.0),
    ('churned = yes', 'plan', 0.006, 0.5), ('churned = yes', 'region', 0.001, 0.5),
    ('churned = yes', 'exit survey', 0.171, 15.0), ('churned = yes', 'account review call', 0.158, 30.0),
    ('plan = Business', 'contract', 0.071, 0.5), ('plan = Business', 'tenure', 0.033, 0.5),
    ('plan = Business', 'onboarded', 0.029, 5.0), ('plan = Business', 'support tickets', 0.024, 1.0),
    ('plan = Business', 'churned', 0.006, 0.5), ('plan = Business', 'payment failed', 0.004, 1.0),
    ('plan = Business', 'usage', 0.002, 2.0), ('plan = Business', 'region', 0.003, 0.5),
    ('plan = Business', 'account review call', 0.094, 30.0)
) t(hypothesis, test, mutual_information, cost);

-- One customer's case (C-01873, who cancelled): the evidence, its most
-- probable explanations, how surprising each observation is, the churn
-- probability as the evidence comes in, and the evidence's conflict.
CREATE OR REPLACE TABLE bn_explanations AS
SELECT * FROM (VALUES
    (1, 0.214, [{'node': 'usage', 'state': 'falling'}, {'node': 'support tickets', 'state': '3 or more'}, {'node': 'onboarded', 'state': 'no'}]),
    (2, 0.163, [{'node': 'usage', 'state': 'falling'}, {'node': 'support tickets', 'state': '1–2'}, {'node': 'onboarded', 'state': 'no'}]),
    (3, 0.118, [{'node': 'usage', 'state': 'falling'}, {'node': 'support tickets', 'state': '3 or more'}, {'node': 'onboarded', 'state': 'yes'}]),
    (4, 0.071, [{'node': 'usage', 'state': 'steady'}, {'node': 'support tickets', 'state': '3 or more'}, {'node': 'onboarded', 'state': 'no'}]),
    (5, 0.052, [{'node': 'usage', 'state': 'steady'}, {'node': 'support tickets', 'state': '1–2'}, {'node': 'onboarded', 'state': 'yes'}])
) t(rank, probability, explanation);

CREATE OR REPLACE TABLE bn_retracted AS
SELECT node, observed_state, p AS p_observed_given_rest, round(-log2(p), 2) AS surprise FROM (VALUES
    ('contract', 'annual', 0.09), ('tenure', 'over 3 years', 0.21), ('plan', 'Business', 0.38),
    ('region', 'DACH', 0.49), ('payment failed', 'no', 0.93), ('churned', 'yes', 0.27)
) t(node, observed_state, p);

CREATE OR REPLACE TABLE bn_impact AS
SELECT * FROM (VALUES
    ('churned = yes', 'contract = annual', 0.18, 0.06),
    ('churned = yes', '+ tenure = over 3 years', 0.06, 0.04),
    ('churned = yes', '+ plan = Business', 0.04, 0.035),
    ('churned = yes', '+ payment failed = no', 0.035, 0.03)
) t(hypothesis_state, evidence_subset, p_without, p_with);

CREATE OR REPLACE TABLE bn_conflict AS
SELECT * FROM (VALUES ('all evidence', 1.9), ('without contract', -0.3)) t(subset, conflict);

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'What sets churners apart?'::DASHBOARD_TITLE;

-- ── 1 · Drivers ──────────────────────────────────────────────────────────────
SELECT 'Drivers'::TAB;

SELECT 'Target'::GROUP;
SELECT target || ' = ' || target_state AS bn_target ::DROPDOWN, 'What sets apart …'::LABEL
FROM (SELECT DISTINCT target, target_state FROM bn_insight) ORDER BY target = 'churned' DESC, 1;
SELECT 1::ENDGROUP;

-- #1275: the learned network (anofox_plot_bayes_net over the producer's
-- network tables, the target highlighted) goes here, above the drivers.

SELECT 'At a glance'::GROUP;
SELECT round(100 * sum(p_target_given_state * p_state), 1)::PERCENT, 'of all customers are in the target state'::LABEL
FROM bn_insight
WHERE target || ' = ' || target_state = getvariable('bn_target')
  AND node = (SELECT min(node) FROM bn_insight WHERE target || ' = ' || target_state = getvariable('bn_target'));
SELECT node || ' = ' || state ::TEXT_LARGE, 'strongest driver (× ' || printf('%.1f', lift) || ' as likely in the target)'::LABEL
FROM bn_insight WHERE target || ' = ' || target_state = getvariable('bn_target') ORDER BY lift DESC LIMIT 1;
SELECT test ::TEXT_LARGE, 'best next question (most information per minute)'::LABEL
FROM bn_value_of_information WHERE hypothesis = getvariable('bn_target') ORDER BY score DESC LIMIT 1;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT bn_sort ::BUTTONS, 'Sort by'::LABEL FROM (VALUES ('lift'), ('distance')) t(bn_sort);
SELECT 460::HEIGHT;
SELECT node || ' = ' || state ::DUMBBELL,
       100 * p_given_not_target AS "not in the target" ::XMIN, 100 * p_given_target AS "in the target" ::XMAX,
       '×' || printf('%.1f', lift) ::LABEL, 'percent'::YFORMAT,
       'Drivers: how common each state is in the target and outside it'::TITLE,
       'The label: how many times as likely the state is in the target (lift)'::SUBTITLE
FROM bn_insight
WHERE target || ' = ' || target_state = getvariable('bn_target') AND rank <= 14
ORDER BY CASE WHEN getvariable('bn_sort') = 'distance' THEN distance ELSE lift END DESC;

SELECT 6::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_matrix(t, row := 'a', col := 'b', value := 'normalized', cluster := true, domain := [0, 1]),
       'Which variables hang together'::TITLE, 'Normalized mutual information, 0 = independent; related variables side by side'::SUBTITLE
FROM bn_associations t;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT test ::XAXIS, mutual_information AS "information (bits)" ::BARCHART, 1::FLIP,
       'Next question: what each answer would tell'::TITLE
FROM bn_value_of_information WHERE hypothesis = getvariable('bn_target')
ORDER BY mutual_information DESC;

SELECT 6::COL;
SELECT 340::HEIGHT;
WITH v AS (
    SELECT *, mutual_information > coalesce(max(mutual_information) OVER (
               ORDER BY cost, mutual_information DESC ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING), -1) AS efficient
    FROM bn_value_of_information WHERE hypothesis = getvariable('bn_target')
)
SELECT cost ::XAXIS, mutual_information AS "question" ::SCATTER,
       CASE WHEN efficient THEN mutual_information END AS "efficient frontier" ::LINECHART,
       CASE WHEN efficient THEN test END ::LABEL, test AS "Question" ::TOOLTIP,
       'log10'::XSCALE, 'cost (minutes, log scale)'::XTITLE, 'information (bits)'::YTITLE,
       'Information against cost — ask along the frontier'::TITLE
FROM v ORDER BY cost;

-- ── 2 · One customer ─────────────────────────────────────────────────────────
SELECT 'One customer'::TAB;

SELECT 12::COL;
SELECT '**Customer C-01873 cancelled in Q3.** The network expected them to stay: an annual contract, over three years with us, the Business plan. What explains the cancellation, and which observation does not fit?'::MARKDOWN,
       'The case'::TITLE;

SELECT 6::COL;
SELECT 340::HEIGHT;
SELECT step ::XAXIS, v ::WATERFALL, 'percent'::YFORMAT,
       'P(churn) as each piece of evidence comes in'::TITLE
FROM (
    SELECT 0 AS k, 'before evidence' AS step, 100 * p_without AS v FROM bn_impact WHERE evidence_subset NOT LIKE '+%'
    UNION ALL
    SELECT row_number() OVER (ORDER BY evidence_subset NOT LIKE '+%' DESC, p_without DESC), evidence_subset, 100 * (p_with - p_without)
    FROM bn_impact
    UNION ALL
    SELECT 99, 'with all evidence', NULL
) ORDER BY k;

SELECT 3::COL;
SELECT 340::HEIGHT;
SELECT conflict ::GAUGE, '-3,3'::RANGE, '<0:good; <1:warn; bad'::THRESHOLDS,
       'Conflict in the evidence'::TITLE,
       'Above 0 the observations point in different directions: log₂ of P(each alone) over P(all together).'::INFO
FROM bn_conflict WHERE subset = 'all evidence';

SELECT 3::COL;
SELECT 340::HEIGHT;
SELECT 'none'::TOOLBAR;
SELECT subset AS "Evidence" ::TABLE, round(conflict, 2) AS "Conflict (bits)", 'Conflict without each part'::TITLE
FROM bn_conflict ORDER BY conflict DESC;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT node || ' = ' || observed_state ::XAXIS,
       CASE WHEN surprise >= 3 THEN 'very surprising' WHEN surprise >= 1.5 THEN 'surprising' ELSE 'expected' END ::CATEGORY,
       surprise AS "surprise (bits)" ::BARCHART_STACKED, 1::FLIP,
       'very surprising:bad; surprising:warn; expected:good'::COLORS,
       'Which observation does not fit'::TITLE, 'Each observation predicted from all the others: −log₂ P'::SUBTITLE
FROM bn_retracted ORDER BY surprise DESC;

SELECT 6::COL;
SELECT 'none'::TOOLBAR;
SELECT 'Most probable explanations'::TITLE,
       rank AS "#" ::TABLE,
       list_aggregate(list_transform(explanation, lambda e: e.node || ' = ' || e.state), 'string_agg', ' · ') AS "Explanation",
       round(100 * probability, 1) AS "Probability (%)"
FROM bn_explanations ORDER BY rank;
