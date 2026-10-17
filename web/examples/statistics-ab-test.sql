-- @title Did the new checkout lift conversion?
-- @group Sales & customers
-- @tags  statistics, ab-test, conversion
--
-- A/B test readout — did the new one-page checkout lift conversion?
--
-- LIVE: every number below is computed when the dashboard runs. The script
-- builds a synthetic, deterministic experiment with plain SQL (hash-based
-- noise, no files) and the anofox_statistics extension does the tests:
-- prop_test_two_agg (conversion lift + 95% CI), prop_test_one_agg (rate per
-- arm), t_test_agg (revenue per visitor), tost_t_test_agg (page-load
-- equivalence), chisq_gof_agg (sample-ratio mismatch), logistic_tidy_by
-- (variant × device interaction) and eb_shrink_by (shrunken segment lifts).
-- Power and sample size use the normal approximation in plain SQL (the
-- extension has no power function).
--   * Browser demo: DuckDB-Wasm + the anofox_statistics wasm extension.
--   * Natively: `dashboard --check examples/statistics-ab-test/showcase.sql`
--     with a duckdb that has anofox_statistics installed (see its README).
--
-- Tabs: Readout · Segments · Guardrails · Power & sample size · Decision.

LOAD anofox_statistics;

SELECT 'Did the new checkout lift conversion?'::DASHBOARD_TITLE;
SELECT 'anofox_statistics'::BRAND;

-- ─────────────────────────────────────────────────────────────────────────────
-- Data (setup statements: plain SQL, no files)
-- ─────────────────────────────────────────────────────────────────────────────
-- Uniform(0,1) from the md5 of a key tuple; a standard normal via Box–Muller.
CREATE OR REPLACE MACRO ab_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE OR REPLACE MACRO ab_rnorm(a, b, c) AS
    sqrt(-2.0 * ln(ab_unif(a, b, c || ':u1'))) * cos(2.0 * pi() * ab_unif(a, b, c || ':u2'));
-- Standard normal CDF (Abramowitz–Stegun 7.1.26, |error| < 1.5e-7) for power.
CREATE OR REPLACE MACRO ab_pnorm(x) AS
    0.5 * (1 + sign(x) * (1 - (0.254829592 * (1 / (1 + 0.3275911 * abs(x) / sqrt(2)))
                              - 0.284496736 * power(1 / (1 + 0.3275911 * abs(x) / sqrt(2)), 2)
                              + 1.421413741 * power(1 / (1 + 0.3275911 * abs(x) / sqrt(2)), 3)
                              - 1.453152027 * power(1 / (1 + 0.3275911 * abs(x) / sqrt(2)), 4)
                              + 1.061405429 * power(1 / (1 + 0.3275911 * abs(x) / sqrt(2)), 5))
                             * exp(-x * x / 2)));

