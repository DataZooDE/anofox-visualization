-- @title Which claims can we fast-track?
-- @group Service & operations
-- @tags  decide, insurance, claims
--
-- Decide use case — insurance claim routing with anofox_decide.
--
-- STORED RESULTS, NOT LIVE. The model's answers were computed natively with
-- the anofox_decide extension and its local laya-multilingual decision model
-- (examples/decide-claims/compute.sql, written to web/data/decide-claims/*.csv
-- by scripts/gen-decide-cases.sh claims). The model runs on ONNX Runtime and
-- can't run in the browser, so this dashboard only reads the stored answers;
-- the routing, the queues, the costs and every metric are computed in SQL,
-- live, for the thresholds you pick.
--
-- A made-up insurer's claims operations: 320 synthetic first notices of loss
-- (motor, home, travel; English and German) over eight weeks, each with its
-- outcome after investigation. Four intake rules and three questions the
-- model answers about the claimant's description add up to a fraud score;
-- two thresholds turn it into three queues (fast track, adjuster, fraud
-- review). Which claims can be paid at once, which need the fraud team, and
-- what does the description add to the rules?
--
-- Tabs: Overview · Fraud signals · Fraud review queue · Fast track · All claims · Decision.
-- Natively, run from web/:  dashboard --check ../examples/decide-claims/showcase.sql

-- ── Header ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_decide'::BRAND;
SELECT 'Which claims can we fast-track, and which go to fraud review?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_decide answers and the fitted signal weights ──────
CREATE OR REPLACE TABLE cl_scored AS FROM read_csv('data/decide-claims/cl_scored.csv');
CREATE OR REPLACE TABLE cl_signals AS FROM read_csv('data/decide-claims/cl_signals.csv');
CREATE OR REPLACE TABLE cl_metrics AS FROM read_csv('data/decide-claims/cl_metrics.csv');
CREATE OR REPLACE TABLE cl_provenance AS FROM read_csv('data/decide-claims/provenance.csv');

-- The controls.
CREATE OR REPLACE MACRO cl_siu() AS getvariable('siu_at_pct')::DOUBLE / 100;
CREATE OR REPLACE MACRO cl_fast() AS getvariable('fast_below_pct')::DOUBLE / 100;
CREATE OR REPLACE MACRO cl_limit() AS getvariable('fast_limit_eur')::DOUBLE;
CREATE OR REPLACE MACRO cl_cap() AS getvariable('siu_capacity')::DOUBLE;
CREATE OR REPLACE MACRO cl_weeks() AS 8;
CREATE OR REPLACE MACRO cl_monday(w) AS DATE '2026-08-03' + (7 * (w - 1))::INTEGER;

-- Cost model (€ per claim): handling per queue, the share of fraud each queue
-- detects (undetected fraud is paid out), and the goodwill a genuine claimant
-- costs when held in fraud review.
CREATE OR REPLACE MACRO cl_cost(q, fraud, amount) AS
    CASE q WHEN 'Fast track' THEN 40 WHEN 'Adjuster' THEN 220 ELSE 900 END
    + CASE WHEN fraud THEN (1 - CASE q WHEN 'Fast track' THEN 0 WHEN 'Adjuster' THEN 0.3 ELSE 0.9 END) * amount
           WHEN q = 'Fraud review' THEN 300 ELSE 0 END;
CREATE OR REPLACE MACRO cl_stopped(q, fraud, amount) AS
    CASE WHEN fraud THEN CASE q WHEN 'Adjuster' THEN 0.3 WHEN 'Fraud review' THEN 0.9 ELSE 0 END * amount ELSE 0 END;
CREATE OR REPLACE MACRO cl_queue(p, amount, injury, siu, fast, lim) AS
    CASE WHEN p >= siu THEN 'Fraud review' WHEN p < fast AND amount <= lim AND NOT injury THEN 'Fast track' ELSE 'Adjuster' END;
CREATE OR REPLACE MACRO cl_sig(x) AS 1 / (1 + exp(-x));

-- Every claim with both scores, both routings and its reasons. The score sums
-- the weights (log-odds points) of the four rules, plus — for "rules +
-- description" — the three answers about the description.
CREATE OR REPLACE MACRO cl_routed() AS TABLE
WITH s AS (
    SELECT *, cl_sig(base_logit + w_new_policy + w_late_report + w_repeat_claimant + w_large_loss) AS p_rules,
           cl_sig(base_logit + w_new_policy + w_late_report + w_repeat_claimant + w_large_loss
                  + w_no_evidence + w_recent + w_pressure) AS p_fraud
    FROM cl_scored WHERE list_contains(getvariable('lob'), lob))
SELECT *,
       cl_queue(p_fraud, amount_eur, injury, cl_siu(), cl_fast(), cl_limit()) AS queue,
       cl_queue(p_rules, amount_eur, injury, cl_siu(), cl_fast(), cl_limit()) AS queue_rules,
       coalesce(nullif(array_to_string(list_transform(
           list_filter(list_reverse_sort([
               {'w': w_large_loss, 'n': 'large loss'}, {'w': w_new_policy, 'n': 'new policy'},
               {'w': w_late_report, 'n': 'late report'}, {'w': w_repeat_claimant, 'n': 'repeat claimant'},
               {'w': w_no_evidence, 'n': 'no evidence'}, {'w': w_recent, 'n': 'new items'},
               {'w': w_pressure, 'n': 'payout pressure'}]), lambda r: r.w >= 0.5),
           lambda r: r.n || ' +' || round(r.w, 1)), ' · '), ''), 'no red flags') AS reasons
FROM s;

-- Both routings, long: one row per claim and policy.
CREATE OR REPLACE MACRO cl_policies() AS TABLE
SELECT 'rules only' AS policy, claim_id, week, lob, fraud, amount_eur, injury, p_rules AS p, queue_rules AS queue FROM cl_routed()
UNION ALL
SELECT 'rules + description', claim_id, week, lob, fraud, amount_eur, injury, p_fraud, queue FROM cl_routed();

-- The cheapest fraud-review threshold the team can handle, for the recommendation.
CREATE OR REPLACE MACRO cl_options() AS TABLE
WITH c AS (
    SELECT t / 100.0 AS th,
           avg(cl_cost(cl_queue(p_fraud, amount_eur, injury, t / 100.0, cl_fast(), cl_limit()), fraud, amount_eur)) AS cost,
           count(*) FILTER (WHERE p_fraud >= t / 100.0) / cl_weeks() AS per_week
    FROM range(5, 100, 5) r(t), cl_routed() GROUP BY t)
SELECT arg_min(th, cost) AS best FROM c WHERE per_week <= cl_cap();

SELECT 12::COL;
SELECT ('**Asked by** the head of claims operations and the fraud team (SIU) lead. **The decision:** which new claims are paid at once (fast track), which go to an adjuster and which to fraud review — and what reading the claimant''s description adds to the four intake rules.')::MARKDOWN;

SELECT 'Routing rules'::GROUP;
SELECT 70 AS siu_at_pct ::NUMBER, 'Fraud review at P(fraud) ≥ (%)'::LABEL;
SELECT 5 AS fast_below_pct ::NUMBER, 'Fast track below P(fraud) (%)'::LABEL;
SELECT 2500 AS fast_limit_eur ::NUMBER, 'Fast-track limit (€)'::LABEL;
SELECT 4 AS siu_capacity ::NUMBER, 'Fraud-team capacity (claims / week)'::LABEL;
SELECT DISTINCT lob ::MULTISELECT, 'Line of business'::LABEL FROM cl_scored ORDER BY lob;
SELECT 1::ENDGROUP;

SELECT 'Rules + description at your thresholds'::GROUP;
SELECT round(100 * avg((queue = 'Fast track')::INTEGER), 1)::PERCENT,
       round(100 * avg((queue_rules = 'Fast track')::INTEGER), 1)::DELTA, 'abs'::DELTA_FORMAT, 'vs rules only'::DELTA_LABEL,
       'claims paid on the fast track'::LABEL
FROM cl_routed();
SELECT round(sum(cl_stopped(queue, fraud, amount_eur)))::METRIC,
       round(sum(cl_stopped(queue_rules, fraud, amount_eur)))::DELTA, 'vs rules only'::DELTA_LABEL,
       'fraud stopped (€)'::LABEL
FROM cl_routed();
SELECT round(100 * count(*) FILTER (WHERE queue = 'Fraud review' AND fraud) / nullif(count(*) FILTER (WHERE queue = 'Fraud review'), 0), 1)::PERCENT,
       round(100 * count(*) FILTER (WHERE queue_rules = 'Fraud review' AND fraud) / nullif(count(*) FILTER (WHERE queue_rules = 'Fraud review'), 0), 1)::DELTA,
       'abs'::DELTA_FORMAT, 'vs rules only'::DELTA_LABEL,
       'of fraud reviews find fraud'::LABEL
FROM cl_routed();
SELECT round(avg(cl_cost('Adjuster', fraud, amount_eur) - cl_cost(queue, fraud, amount_eur)))::METRIC,
       round(avg(cl_cost('Adjuster', fraud, amount_eur) - cl_cost(queue_rules, fraud, amount_eur)))::DELTA, 'vs rules only'::DELTA_LABEL,
       'saved per claim vs. "all to an adjuster" (€)'::LABEL,
       'Back-tested on the investigated claims: handling (fast track 40 €, adjuster 220 €, fraud review 900 €), plus the fraud each queue misses (an adjuster stops 30 %, fraud review 90 %), plus 300 € goodwill for a genuine claimant held in fraud review.'::INFO
FROM cl_routed();
-- Green within the fraud team's capacity, amber up to a quarter over it, red beyond.
WITH w AS (SELECT week, count(*) FILTER (WHERE queue = 'Fraud review') AS n FROM cl_routed() GROUP BY week)
SELECT round(sum(n) / cl_weeks(), 1)::METRIC, list(n ORDER BY week)::SPARKLINE,
       ('<=' || cl_cap() || ':good; <=' || 1.25 * cl_cap() || ':warn; bad')::THRESHOLDS,
       ('fraud reviews a week (capacity ' || cl_cap()::INTEGER || ')')::LABEL
FROM w;
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 7::COL;
SELECT 330::HEIGHT;
WITH q AS (SELECT queue, policy, count(*) AS n FROM cl_policies() GROUP BY ALL)
SELECT queue ::XAXIS, policy ::CATEGORY, n ::BARCHART, 'tween'::TRANSITION,
       'Where the claims go — the description adds fast-track and fraud-review cases'::TITLE
FROM q ORDER BY max(n) FILTER (WHERE policy = 'rules + description') OVER (PARTITION BY queue) DESC, policy;

SELECT 5::COL;
SELECT 330::HEIGHT;
SELECT cl_monday(week) ::XAXIS, count(*) FILTER (WHERE queue = 'Fraud review') ::BARCHART, 'tween'::TRANSITION,
       cl_cap() ::REFLINE, 'capacity'::REFLINE_LABEL,
       'Fraud reviews per week against the team''s capacity'::TITLE
FROM cl_routed() GROUP BY week ORDER BY week;

SELECT 12::COL;
SELECT 340::HEIGHT;
WITH c AS (
    SELECT t / 100.0 AS th, p.policy, avg(cl_cost(cl_queue(p.p, amount_eur, injury, t / 100.0, cl_fast(), cl_limit()), fraud, amount_eur)) AS cost
    FROM range(5, 100, 5) r(t), cl_policies() p GROUP BY ALL),
o AS (SELECT arg_min(th, cost) AS best, round(min(cost)) AS best_cost FROM c WHERE policy = 'rules + description')
SELECT round(100 * c.th) ::XAXIS, c.policy ::CATEGORY, round(c.cost) ::LINECHART, '€'::YFORMAT, 100 * cl_siu() ::XLINE,
       'Back-tested on the investigated claims: handling (fast track 40 €, adjuster 220 €, fraud review 900 €), plus the fraud each queue misses (an adjuster stops 30 %, fraud review 90 %, the fast track none), plus 300 € goodwill for a genuine claimant held in fraud review.'::INFO,
       ('Cost per claim by fraud-review threshold, P(fraud) % — rules + description is cheapest at ' || round(100 * o.best)::INTEGER
        || ' % (' || o.best_cost::INTEGER || ' € a claim); yours dashed')::TITLE
FROM c, o ORDER BY c.th, c.policy;

-- ── 2 · Fraud signals ────────────────────────────────────────────────────────
SELECT 'Fraud signals'::TAB;

SELECT 12::COL;
SELECT 'Each signal adds **points** (log-odds) to the base fraud rate: the rules from how often they were right on the investigated claims, the model''s answers through a Platt fit (`decide_fit_calibration`) that turns its probability into P(fraud). A concrete question beats a vague one: *"Does the claimant provide evidence?"* separates fraud far better than *"Is this claim suspicious?"*.'::MARKDOWN,
       'How the score is built'::TITLE;

SELECT 6::COL;
SELECT 330::HEIGHT;
WITH v AS (
    UNPIVOT (SELECT fraud, w_new_policy, w_late_report, w_repeat_claimant, w_large_loss, w_no_evidence, w_recent, w_pressure,
                    (p_suspicious - 0.5) AS w_suspicious FROM cl_routed())
    ON COLUMNS('^w_') INTO NAME signal VALUE w),
r AS (SELECT signal, fraud, rank() OVER (PARTITION BY signal ORDER BY w) + (count(*) OVER (PARTITION BY signal, w) - 1) / 2 AS rk FROM v)
SELECT CASE signal WHEN 'w_large_loss' THEN 'large loss (> 8,000 €)' WHEN 'w_new_policy' THEN 'new policy (< 90 days)'
                   WHEN 'w_late_report' THEN 'late report (> 14 days)' WHEN 'w_repeat_claimant' THEN 'repeat claimant (2+)'
                   WHEN 'w_no_evidence' THEN 'no evidence' WHEN 'w_recent' THEN 'new items / raised cover'
                   WHEN 'w_pressure' THEN 'payout pressure' ELSE '"suspicious?" (unused)' END ::XAXIS,
       CASE WHEN signal IN ('w_new_policy', 'w_late_report', 'w_repeat_claimant', 'w_large_loss') THEN 'intake rule' ELSE 'model answer' END ::CATEGORY,
       round((sum(rk) FILTER (WHERE fraud) - count(*) FILTER (WHERE fraud) * (count(*) FILTER (WHERE fraud) + 1) / 2)
             / (count(*) FILTER (WHERE fraud) * count(*) FILTER (WHERE NOT fraud)), 3) ::BARCHART,
       1::FLIP, 0.5::REFLINE, 'How well each signal alone separates fraud (AUROC; 0.5 = coin toss)'::TITLE
FROM r GROUP BY signal ORDER BY 3 DESC;

SELECT 6::COL;
SELECT 330::HEIGHT;
WITH s AS (SELECT * FROM cl_policies()),
th AS (SELECT DISTINCT policy, p AS t FROM s UNION ALL SELECT DISTINCT policy, 1.01 FROM s),
pos AS (SELECT policy, count(*) FILTER (WHERE fraud) AS np, count(*) FILTER (WHERE NOT fraud) AS nn FROM s GROUP BY policy),
roc AS (SELECT th.policy, th.t, count(*) FILTER (WHERE s.p >= th.t AND NOT s.fraud) / any_value(pos.nn) AS fpr,
               count(*) FILTER (WHERE s.p >= th.t AND s.fraud) / any_value(pos.np) AS tpr
        FROM th JOIN s USING (policy) JOIN pos USING (policy) GROUP BY th.policy, th.t)
SELECT round(fpr, 4) ::XAXIS, policy ::CATEGORY, round(tpr, 4) ::STEP, 1::IDENTITY,
       'ROC — the description lifts the rules (x: genuine claims flagged, y: fraud found)'::TITLE
FROM roc ORDER BY policy, fpr, tpr;

SELECT 7::COL;
SELECT 330::HEIGHT;
SELECT policy ::XAXIS, round(p, 3) ::JITTER, CASE WHEN fraud THEN 'fraud confirmed' ELSE 'genuine' END ::CATEGORY,
       cl_siu() ::REFLINE, 'P(fraud) per claim — with the description, fraud rises above your threshold (line)'::TITLE
FROM cl_policies() ORDER BY fraud, policy;

SELECT 5::COL;
SELECT 'Raw answers vs. outcome — calibrate before you threshold'::TITLE,
       CASE question WHEN 'no_evidence' THEN 'no evidence (1 − P(evidence))' WHEN 'recent' THEN 'new items / raised cover'
                     WHEN 'pressure' THEN 'payout pressure' ELSE '"suspicious?"' END AS "Question" ::TABLE,
       round(100 * accuracy, 1) AS "Accuracy" ::PERCENT, brier AS "Brier" ::COLORSCALE, ece AS "Calibration error" ::COLORSCALE
FROM cl_metrics ORDER BY brier;

-- ── 3 · Fraud review queue ───────────────────────────────────────────────────
SELECT 'Fraud review queue'::TAB;

SELECT 12::COL;
SELECT 'The queue at your thresholds, highest P(fraud) first; click a reason to list only its claims. **Back-test:** the outcome column is what the investigation found later — it is not known when the claim arrives. Claims past the team''s weekly capacity are marked; they wait or go to an adjuster.'::MARKDOWN,
       'Reading the queue'::TITLE;

SELECT 5::COL;
SELECT 380::HEIGHT;
SELECT 'reason'::SELECT_AS;
SELECT reason ::XAXIS, count(*) ::BARCHART, 1::FLIP, 'Why claims land in fraud review (reasons ≥ 0.5 points)'::TITLE
FROM (SELECT unnest(string_split(regexp_replace(reasons, ' \+[0-9.]+', '', 'g'), ' · ')) AS reason FROM cl_routed() WHERE queue = 'Fraud review')
WHERE reason <> 'no red flags' GROUP BY ALL ORDER BY 2 DESC;

SELECT 7::COL;
SELECT 380::HEIGHT;
SELECT CASE WHEN fraud THEN 'fraud confirmed' ELSE 'genuine' END ::XAXIS, queue ::CATEGORY, count(*) ::BARCHART_STACKED,
       'Where fraud ends up — and how many genuine claims wait in fraud review'::TITLE
FROM cl_routed() GROUP BY ALL ORDER BY 1, 2;

SELECT 12::COL;
SELECT 'Amount: sum, P(fraud): mean'::TOTALS;
SELECT 'Amount: top 3'::HIGHLIGHT;
WITH q AS (SELECT *, row_number() OVER (PARTITION BY week ORDER BY p_fraud DESC) AS week_rank FROM cl_routed() WHERE queue = 'Fraud review')
SELECT ('Fraud review queue at your thresholds' || CASE WHEN coalesce(getvariable('reason'), '') <> '' THEN ' — ' || getvariable('reason') ELSE '' END)::TITLE,
       claim_id AS "Claim" ::TABLE, cl_monday(week) AS "Week of", lob AS "Line", amount_eur AS "Amount", round(100 * p_fraud, 1) AS "P(fraud)" ::PERCENT,
       CASE WHEN week_rank <= cl_cap() THEN 'ok: within capacity' ELSE 'review: over capacity' END AS "Capacity" ::BADGE,
       reasons AS "Reasons (points)",
       CASE WHEN fraud THEN 'failed: fraud confirmed' ELSE 'ok: genuine' END AS "Outcome (back-test)" ::BADGE,
       description AS "Description"
FROM q
WHERE coalesce(getvariable('reason'), '') = '' OR reasons LIKE '%' || getvariable('reason') || ' +%'
ORDER BY p_fraud DESC;

-- ── 4 · Fast track ───────────────────────────────────────────────────────────
SELECT 'Fast track'::TAB;

SELECT 6::COL;
SELECT 320::HEIGHT;
WITH f AS (SELECT lob, policy, round(100 * avg((queue = 'Fast track')::INTEGER), 1) AS share FROM cl_policies() GROUP BY ALL)
SELECT lob ::XAXIS, policy ::CATEGORY, share ::BARCHART, 'percent'::YFORMAT,
       'Share fast-tracked per line of business'::TITLE
FROM f ORDER BY max(share) FILTER (WHERE policy = 'rules + description') OVER (PARTITION BY lob) DESC, policy;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT CASE WHEN injury THEN 'injury' WHEN amount_eur > cl_limit() THEN 'over the limit' WHEN p_fraud >= cl_siu() THEN 'fraud review'
            WHEN p_fraud >= cl_fast() THEN 'P(fraud) not low enough' ELSE 'fast track' END ::XAXIS,
       count(*) ::BARCHART, 1::FLIP, 'Why a claim is or is not fast-tracked (the first rule that applies)'::TITLE
FROM cl_routed() GROUP BY ALL ORDER BY 2 DESC;

SELECT 12::COL;
SELECT 'Amount: sum'::TOTALS;
SELECT 'Queue = Fast track'::FLAG;
SELECT 'Fraud that is paid without a fraud review at your thresholds (fast track: always; adjuster: 70 % of it)'::TITLE,
       claim_id AS "Claim" ::TABLE, queue AS "Queue", lob AS "Line", amount_eur AS "Amount", round(100 * p_fraud, 1) AS "P(fraud)" ::PERCENT,
       round(100 * p_rules, 1) AS "P(fraud), rules only" ::PERCENT, reasons AS "Reasons (points)",
       description AS "Description"
FROM cl_routed() WHERE fraud AND queue <> 'Fraud review' ORDER BY queue DESC, amount_eur DESC;

-- ── 5 · All claims ───────────────────────────────────────────────────────────
SELECT 'All claims'::TAB;

SELECT 12::COL;
SELECT 'queue'::SUBTOTAL;
SELECT 'Amount: sum, P(fraud): mean, Cost: sum'::TOTALS;
SELECT 'The last four weeks'' claims by queue at your thresholds, with their reasons and the back-tested cost'::TITLE,
       queue ::TABLE, claim_id AS "Claim", cl_monday(week) AS "Week of", lob AS "Line", lang AS "Lang", amount_eur AS "Amount",
       round(100 * p_fraud, 1) AS "P(fraud)" ::PERCENT, reasons AS "Reasons (points)",
       CASE WHEN fraud THEN 'failed: fraud' ELSE 'ok: genuine' END AS "Outcome" ::BADGE,
       round(cl_cost(queue, fraud, amount_eur)) AS "Cost", description AS "Description"
FROM cl_routed() WHERE week > cl_weeks() - 4 ORDER BY CASE queue WHEN 'Fast track' THEN 1 WHEN 'Adjuster' THEN 2 ELSE 3 END, p_fraud DESC;

-- ── 6 · Decision ─────────────────────────────────────────────────────────────
SELECT 'Decision'::TAB;

SELECT 12::COL;
WITH b AS (SELECT coalesce((SELECT best FROM cl_options()), 0.95) AS best),
r AS (SELECT count(*) AS n,
             avg(cl_cost(queue_rules, fraud, amount_eur)) AS c_rules,
             avg(cl_cost(cl_queue(p_fraud, amount_eur, injury, b.best, cl_fast(), cl_limit()), fraud, amount_eur)) AS c_best,
             avg((cl_queue(p_fraud, amount_eur, injury, b.best, cl_fast(), cl_limit()) = 'Fast track')::INTEGER) AS ft_best,
             count(*) FILTER (WHERE p_fraud >= b.best) / cl_weeks() AS siu_best, any_value(b.best) AS best
      FROM cl_routed(), b)
SELECT ('**Recommendation.** Route on **rules + description** and send claims to fraud review from **P(fraud) ≥ ' || round(100 * best)::INTEGER
        || ' %** — the cheapest threshold the fraud team can handle (' || round(siu_best, 1) || ' reviews a week against a capacity of '
        || cl_cap()::INTEGER || '). Fast-track below ' || round(100 * cl_fast())::INTEGER || ' % up to ' || cl_limit()::INTEGER || ' €: that pays '
        || round(100 * ft_best)::INTEGER || ' % of claims at once. Against the rules alone this saves about **'
        || format('{:,}', (round((c_rules - c_best) * n / cl_weeks() * 52 / 1000) * 1000)::INTEGER)
        || ' € a year** at this volume (' || round(n / cl_weeks())::INTEGER || ' claims a week). Keep the weights under review: they were fitted on these '
        || n || ' investigated claims, so re-fit them (`decide_fit_calibration`) on next quarter''s outcomes before trusting the savings.')::MARKDOWN,
       'What to do'::TITLE
FROM r;

SELECT 12::COL;
SELECT 320::HEIGHT;
WITH b AS (SELECT coalesce((SELECT best FROM cl_options()), 0.95) AS best),
r AS (SELECT count(*) / cl_weeks() * 52 AS per_year,
             avg(cl_cost('Adjuster', fraud, amount_eur)) AS c_today,
             avg(cl_cost(queue_rules, fraud, amount_eur)) AS c_rules,
             avg(cl_cost(queue, fraud, amount_eur)) AS c_desc,
             avg(cl_cost(cl_queue(p_fraud, amount_eur, injury, b.best, cl_fast(), cl_limit()), fraud, amount_eur)) AS c_best
      FROM cl_routed(), b)
SELECT step ::XAXIS, round(eur, -2) ::WATERFALL, '€'::YFORMAT,
       'Saving a year against "all to an adjuster", step by step'::TITLE
FROM (SELECT 1 AS i, 'Rules, your thresholds' AS step, (c_today - c_rules) * per_year AS eur FROM r
      UNION ALL SELECT 2, '+ reading the description', (c_rules - c_desc) * per_year FROM r
      UNION ALL SELECT 3, '+ the cheapest threshold within capacity', (c_desc - c_best) * per_year FROM r
      UNION ALL SELECT 4, 'Recommended', NULL)
ORDER BY i;

SELECT 12::COL;
SELECT 'Cost per claim: bottom 1, Fraud stopped: top 1'::HIGHLIGHT;
SELECT 'none'::TOTALS;
WITH b AS (SELECT coalesce((SELECT best FROM cl_options()), 0.95) AS best),
o AS (
    SELECT 1 AS k, 'All to an adjuster (today)' AS option, 'Adjuster' AS queue, fraud, amount_eur FROM cl_routed()
    UNION ALL SELECT 2, 'Rules only, your thresholds', queue_rules, fraud, amount_eur FROM cl_routed()
    UNION ALL SELECT 3, 'Rules + description, your thresholds', queue, fraud, amount_eur FROM cl_routed()
    UNION ALL SELECT 4, 'Rules + description, cheapest within capacity (' || round(100 * b.best)::INTEGER || ' %)',
                     cl_queue(p_fraud, amount_eur, injury, b.best, cl_fast(), cl_limit()), fraud, amount_eur FROM cl_routed(), b)
SELECT 'The options side by side (back-test on the investigated claims)'::TITLE,
       option AS "Option" ::TABLE,
       round(100 * avg((queue = 'Fast track')::INTEGER), 1) AS "Fast-tracked" ::PERCENT,
       round(count(*) FILTER (WHERE queue = 'Fraud review') / cl_weeks(), 1) AS "Fraud reviews / week",
       round(100 * sum(cl_stopped(queue, fraud, amount_eur)) / nullif(sum(amount_eur) FILTER (WHERE fraud), 0), 1) AS "Fraud € stopped" ::PERCENT,
       round(sum(cl_stopped(queue, fraud, amount_eur))) AS "Fraud stopped",
       round(avg(cl_cost(queue, fraud, amount_eur))) AS "Cost per claim"
FROM o GROUP BY k, option ORDER BY k;
