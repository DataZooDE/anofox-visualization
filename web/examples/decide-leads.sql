-- Decide use case — inbound lead routing and qualification with anofox_decide.
--
-- STORED RESULTS, NOT LIVE. The models' answers were computed natively with
-- the anofox_decide extension and two local decision models,
-- laya-typed-decisions and laya-multilingual
-- (examples/decide-leads/compute.sql, written to web/data/decide-leads/*.csv
-- by scripts/gen-decide-cases.sh leads). The models run on ONNX Runtime and
-- can't run in the browser, so this dashboard only reads the stored answers;
-- the routing, the lead score, the call list and every metric are computed in
-- SQL, live, for the settings you pick.
--
-- A made-up warehouse-software vendor, "Stockline": 300 synthetic inbound
-- messages over six weeks. The model answers four questions per message:
-- which team should handle it, is a budget approved, is the writer a senior
-- decision maker, do they want to decide this quarter. The CRM holds what the
-- sales development reps found out later and which messages became
-- opportunities. Who should sales call first, and why?
--
-- Tabs: Overview · Reasons · Routing · This week's call list · All leads.
-- Natively, run from web/:  dashboard --check ../examples/decide-leads/showcase.sql

-- ── Header ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_decide'::BRAND;
SELECT 'Who should sales call first this week?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_decide answers ───────────────────────────────────
CREATE OR REPLACE TABLE ld_scored AS FROM read_csv('data/decide-leads/ld_scored.csv');
CREATE OR REPLACE TABLE ld_metrics AS FROM read_csv('data/decide-leads/ld_metrics.csv');
CREATE OR REPLACE TABLE ld_provenance AS FROM read_csv('data/decide-leads/provenance.csv');

-- The controls.
CREATE OR REPLACE MACRO ld_th() AS getvariable('sales_at_pct')::DOUBLE / 100;
CREATE OR REPLACE MACRO ld_cap() AS getvariable('calls_per_week')::INTEGER;
CREATE OR REPLACE MACRO ld_score(pb, pa, pt) AS
    100 * (getvariable('w_budget') * pb + getvariable('w_authority') * pa + getvariable('w_timeline') * pt)
    / nullif(getvariable('w_budget') + getvariable('w_authority') + getvariable('w_timeline'), 0);
CREATE OR REPLACE MACRO ld_monday(w) AS DATE '2026-08-31' + (7 * (w - 1))::INTEGER;

-- Every message for the chosen model: lead score, sales queue, reasons, and
-- its place in the week's call list (by score) and in today's process (a
-- coordinator sorts the inbox by hand; reps call sales enquiries in arrival order).
CREATE OR REPLACE MACRO ld_leads(m) AS TABLE
WITH s AS (
    SELECT *, ld_score(p_budget, p_authority, p_timeline) AS score, p_sales >= ld_th() AS to_sales,
           coalesce(nullif(array_to_string(list_transform(
               list_filter(list_reverse_sort([
                   {'p': p_budget, 'n': 'budget'}, {'p': p_authority, 'n': 'senior'},
                   {'p': p_timeline, 'n': 'this quarter'}]), lambda r: r.p >= 0.5),
               lambda r: r.n || ' ' || round(100 * r.p)::INTEGER || ' %'), ' · '), ''), 'no buying signal') AS reasons
    FROM ld_scored WHERE model = m)
SELECT *,
       CASE WHEN to_sales THEN row_number() OVER (PARTITION BY week, to_sales ORDER BY score DESC, lead_id) END AS call_rank,
       CASE WHEN team = 'sales' THEN row_number() OVER (PARTITION BY week, team = 'sales' ORDER BY lead_id) END AS inbox_rank
FROM s;
CREATE OR REPLACE MACRO ld_now() AS TABLE FROM ld_leads(getvariable('model'));

SELECT 12::COL;
SELECT ('**Asked by** the head of sales development, every Monday. **The decision:** which inbound messages go to the sales team, in which order the reps call them, and what to open the call with. Today a coordinator sorts the shared inbox by hand and the reps call the sales enquiries first come, first served.')::MARKDOWN;

SELECT 'Routing and call list'::GROUP;
SELECT model ::DROPDOWN, 'Decision model'::LABEL FROM (SELECT DISTINCT model FROM ld_scored) ORDER BY model DESC;
SELECT 25 AS sales_at_pct ::NUMBER, 'To sales at P(sales) ≥ (%)'::LABEL;
SELECT 10 AS calls_per_week ::NUMBER, 'Calls per week (rep capacity)'::LABEL;
SELECT 3 AS w_budget ::NUMBER, 'Weight: budget approved'::LABEL;
SELECT 1 AS w_authority ::NUMBER, 'Weight: decision maker'::LABEL;
SELECT 3 AS w_timeline ::NUMBER, 'Weight: decides this quarter'::LABEL;
SELECT 1::ENDGROUP;

SELECT 'Six weeks of calls at these settings (arrow: change from today''s first come, first served)'::GROUP;
SELECT count(*) FILTER (WHERE call_rank <= ld_cap() AND converted)::METRIC,
       count(*) FILTER (WHERE inbox_rank <= ld_cap() AND converted)::DELTA,
       ('opportunities from ' || 6 * ld_cap() || ' calls')::LABEL
FROM ld_now();
SELECT sum(deal_value_eur) FILTER (WHERE call_rank <= ld_cap() AND converted)::METRIC,
       sum(deal_value_eur) FILTER (WHERE inbox_rank <= ld_cap() AND converted)::DELTA,
       'pipeline from those calls (€ a year)'::LABEL
FROM ld_now();
SELECT round(100 * count(*) FILTER (WHERE call_rank <= ld_cap() AND converted) / nullif(count(*) FILTER (WHERE call_rank <= ld_cap()), 0), 1)::PERCENT,
       round(100 * count(*) FILTER (WHERE inbox_rank <= ld_cap() AND converted) / nullif(count(*) FILTER (WHERE inbox_rank <= ld_cap()), 0), 1)::DELTA,
       'of calls become an opportunity'::LABEL
FROM ld_now();
SELECT round(100 * count(*) FILTER (WHERE to_sales AND team = 'sales') / count(*) FILTER (WHERE team = 'sales'), 1)::PERCENT,
       'of sales enquiries reach the sales queue'::LABEL
FROM ld_now();
SELECT round(100 * count(*) FILTER (WHERE NOT to_sales AND team <> 'sales') / count(*) FILTER (WHERE team <> 'sales'), 1)::PERCENT,
       'of other messages kept out of it'::LABEL
FROM ld_now();
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 12::COL;
SELECT 340::HEIGHT;
WITH k AS (SELECT range AS k FROM range(1, 31)),
g AS (
    SELECT k.k, 'Lead score (model)' AS ranking, count(*) FILTER (WHERE l.call_rank <= k.k AND l.converted) AS opps FROM k, ld_now() l GROUP BY k.k
    UNION ALL SELECT k.k, 'Today: first come, first served', count(*) FILTER (WHERE l.inbox_rank <= k.k AND l.converted) FROM k, ld_now() l GROUP BY k.k
    UNION ALL SELECT k.k, 'Upper bound: hindsight', sum(least(k.k, w.n)) FROM k, (SELECT week, count(*) FILTER (WHERE converted) AS n FROM ld_now() GROUP BY week) w GROUP BY k.k)
SELECT k ::XAXIS, ranking ::CATEGORY, opps ::LINECHART, ld_cap() ::XLINE,
       ('Opportunities in six weeks by calls a week — at ' || ld_cap() || ' calls the score finds '
        || (SELECT opps FROM g WHERE k = ld_cap() AND ranking LIKE 'Lead%') || ', arrival order '
        || (SELECT opps FROM g WHERE k = ld_cap() AND ranking LIKE 'Today%') || ' (your capacity dashed)')::TITLE
FROM g ORDER BY k, ranking;

SELECT 6::COL;
SELECT 320::HEIGHT;
WITH b AS (SELECT least(floor(score / 20), 4)::INTEGER AS band, avg(converted::INTEGER) AS rate FROM ld_now() WHERE to_sales GROUP BY ALL)
SELECT 20 * band || '–' || 20 * band + 19 ::XAXIS, round(100 * rate, 1) ::BARCHART, 'percent'::YFORMAT,
       'Deal rate by lead score (sales queue) — a higher score, more deals'::TITLE
FROM b ORDER BY band;

SELECT 6::COL;
SELECT 320::HEIGHT;
WITH m AS (
    SELECT l.model AS ranking, count(*) FILTER (WHERE l.call_rank <= ld_cap() AND l.converted) AS opps
    FROM (SELECT DISTINCT model FROM ld_scored) d, ld_leads(d.model) l GROUP BY l.model
    UNION ALL SELECT 'today: first come, first served', count(*) FILTER (WHERE inbox_rank <= ld_cap() AND converted) FROM ld_now())
SELECT ranking ::XAXIS, opps ::BARCHART, 1::FLIP,
       'Opportunities at your capacity, by model — the model matters'::TITLE
FROM m ORDER BY opps DESC;

-- ── 2 · Reasons ──────────────────────────────────────────────────────────────
SELECT 'Reasons'::TAB;

SELECT 12::COL;
SELECT 'The score is built from three yes/no questions the model answers about each message — *"A budget has been approved."*, *"The writer is a director, head or managing director."*, *"They want to decide this quarter or within weeks."* — weighted with the weights above. Each answer is checked against what the reps found out on the call. A reason the model cannot read reliably should get a low weight.'::MARKDOWN,
       'What the reasons are'::TITLE;

SELECT 6::COL;
SELECT 330::HEIGHT;
WITH v AS (
    SELECT model, 'budget approved' AS reason, p_budget AS p, budget_approved AS y FROM ld_scored WHERE team = 'sales'
    UNION ALL SELECT model, 'decision maker', p_authority, decision_maker FROM ld_scored WHERE team = 'sales'
    UNION ALL SELECT model, 'decides this quarter', p_timeline, buys_this_quarter FROM ld_scored WHERE team = 'sales'),
r AS (SELECT *, rank() OVER (PARTITION BY model, reason ORDER BY p) + (count(*) OVER (PARTITION BY model, reason, p) - 1) / 2 AS rk FROM v)
SELECT reason ::XAXIS, model ::CATEGORY,
       round((sum(rk) FILTER (WHERE y) - count(*) FILTER (WHERE y) * (count(*) FILTER (WHERE y) + 1) / 2)
             / (count(*) FILTER (WHERE y) * count(*) FILTER (WHERE NOT y)), 3) ::BARCHART,
       1::FLIP, 0.5::REFLINE, 'Does the answer agree with the rep''s finding? (AUROC; 0.5 = coin toss)'::TITLE
FROM r GROUP BY ALL ORDER BY reason, model;

SELECT 6::COL;
SELECT 330::HEIGHT;
WITH v AS (
    SELECT 'budget approved' AS reason, p_budget AS p, budget_approved AS y FROM ld_now() WHERE team = 'sales'
    UNION ALL SELECT 'decision maker', p_authority, decision_maker FROM ld_now() WHERE team = 'sales'
    UNION ALL SELECT 'decides this quarter', p_timeline, buys_this_quarter FROM ld_now() WHERE team = 'sales')
SELECT reason ::XAXIS, CASE WHEN y THEN 'rep found yes' ELSE 'rep found no' END ::CATEGORY, round(100 * avg(p), 1) ::BARCHART,
       'percent'::YFORMAT, 'The chosen model''s mean P(yes), split by what the rep found'::TITLE
FROM v GROUP BY ALL ORDER BY reason, 2 DESC;

SELECT 12::COL;
SELECT 'Accuracy: top 1, Brier: bottom 1'::HIGHLIGHT;
SELECT 'none'::TOTALS;
SELECT 'Both models on the sales enquiries (decide_accuracy at 0.5, decide_brier_score, decide_ece; lower Brier and calibration error are better)'::TITLE,
       replace(reason, 'buys', 'decides') AS "Reason" ::TABLE, model AS "Model", positives || ' of ' || leads AS "Rep found yes",
       round(100 * accuracy, 1) AS "Accuracy" ::PERCENT, brier AS "Brier" ::COLORSCALE, ece AS "Calibration error" ::COLORSCALE
FROM ld_metrics ORDER BY reason, brier;

-- ── 3 · Routing ──────────────────────────────────────────────────────────────
SELECT 'Routing'::TAB;

SELECT 12::COL;
SELECT 'A `choice` question picks one of four teams; each option carries a short description in the query (*"sales: wants to buy, asks for a demo, a quote or pricing"*). Without them, enquiries that mention a problem went to support. The sales queue takes every message whose **P(sales)** reaches your threshold, so a lower threshold trades a few stray messages for fewer lost leads.'::MARKDOWN,
       'How messages are routed'::TITLE;

SELECT 5::COL;
SELECT 340::HEIGHT;
WITH h AS (SELECT team, team_pred, count(*) AS n FROM ld_now() GROUP BY ALL)
SELECT team_pred ::XAXIS, team ::YAXIS, n ::HEATMAP, ''::DATALABELS,
       'The rep''s team (rows) vs. the model''s choice'::TITLE
FROM h ORDER BY team <> 'sales', team, team_pred;

SELECT 7::COL;
SELECT 340::HEIGHT;
WITH t AS (
    SELECT r.pct, 100 * count(*) FILTER (WHERE l.p_sales >= r.pct / 100 AND l.team = 'sales') / count(*) FILTER (WHERE l.team = 'sales') AS reach,
           100 * count(*) FILTER (WHERE l.p_sales >= r.pct / 100 AND l.team <> 'sales') / nullif(count(*) FILTER (WHERE l.p_sales >= r.pct / 100), 0) AS stray
    FROM (SELECT range AS pct FROM range(5, 65, 5)) r, ld_now() l GROUP BY r.pct)
SELECT pct ::XAXIS, metric ::CATEGORY, round(value, 1) ::LINECHART, 'percent'::YFORMAT, 100 * ld_th() ::XLINE,
       'Sales queue by P(sales) threshold (%) — yours dashed'::TITLE
FROM (UNPIVOT t ON reach AS 'sales enquiries reaching sales', stray AS 'other messages in the sales queue' INTO NAME metric VALUE value)
ORDER BY pct, metric;

SELECT 12::COL;
SELECT 'Deal value: sum'::TOTALS;
SELECT 'Deal value: top 3'::HIGHLIGHT;
SELECT 'Lost leads — sales enquiries that miss the sales queue at your threshold'::TITLE,
       lead_id AS "Lead" ::TABLE, ld_monday(week) AS "Week of", team_pred AS "Model's team", round(100 * p_sales, 1) AS "P(sales)" ::PERCENT,
       CASE WHEN converted THEN 'failed: became a deal' ELSE 'review: no deal' END AS "Outcome (back-test)" ::BADGE,
       CASE WHEN converted THEN deal_value_eur END AS "Deal value", message AS "Message"
FROM ld_now() WHERE team = 'sales' AND NOT to_sales ORDER BY converted DESC, p_sales DESC;

-- ── 4 · This week's call list ────────────────────────────────────────────────
SELECT 'This week''s call list'::TAB;

SELECT 12::COL;
WITH w AS (SELECT * FROM ld_now() WHERE week = 6),
b AS (SELECT count(*) FILTER (WHERE to_sales) AS queued, count(*) FILTER (WHERE call_rank <= ld_cap()) AS calls,
             count(*) FILTER (WHERE call_rank <= ld_cap() AND p_budget >= 0.5) AS with_budget,
             count(*) FILTER (WHERE to_sales AND call_rank > ld_cap() AND score >= 50) AS strong_waiting FROM w),
s AS (SELECT count(*) FILTER (WHERE call_rank <= ld_cap() AND converted) AS m, count(*) FILTER (WHERE inbox_rank <= ld_cap() AND converted) AS f FROM ld_now()),
best AS (SELECT arg_max(l.model, l.n) AS model, max(l.n) AS n
         FROM (SELECT d.model, count(*) FILTER (WHERE x.call_rank <= ld_cap() AND x.converted) AS n
               FROM (SELECT DISTINCT model FROM ld_scored) d, ld_leads(d.model) x GROUP BY d.model) l)
SELECT ('**Recommendation for the week of ' || strftime(ld_monday(6), '%d %b %Y') || '.** ' || queued || ' messages reached the sales queue; call the top **'
        || calls || '** below in this order — ' || with_budget || ' of them have an approved budget. Open each call with its reasons. '
        || CASE WHEN strong_waiting > 0 THEN strong_waiting || ' more scored 50 or higher but do not fit this week''s capacity: add a rep or carry them over. ' ELSE '' END
        || 'Over the last six weeks this order found **' || s.m || '** opportunities where calling in arrival order found ' || s.f || '. '
        || CASE WHEN best.model = getvariable('model') THEN 'Keep **' || best.model || '**: it finds the most opportunities on these messages.'
                ELSE 'Switch to **' || best.model || '**: it would have found ' || best.n || ' opportunities, because it reads the team and the reasons better.' END)::MARKDOWN,
       'What to do'::TITLE
FROM b, s, best;

SELECT 12::COL;
SELECT 'none, Deal value: sum, Score: mean'::TOTALS;
SELECT 'Score: top 3'::HIGHLIGHT;
SELECT 'P(sales) < 40'::FLAG;
SELECT ('Call list for the week of ' || strftime(ld_monday(6), '%d %b %Y') || ' — top ' || ld_cap() || ' of the sales queue by lead score')::TITLE,
       lead_id AS "Lead" ::TABLE, call_rank AS "Call #" ::PLAIN, round(score) AS "Score", reasons AS "Open with",
       round(100 * p_sales, 1) AS "P(sales)" ::PERCENT, warehouses AS "Warehouses" ::PLAIN,
       CASE WHEN converted THEN 'ok: became a deal' WHEN team <> 'sales' THEN 'failed: not a sales enquiry' ELSE 'review: no deal' END AS "Outcome (back-test)" ::BADGE,
       CASE WHEN converted THEN deal_value_eur END AS "Deal value", message AS "Message"
FROM ld_now() WHERE week = 6 AND call_rank <= ld_cap() ORDER BY call_rank;

-- ── 5 · All leads ────────────────────────────────────────────────────────────
SELECT 'All leads'::TAB;

SELECT 12::COL;
SELECT 'queue'::SUBTOTAL;
SELECT 'none, Deal value: sum, Score: mean'::TOTALS;
SELECT 'The last three weeks'' messages by queue, with the model''s answers and the CRM outcome'::TITLE,
       CASE WHEN call_rank <= ld_cap() THEN '1 call list' WHEN to_sales THEN '2 sales queue, not called' ELSE '3 not sales: ' || team_pred END AS queue ::TABLE,
       lead_id AS "Lead", ld_monday(week) AS "Week of", round(score) AS "Score", reasons AS "Reasons",
       round(100 * p_sales, 1) AS "P(sales)" ::PERCENT, team AS "Rep's team",
       CASE WHEN converted THEN 'ok: deal' WHEN team = 'sales' THEN 'review: no deal' ELSE 'ok: not sales' END AS "Outcome" ::BADGE,
       CASE WHEN converted THEN deal_value_eur END AS "Deal value", message AS "Message"
FROM ld_now() WHERE week > 3 ORDER BY queue, week DESC, score DESC;