-- ab_users — 96,000 visitors who reached the basket, 1–21 Sep 2026, split
-- 50/50 between A (today's three-step checkout) and B (the new one-page
-- checkout). Base conversion depends on device, new vs returning visitor and
-- channel (≈ 4% overall). The planted truth: B lifts mobile by +0.85 pp,
-- tablet by +0.4 pp and desktop by almost nothing (+0.08 pp); the lift is half
-- as large again in the first three days (novelty). B's page is ~38 ms slower,
-- cuts payment errors from 2.1% to 1.6%, and leaves support contacts, order
-- value (B −1.20 €, noise) and refunds (6.1% of orders) about where they were.
CREATE OR REPLACE TABLE ab_users AS
WITH u AS (
    SELECT i,
           DATE '2026-09-01' + floor(21 * ab_unif('ab', i, 'day'))::INTEGER AS first_day,
           CASE WHEN ab_unif('ab', i, 'arm') < 0.5 THEN 'B' ELSE 'A' END AS variant,
           CASE WHEN ab_unif('ab', i, 'dev') < 0.58 THEN 'mobile'
                WHEN ab_unif('ab', i, 'dev') < 0.93 THEN 'desktop' ELSE 'tablet' END AS device,
           CASE WHEN ab_unif('ab', i, 'ret') < 0.42 THEN 'new' ELSE 'returning' END AS visitor,
           CASE WHEN ab_unif('ab', i, 'ch') < 0.38 THEN 'paid search' WHEN ab_unif('ab', i, 'ch') < 0.66 THEN 'organic'
                WHEN ab_unif('ab', i, 'ch') < 0.86 THEN 'email' ELSE 'social' END AS channel
    FROM range(1, 96001) r(i)
), p AS (
    SELECT u.*,
           (CASE device WHEN 'desktop' THEN 0.052 WHEN 'mobile' THEN 0.030 ELSE 0.038 END)
             * (CASE visitor WHEN 'returning' THEN 1.25 ELSE 0.70 END)
             * (CASE channel WHEN 'email' THEN 1.45 WHEN 'paid search' THEN 1.05 WHEN 'social' THEN 0.60 ELSE 0.95 END) AS p0,
           CASE WHEN variant = 'B' THEN
               (CASE device WHEN 'mobile' THEN 0.0085 WHEN 'tablet' THEN 0.004 ELSE 0.0008 END)
               * (1 + 0.5 * greatest(0, 3 - (first_day - DATE '2026-09-01')) / 3.0) ELSE 0 END AS lift
    FROM u
), c AS (
    SELECT p.*, (ab_unif('ab', i, 'conv') < p0 + lift)::INTEGER AS converted FROM p
)
SELECT i::INTEGER AS user_id, first_day, variant, device, visitor, channel, converted,
       CASE WHEN converted = 1
            THEN round(exp(ln(62) + 0.55 * ab_rnorm('ab', i, 'aov')) - CASE WHEN variant = 'B' THEN 1.2 ELSE 0 END, 2)
            ELSE 0 END::DOUBLE AS revenue,
       round(1850 + CASE device WHEN 'mobile' THEN 420 ELSE 0 END + CASE WHEN variant = 'B' THEN 38 ELSE 0 END
             + 380 * ab_rnorm('ab', i, 'load'))::DOUBLE AS page_load_ms,
       (ab_unif('ab', i, 'pay') < CASE WHEN variant = 'B' THEN 0.016 ELSE 0.021 END)::INTEGER AS payment_error,
       (ab_unif('ab', i, 'sup') < CASE WHEN variant = 'B' THEN 0.0125 ELSE 0.0118 END)::INTEGER AS support_contact,
       CASE WHEN converted = 1 THEN (ab_unif('ab', i, 'ref') < 0.061)::INTEGER END AS refunded,
       -- 0 = B, 1 = A: the two-sample tests then report B − A
       (variant = 'A')::INTEGER AS arm
FROM c ORDER BY i;

-- For the interaction model: 0/1 columns (desktop and new visitors are the reference).
CREATE OR REPLACE TABLE ab_terms AS
SELECT 'all' AS grp, converted::DOUBLE AS converted_d, (variant = 'B')::DOUBLE AS is_b,
       (device = 'mobile')::DOUBLE AS mobile, (device = 'tablet')::DOUBLE AS tablet,
       (visitor = 'returning')::DOUBLE AS returning_v,
       (variant = 'B' AND device = 'mobile')::DOUBLE AS b_mobile, (variant = 'B' AND device = 'tablet')::DOUBLE AS b_tablet
FROM ab_users;
-- Lift per device × visitor cell with its standard error (from the 95% CI).
CREATE OR REPLACE TABLE ab_cells AS
SELECT device || ' · ' || visitor AS cell, count(*) AS n,
       (prop_test_two_agg(converted::BIGINT, arm::BIGINT)).estimate AS est,
       ((prop_test_two_agg(converted::BIGINT, arm::BIGINT)).conf_high
        - (prop_test_two_agg(converted::BIGINT, arm::BIGINT)).conf_low) / (2 * 1.959964) AS se
FROM ab_users GROUP BY device, visitor;

-- The inputs (strings from the controls → typed values).
CREATE OR REPLACE MACRO ab_dev() AS coalesce(getvariable('device'), 'all devices');
CREATE OR REPLACE MACRO ab_mde() AS coalesce(TRY_CAST(getvariable('min_lift_pp') AS DOUBLE), 0.4) / 100;
CREATE OR REPLACE MACRO ab_tol() AS coalesce(TRY_CAST(getvariable('load_tolerance_ms') AS DOUBLE), 100);

-- The visitors of the chosen device (all of them by default).
CREATE OR REPLACE VIEW ab_view AS SELECT * FROM ab_users WHERE ab_dev() IN ('all devices', device);

-- The headline tests, once per run.
CREATE OR REPLACE VIEW ab_head AS
SELECT prop_test_two_agg(converted::BIGINT, arm::BIGINT) AS conv,
       t_test_agg(revenue, arm::BIGINT) AS rpv,
       avg(converted) FILTER (WHERE variant = 'A') AS p_a,
       avg(converted) FILTER (WHERE variant = 'B') AS p_b,
       avg(revenue) FILTER (WHERE variant = 'A') AS rpv_a,
       avg(revenue) FILTER (WHERE variant = 'B') AS rpv_b,
       count(*) FILTER (WHERE variant = 'A') AS n_a,
       count(*) FILTER (WHERE variant = 'B') AS n_b
FROM ab_view;

-- Guardrails: one row per metric — B − A, its CI and a three-state rule:
-- "fail" when B is worse beyond doubt (the whole CI on the harmful side),
-- "watch" when harm beyond the margin is not ruled out, else "pass". Page load
-- uses an equivalence test (TOST) against the tolerance input.
CREATE OR REPLACE VIEW ab_guard AS
WITH g AS (
    SELECT tost_t_test_agg(page_load_ms, arm, {'delta': ab_tol()}) AS load_tost,
           prop_test_two_agg(payment_error::BIGINT, arm::BIGINT) AS pay,
           prop_test_two_agg(support_contact::BIGINT, arm::BIGINT) AS sup,
           prop_test_two_agg(refunded::BIGINT, arm::BIGINT) FILTER (WHERE converted = 1) AS ref,
           t_test_agg(revenue, arm::BIGINT) FILTER (WHERE converted = 1) AS aov,
           avg(page_load_ms) FILTER (WHERE variant = 'A') AS load_a, avg(page_load_ms) FILTER (WHERE variant = 'B') AS load_b,
           avg(payment_error) FILTER (WHERE variant = 'A') AS pay_a, avg(payment_error) FILTER (WHERE variant = 'B') AS pay_b,
           avg(support_contact) FILTER (WHERE variant = 'A') AS sup_a, avg(support_contact) FILTER (WHERE variant = 'B') AS sup_b,
           avg(refunded) FILTER (WHERE variant = 'A') AS ref_a, avg(refunded) FILTER (WHERE variant = 'B') AS ref_b,
           avg(revenue) FILTER (WHERE variant = 'A' AND converted = 1) AS aov_a,
           avg(revenue) FILTER (WHERE variant = 'B' AND converted = 1) AS aov_b
    FROM ab_view
), r AS (
    SELECT 1 AS k, 'Page load (ms)' AS metric, load_a AS a, load_b AS b, load_tost.estimate AS diff,
           load_tost.conf_low AS lo, load_tost.conf_high AS hi, load_tost.p_value AS p,
           'equivalent within ±' || ab_tol()::INTEGER || ' ms (TOST, 90% CI)' AS rule,
           CASE WHEN load_tost.equivalent THEN 'pass' WHEN load_tost.conf_low > ab_tol() THEN 'fail' ELSE 'watch' END AS verdict FROM g
    UNION ALL SELECT 2, 'Payment errors (%)', 100 * pay_a, 100 * pay_b, 100 * pay.estimate, 100 * pay.conf_low, 100 * pay.conf_high, pay.p_value,
           'not up by more than 0.2 pp',
           CASE WHEN pay.conf_low > 0 THEN 'fail' WHEN 100 * pay.conf_high > 0.2 THEN 'watch' ELSE 'pass' END FROM g
    UNION ALL SELECT 3, 'Support contacts (%)', 100 * sup_a, 100 * sup_b, 100 * sup.estimate, 100 * sup.conf_low, 100 * sup.conf_high, sup.p_value,
           'not up by more than 0.3 pp',
           CASE WHEN sup.conf_low > 0 THEN 'fail' WHEN 100 * sup.conf_high > 0.3 THEN 'watch' ELSE 'pass' END FROM g
    UNION ALL SELECT 4, 'Refunds (% of orders)', 100 * ref_a, 100 * ref_b, 100 * ref.estimate, 100 * ref.conf_low, 100 * ref.conf_high, ref.p_value,
           'not up by more than 3 pp',
           CASE WHEN ref.conf_low > 0 THEN 'fail' WHEN 100 * ref.conf_high > 3 THEN 'watch' ELSE 'pass' END FROM g
    UNION ALL SELECT 5, 'Order value (€)', aov_a, aov_b, aov.estimate, aov.conf_low, aov.conf_high, aov.p_value,
           'not down by more than 5 €',
           CASE WHEN aov.conf_high < 0 THEN 'fail' WHEN aov.conf_low < -5 THEN 'watch' ELSE 'pass' END FROM g
)
SELECT * FROM r;

-- The verdict: the pre-registered rule on the primary metric, the split and the guardrails.
CREATE OR REPLACE VIEW ab_verdict AS
SELECT h.*, s.srm_p, gd.n_fail, gd.n_watch, gd.watch_list,
       CASE WHEN s.srm_p < 0.001 THEN 'Stop: the split is broken'
            WHEN gd.n_fail > 0 THEN 'Don''t ship: a guardrail broke'
            WHEN h.conv.conf_low > 0
                 THEN CASE WHEN gd.n_watch > 0 THEN 'Ship, watch ' || gd.watch_list ELSE 'Ship' END
            WHEN h.conv.conf_high < ab_mde() THEN 'Don''t ship: no worthwhile lift'
            ELSE 'Keep running' END AS verdict
FROM ab_head h,
     (SELECT (chisq_gof_agg(n, 0.5)).p_value AS srm_p FROM (SELECT variant, count(*) AS n FROM ab_view GROUP BY variant)) s,
     (SELECT count(*) FILTER (WHERE verdict = 'fail') AS n_fail, count(*) FILTER (WHERE verdict = 'watch') AS n_watch,
             lower(string_agg(regexp_replace(metric, ' \(.*', ''), ', ' ORDER BY k) FILTER (WHERE verdict = 'watch')) AS watch_list
      FROM ab_guard) gd;

-- Power and sample size (normal approximation, two-sided α = 5%, 80% power):
-- n per arm to detect a lift d on a base rate p, and the power at n per arm.
CREATE OR REPLACE MACRO ab_n_needed(p, d) AS
    ceil(power(1.959964 + 0.841621, 2) * (p * (1 - p) + (p + d) * (1 - p - d)) / (d * d));
CREATE OR REPLACE MACRO ab_power(p, d, n) AS
    ab_pnorm(abs(d) / sqrt((p * (1 - p) + (p + d) * (1 - p - d)) / n) - 1.959964);

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
-- The controls: a slim bar on top, read by every tab.
SELECT 'Test settings (every tab reads them)'::GROUP;
SELECT device ::DROPDOWN, 'Device'::LABEL
FROM (VALUES (1, 'all devices'), (2, 'mobile'), (3, 'desktop'), (4, 'tablet')) t(k, device) ORDER BY k;
SELECT 0.4 AS min_lift_pp ::NUMBER, 'Lift the test was sized for (pp)'::LABEL;
SELECT 100 AS load_tolerance_ms ::NUMBER, 'Page-load tolerance (± ms)'::LABEL;
SELECT 1::ENDGROUP;

-- ── 1 · Readout ──────────────────────────────────────────────────────────────
SELECT 'Readout'::TAB;

SELECT 12::COL;
SELECT '**Who asks:** the head of e-commerce, before the Q4 release freeze — *ship the one-page checkout (B) to everyone, or keep the three-step checkout (A)?* **The test:** 96,000 visitors who reached the basket, 1–21 September 2026, randomly split 50/50, sized to see a lift of **+0.4 pp** (≈ 10% relative) with 80% power. **Primary metric:** conversion (order placed). **Pre-registered rule:** ship if the 95% interval of the lift lies above zero, the split is healthy and no guardrail breaks. The settings above re-run every test.'::MARKDOWN,
       'The question'::TITLE;

SELECT 'Headline: B against A'::GROUP;
SELECT round(100 * p_b, 2)::PERCENT, round(100 * p_a, 2)::DELTA, 'Conversion B (arrow vs A)'::LABEL FROM ab_head;
SELECT round(100 * conv.estimate, 2)::METRIC,
       ('Lift (pp), 95% CI ' || printf('%+.2f', 100 * conv.conf_low) || ' to ' || printf('%+.2f', 100 * conv.conf_high))::LABEL,
       (CASE WHEN conv.conf_low > 0 THEN 'good' WHEN conv.conf_high < 0 THEN 'bad' ELSE 'warn' END)::THRESHOLDS,
       'Conversion of B minus conversion of A, in percentage points (pp): +0.50 pp lifts 4.00 % to 4.50 %. Green when the whole 95 % interval lies above zero (two-sample proportion test).'::INFO
FROM ab_head;
SELECT round(rpv_b, 2)::METRIC, round(rpv_a, 2)::DELTA, 'Revenue per visitor B (€, arrow vs A)'::LABEL FROM ab_head;
SELECT printf('%.2g', conv.p_value)::TEXT_LARGE, 'p-value of the lift'::LABEL,
       'The chance of a lift at least this large if B truly made no difference. Below 0.05 counts as significant here; it says nothing about how large the lift is — read the interval for that.'::INFO
FROM ab_head;
SELECT verdict::TEXT_LARGE, 'Verdict (pre-registered rule)'::LABEL FROM ab_verdict;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 340::HEIGHT;
WITH days AS (SELECT DISTINCT first_day AS d FROM ab_view),
cum AS (
    SELECT days.d, prop_test_two_agg(v.converted::BIGINT, v.arm::BIGINT) AS r
    FROM days JOIN ab_view v ON v.first_day <= days.d GROUP BY days.d
)
SELECT d::XAXIS, round(100 * r.estimate, 3) AS "lift (pp)" ::LINECHART,
       round(100 * r.conf_low, 3)::BAND_LOWER, round(100 * r.conf_high, 3)::BAND_UPPER,
       0::REFLINE, 'no effect'::REFLINE_LABEL,
       CASE d WHEN DATE '2026-09-01' THEN 'Day 1: peeking now would claim a lift of ' || printf('%+.1f', 100 * r.estimate) || ' pp — the novelty bump of new visitors, on a tiny sample'
              WHEN DATE '2026-09-04' THEN 'Day 4: the novelty fades and the interval narrows as visitors accumulate'
              WHEN DATE '2026-09-21' THEN 'Day 21: the lift settles at ' || printf('%+.2f', 100 * r.estimate) || ' pp, its 95 % interval (' || printf('%+.2f', 100 * r.conf_low) || ' to ' || printf('%+.2f', 100 * r.conf_high) || ') clear of zero'
       END::STORY,
       'Cumulative lift with its 95% CI — peeking on day 1 would have claimed +1.6 pp; the lift settles near +0.5 pp, clear of zero'::TITLE
FROM cum ORDER BY d;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT variant::XAXIS, round(100 * r.estimate, 2) AS "conversion (%)" ::BARCHART,
       round(100 * r.conf_low, 2)::YMIN, round(100 * r.conf_high, 2)::YMAX,
       'Conversion by arm, % (95% CI) — B converts better'::TITLE
FROM (SELECT variant, prop_test_one_agg(converted::BIGINT) AS r FROM ab_view GROUP BY variant)
ORDER BY variant;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT variant::XAXIS, round(avg(revenue), 3) AS "revenue per visitor (€)" ::BARCHART,
       round(avg(revenue) - 1.96 * stddev_samp(revenue) / sqrt(count(*)), 3)::YMIN,
       round(avg(revenue) + 1.96 * stddev_samp(revenue) / sqrt(count(*)), 3)::YMAX,
       '€'::YFORMAT,
       'Revenue per visitor (95% CI) — more orders, about the same basket'::TITLE
FROM ab_view GROUP BY variant ORDER BY variant;

SELECT 12::COL;
SELECT ('### ' || verdict || E'\n\n'
        || 'B converts **' || printf('%.2f', 100 * p_b) || '%** against **' || printf('%.2f', 100 * p_a) || '%** for A: a lift of **'
        || printf('%+.2f', 100 * conv.estimate) || ' pp** (95% CI ' || printf('%+.2f', 100 * conv.conf_low) || ' to '
        || printf('%+.2f', 100 * conv.conf_high) || ' pp, p = ' || printf('%.2g', conv.p_value) || '), '
        || printf('%+.1f', 100 * conv.estimate / p_a) || '% relative. Revenue per visitor moves by **'
        || printf('%+.3f', rpv.estimate) || ' €** (p = ' || printf('%.2g', rpv.p_value) || '). '
        || CASE WHEN n_fail > 0 THEN n_fail || ' guardrail(s) **fail**'
                WHEN n_watch > 0 THEN 'No guardrail fails; ' || watch_list || ' could not be cleared yet (**watch**)'
                ELSE 'All five guardrails hold' END
        || ' and the split is ' || CASE WHEN srm_p < 0.001 THEN '**broken** (sample-ratio mismatch)' ELSE 'healthy (SRM p = ' || printf('%.2f', srm_p) || ')' END
        || '. Details in the next tabs; the rollout case in **Decision**.')::MARKDOWN,
       CASE WHEN verdict LIKE 'Ship%' THEN 'success' WHEN verdict = 'Keep running' THEN 'note' ELSE 'warning' END::CALLOUT,
       'Verdict'::TITLE
FROM ab_verdict;

-- ── 2 · Segments ─────────────────────────────────────────────────────────────
SELECT 'Segments'::TAB;

SELECT 12::COL;
SELECT '**Where does the lift come from?** One two-sample test per segment (`prop_test_two_agg` grouped by device, visitor type and channel, on all visitors whatever the device input). **What to look for:** intervals that cross zero are not significant at 5%. Mobile and tablet carry the effect; desktop shows none. But slicing a test nine ways finds a "significant" segment by chance (social looks negative, email flat — neither is): the **interaction** model below asks the right question — *does* the lift differ by device? — and the **empirical-Bayes** table pulls each noisy cell lift towards the overall one (`eb_shrink_by`): small tablet cells move most.'::MARKDOWN,
       'What to look for'::TITLE;

SELECT 12::COL;
SELECT 460::HEIGHT;
SELECT anofox_plot_tests(t, x_label := 'conversion lift B − A (pp, 95% CI)'),
       'Lift by segment — mobile and tablet carry the effect, desktop is flat'::TITLE
FROM (SELECT dim AS id, lvl AS test_id,
             100 * r.estimate AS estimate, 100 * r.conf_low AS conf_low, 100 * r.conf_high AS conf_high, r.p_value AS p_value,
             r.method AS method
      FROM (SELECT 'device' AS dim, device AS lvl, prop_test_two_agg(converted::BIGINT, arm::BIGINT) AS r FROM ab_users GROUP BY ALL
            UNION ALL
            SELECT 'visitor', visitor, prop_test_two_agg(converted::BIGINT, arm::BIGINT) FROM ab_users GROUP BY ALL
            UNION ALL
            SELECT 'channel', channel, prop_test_two_agg(converted::BIGINT, arm::BIGINT) FROM ab_users GROUP BY ALL)) t;

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT anofox_plot_terms(t, x_label := 'log-odds of converting'),
       'Logistic model: B alone (= desktop) is n.s. (faded); B × mobile and B × tablet carry the lift'::TITLE
FROM logistic_tidy_by('ab_terms', grp, converted_d,
                      [is_b, mobile, tablet, returning_v, b_mobile, b_tablet],
                      names := ['B', 'mobile', 'tablet', 'returning', 'B × mobile', 'B × tablet']) t
WHERE term <> '(Intercept)';

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT 'Device × visitor cells: raw vs shrunken lift'::TITLE,
       cell AS "Cell" ::TABLE, n AS "Visitors", round(100 * est, 2) AS "Raw lift (pp)" ::COLORSCALE_DIVERGING,
       round(100 * shrunken, 2) AS "Shrunken (pp)" ::COLORSCALE_DIVERGING, round(weight, 2) AS "Weight on own data" ::PLAIN
FROM eb_shrink_by('ab_cells', est, se)
ORDER BY shrunken DESC;

-- ── 3 · Guardrails ───────────────────────────────────────────────────────────
SELECT 'Guardrails'::TAB;

SELECT 12::COL;
SELECT '**Did B break anything?** A lift is only worth shipping if nothing important got worse. Each guardrail compares B with A and has a margin agreed before the test. **What to look for:** a guardrail **fails** when B is worse beyond doubt (its whole 95% interval on the harmful side), is on **watch** when harm beyond the margin is not yet ruled out, and **passes** otherwise. Page load uses an **equivalence** test (`tost_t_test_agg`): B is ~40 ms slower, well inside the tolerance. Payment errors *fall* (2.15% → 1.58%); support contacts (+0.13 pp, p = 0.06) and refunds (+1.2 pp, p = 0.12) tick up without being significant, both inside their margins — worth an alert after launch. The **split** itself is checked with a χ² goodness-of-fit test (`chisq_gof_agg`) against 50/50: a sample-ratio mismatch would void the whole readout.'::MARKDOWN,
       'What to look for'::TITLE;

SELECT 'Guardrails at a glance'::GROUP;
SELECT count(*) FILTER (WHERE verdict = 'pass')::METRIC, 'guardrails pass (of 5)'::LABEL FROM ab_guard;
SELECT count(*) FILTER (WHERE verdict = 'watch')::METRIC, 'on watch'::LABEL FROM ab_guard;
SELECT count(*) FILTER (WHERE verdict = 'fail')::METRIC, 'fail'::LABEL FROM ab_guard;
SELECT printf('%.2f', srm_p)::TEXT_LARGE, ('Sample-ratio p (' || n_b || ' B vs ' || n_a || ' A)')::LABEL,
       'Sample-ratio mismatch: a χ² goodness-of-fit test of the visitor counts against the planned 50/50 split. A p below 0.001 means the randomisation or the tracking is broken, and the readout cannot be trusted.'::INFO
FROM ab_verdict;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 'verdict = fail, verdict = watch'::FLAG;
SELECT 'Guardrail tests, B − A (the rule was fixed before the test)'::TITLE,
       metric AS "Guardrail" ::TABLE, round(a, 2) AS "A" ::PLAIN, round(b, 2) AS "B" ::PLAIN,
       round(diff, 2) AS "B − A" ::COLORSCALE_DIVERGING,
       printf('%+.2f to %+.2f', lo, hi) AS "CI",
       printf('%.2g', p) AS "p-value", rule AS "Rule", verdict AS "Result" ::BADGE
FROM ab_guard ORDER BY k;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_tests(t, x_label := 'B − A (pp, 95% CI)'),
       'Rates: payment errors fall, support contacts and refunds unchanged within noise'::TITLE
FROM (SELECT metric AS test_id, diff AS estimate, lo AS conf_low, hi AS conf_high,
             p AS p_value, 'two-sample proportion test' AS method
      FROM ab_guard WHERE k IN (2, 3, 4)) t;

SELECT 6::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_compare(r, value := 'page_load_ms', factor := 'variant', kind := 'violin',
                           tests := t_test_agg(page_load_ms, arm), y_label := 'page load (ms)'),
       'Page load — B is ~40 ms slower: detectable, but well inside the tolerance'::TITLE
FROM (SELECT variant, page_load_ms, arm FROM ab_view WHERE user_id % 16 = 0) r;

-- ── 4 · Power & sample size ──────────────────────────────────────────────────
SELECT 'Power & sample size'::TAB;

SELECT 12::COL;
SELECT '**Was the test big enough?** A test that cannot see the lift that matters can only say "no evidence", never "no effect". With base conversion $p$ and a lift $d$, each arm needs

$$n = \frac{(z_{0.975} + z_{0.8})^2 \, s^2}{d^2}$$

$$s^2 = p(1-p) + (p+d)(1-p-d)$$

visitors for 80% power at α = 5% (normal approximation). **What to look for:** the power at the lift the test was sized for (input above), and how many days the test would have needed at the current traffic. Change the device: desktop alone has a third of the traffic, so on its own it is badly underpowered for the same lift — its "no effect" is really "not enough data".'::MARKDOWN,
       'What to look for'::TITLE;

SELECT 'Design check (for the lift the test was sized for)'::GROUP;
SELECT round(100 * ab_power(p_a, ab_mde(), least(n_a, n_b)), 0)::METRIC,
       ('power at +' || printf('%.2f', 100 * ab_mde()) || ' pp (%)')::LABEL, '<50:bad; <80:warn; good'::THRESHOLDS,
       'Power: the chance that this test detects the lift if it is real. Below 80 %, a "no significant lift" is more likely a test that was too small than a checkout that does not work.'::INFO
FROM ab_head;
SELECT ab_n_needed(p_a, ab_mde())::COMPACT, 'visitors per arm needed (80% power)'::LABEL FROM ab_head;
SELECT least(n_a, n_b)::COMPACT, ab_n_needed(p_a, ab_mde())::DELTA, 'visitors per arm in the test (arrow vs needed)'::LABEL FROM ab_head;
SELECT ceil(2 * ab_n_needed(p_a, ab_mde()) / ((n_a + n_b) / 21.0))::METRIC, 'days needed at today''s traffic'::LABEL FROM ab_head;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT round(d / 100, 2) AS "true lift (pp)" ::XAXIS,
       round(100 * ab_power(p_a, d / 10000.0, least(n_a, n_b)), 1) AS "power (%)" ::LINECHART,
       80::REFLINE, round(100 * ab_mde(), 2)::XLINE,
       'Power of this test by true lift — about 87% at the +0.4 pp it was sized for (dashed)'::TITLE
FROM ab_head, range(5, 101, 5) r(d) ORDER BY d;

SELECT 6::COL;
SELECT 360::HEIGHT;
SELECT round(d / 100, 2) AS "lift to detect (pp)" ::XAXIS,
       ceil(2 * ab_n_needed(p_a, d / 10000.0) / ((n_a + n_b) / 21.0)) AS "days needed" ::LINECHART,
       21::REFLINE, round(100 * ab_mde(), 2)::XLINE,
       'Days of traffic needed for 80% power — halving the lift quadruples the test (21 days ran)'::TITLE
FROM ab_head, range(15, 101, 5) r(d) ORDER BY d;

SELECT 'days_needed > 28'::FLAG;
SELECT 'Sample size by lift, at today''s traffic'::TITLE,
       printf('+%.2f pp', d / 100) AS "Lift to detect" ::TABLE,
       round(100 * d / 100 / (100 * p_a), 1) AS "Relative (%)" ::PLAIN,
       ab_n_needed(p_a, d / 10000.0) AS "Visitors per arm" ::PLAIN,
       ceil(2 * ab_n_needed(p_a, d / 10000.0) / ((n_a + n_b) / 21.0)) AS days_needed,
       round(100 * ab_power(p_a, d / 10000.0, least(n_a, n_b)), 0) AS "Power of this test (%)" ::COLORSCALE_HIGH_GOOD
FROM ab_head, (VALUES (10), (15), (20), (25), (30), (40), (50), (75), (100)) v(d) ORDER BY d;

-- ── 5 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

SELECT 'If we roll B out (per month, ≈ 137,000 basket visitors)'::GROUP;
SELECT round(conv.estimate * (n_a + n_b) / 21.0 * 30)::METRIC,
       ('extra orders, 95% CI ' || round(conv.conf_low * (n_a + n_b) / 21.0 * 30)::INTEGER || ' to '
        || round(conv.conf_high * (n_a + n_b) / 21.0 * 30)::INTEGER)::LABEL FROM ab_head;
SELECT round(rpv.estimate * (n_a + n_b) / 21.0 * 30)::METRIC,
       ('extra revenue (€), 95% CI ' || printf('%,.0f', rpv.conf_low * (n_a + n_b) / 21.0 * 30) || ' to '
        || printf('%,.0f', rpv.conf_high * (n_a + n_b) / 21.0 * 30) || ' €')::LABEL FROM ab_head;
SELECT verdict::TEXT_LARGE, 'Verdict (pre-registered rule)'::LABEL FROM ab_verdict;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 'Result = fail, Result = watch'::FLAG;
SELECT 'The decision checklist (guardrail details: Guardrails tab)'::TITLE,
       check_ AS "Check" ::TABLE, observed AS "Observed", rule AS "Rule", result AS "Result" ::BADGE
FROM ab_verdict,
     LATERAL (VALUES
        (1, 'Split is 50/50 (no sample-ratio mismatch)', 'p = ' || printf('%.2f', srm_p), 'chi-square p ≥ 0.001',
            CASE WHEN srm_p >= 0.001 THEN 'pass' ELSE 'fail' END),
        (2, 'Conversion lift is real', printf('%+.2f pp (%+.2f to %+.2f)', 100 * conv.estimate, 100 * conv.conf_low, 100 * conv.conf_high),
            '95% CI above 0', CASE WHEN conv.conf_low > 0 THEN 'pass' WHEN conv.conf_high < 0 THEN 'fail' ELSE 'watch' END),
        (3, 'Test was big enough for the lift it was sized for', round(100 * ab_power(p_a, ab_mde(), least(n_a, n_b)))::INTEGER || ' % power',
            '≥ 80 % power', CASE WHEN ab_power(p_a, ab_mde(), least(n_a, n_b)) >= 0.8 THEN 'pass' ELSE 'watch' END),
        (5, 'Guardrails hold', (5 - n_fail - n_watch) || ' pass, ' || n_watch || ' watch, ' || n_fail || ' fail',
            'none fails', CASE WHEN n_fail > 0 THEN 'fail' WHEN n_watch > 0 THEN 'watch' ELSE 'pass' END)
     ) t(k, check_, observed, rule, result)
ORDER BY k;

SELECT 12::COL;
WITH d AS (
    SELECT device, prop_test_two_agg(converted::BIGINT, arm::BIGINT) AS r FROM ab_users GROUP BY device
)  -- per device on all visitors, whatever the device input
SELECT ('### Recommendation' || E'\n\n'
        || '**Ship B on ' || coalesce(string_agg(device, ' and ' ORDER BY device DESC) FILTER (WHERE r.conf_low > 0), 'no device') || '** — '
        || coalesce(string_agg(device || ' ' || printf('%+.2f pp (%+.2f to %+.2f)', 100 * r.estimate, 100 * r.conf_low, 100 * r.conf_high), '; ' ORDER BY device DESC) FILTER (WHERE r.conf_low > 0), '')
        || '. **Keep A on ' || coalesce(string_agg(device, ' and ') FILTER (WHERE r.conf_low <= 0), 'no device') || ' and re-test**: '
        || coalesce(string_agg(device || ' ' || printf('%+.2f pp (%+.2f to %+.2f)', 100 * r.estimate, 100 * r.conf_low, 100 * r.conf_high), '; ') FILTER (WHERE r.conf_low <= 0), '')
        ||  ' — no proven lift there, and a loss is not ruled out. The B × device interactions of the logistic model (Segments) back the split. '
        || 'Before the full rollout, put an alert on **support contacts** ('
        || (SELECT printf('%+.2f pp, p = %.2f', 100 * t.estimate, t.p_value)
            FROM (SELECT prop_test_two_agg(support_contact::BIGINT, arm::BIGINT) AS t FROM ab_users))
        || ', inside its margin but not zero) and review them after two weeks. '
        || 'The novelty bump of the first three days is already in the numbers: judge the rollout on weeks 3–4, not week 1.')::MARKDOWN,
       'Recommendation for the head of e-commerce'::TITLE
FROM d;
