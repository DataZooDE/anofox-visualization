-- @title Can a model triage the support inbox?
-- @group Service & operations
-- @tags  decide, support, triage
--
-- Decide showcase — natural-language decisions with anofox_decide.
--
-- STORED RESULTS, NOT LIVE. The model answers below were computed natively
-- with the anofox_decide extension and its local laya-multilingual decision
-- model (examples/decide-showcase/compute.sql, written to web/data/decide/*.csv
-- by scripts/gen-showcase-data.sh). The model runs on ONNX Runtime and can't
-- run in the browser, so this dashboard only reads the stored answers; every
-- metric (precision, recall, cost, Brier score, calibration, routing accuracy)
-- is computed in SQL from them, live, for the threshold and filters you pick.
--
-- A made-up software shop's support inbox: 92 synthetic tickets in English,
-- German and French, each labelled by a support lead (refund requested?
-- owning team? how upset?). The model answers the same three questions in one
-- decide_answers() call per ticket. Can its answers route tickets
-- automatically, and where should the thresholds sit?
--
-- Tabs: Refund queue · Calibration · Routing · Frustration · All tickets.
-- Natively, run from web/:  dashboard --check ../examples/decide-showcase/showcase.sql

-- ── Header ─────────────────────────────────────────────────────────────────────
SELECT 'anofox_decide'::BRAND;
SELECT 'Can a decision model triage the support inbox?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_decide answers ───────────────────────────────────
CREATE OR REPLACE TABLE dc_scored AS
SELECT *, CASE lang WHEN 'en' THEN 'English' WHEN 'de' THEN 'German' ELSE 'French' END AS language
FROM read_csv('data/decide/dc_scored.csv');
CREATE OR REPLACE TABLE dc_provenance AS FROM read_csv('data/decide/provenance.csv');

-- The controls: refund threshold, the cost (agent minutes) of each kind of
-- mistake, the confidence a routing answer needs to be applied automatically,
-- and the ticket filters.
CREATE OR REPLACE MACRO dc_t() AS getvariable('refund_at_pct')::DOUBLE / 100;
CREATE OR REPLACE MACRO dc_gate() AS getvariable('autoroute_at_pct')::DOUBLE / 100;
CREATE OR REPLACE MACRO dc_miss() AS getvariable('missed_refund_min')::DOUBLE;
CREATE OR REPLACE MACRO dc_false() AS getvariable('false_alarm_min')::DOUBLE;
CREATE OR REPLACE MACRO dc_in(lg, tm) AS
    list_contains(getvariable('language'), lg) AND getvariable('team') IN ('All teams', tm);
CREATE OR REPLACE MACRO dc_verdict(p, y, t) AS
    CASE WHEN p >= t AND y THEN 'ok: caught' WHEN p < t AND NOT y THEN 'ok: no refund'
         WHEN y THEN 'failed: missed refund' ELSE 'review: false alarm' END;


SELECT 'Decision rules'::GROUP;
SELECT 50 AS refund_at_pct ::NUMBER, 'Escalate as refund at P ≥ (%)'::LABEL;
SELECT 30 AS missed_refund_min ::NUMBER, 'Cost of a missed refund (min)'::LABEL;
SELECT 5 AS false_alarm_min ::NUMBER, 'Cost of a false alarm (min)'::LABEL;
SELECT 90 AS autoroute_at_pct ::NUMBER, 'Auto-route at confidence ≥ (%)'::LABEL;
SELECT DISTINCT language ::MULTISELECT, 'Languages'::LABEL FROM dc_scored ORDER BY language;
SELECT team ::BUTTONS, 'Labelled team'::LABEL FROM (SELECT 0 AS k, 'All teams' AS team UNION ALL SELECT DISTINCT 1, team FROM dc_scored) ORDER BY k, team;
SELECT 1::ENDGROUP;

SELECT 'At these rules'::GROUP;
SELECT count(*) FILTER (WHERE p_refund >= dc_t())::METRIC, ('of ' || count(*) || ' tickets escalated as refunds')::LABEL
FROM dc_scored WHERE dc_in(language, team);
SELECT (100 * count(*) FILTER (WHERE p_refund >= dc_t() AND refund) / nullif(count(*) FILTER (WHERE p_refund >= dc_t()), 0))::PERCENT,
       'precision: escalations that are refunds'::LABEL
FROM dc_scored WHERE dc_in(language, team);
SELECT (100 * count(*) FILTER (WHERE p_refund >= dc_t() AND refund) / nullif(count(*) FILTER (WHERE refund), 0))::PERCENT,
       'recall: refunds caught'::LABEL
FROM dc_scored WHERE dc_in(language, team);
SELECT round(100 * (dc_miss() * count(*) FILTER (WHERE p_refund < dc_t() AND refund)
                  + dc_false() * count(*) FILTER (WHERE p_refund >= dc_t() AND NOT refund)) / count(*))::METRIC,
       'agent minutes lost per 100 tickets'::LABEL,
       'Every missed refund costs the minutes set in "Cost of a missed refund", every false alarm those in "Cost of a false alarm"; the sum per 100 tickets.'::INFO
FROM dc_scored WHERE dc_in(language, team);
SELECT (100 * count(*) FILTER (WHERE p_team >= dc_gate()) / count(*))::PERCENT, 'tickets routed automatically'::LABEL
FROM dc_scored WHERE dc_in(language, team);
SELECT (100 * avg((team_pred = team)::INTEGER) FILTER (WHERE p_team >= dc_gate()))::PERCENT, 'of those to the right team'::LABEL
FROM dc_scored WHERE dc_in(language, team);
SELECT 1::ENDGROUP;

-- ── 1 · Refund queue ─────────────────────────────────────────────────────────
SELECT 'Refund queue'::TAB;

SELECT 12::COL;
SELECT 'The model returns a **probability**, so the threshold is a business decision: a missed refund costs a callback, a false alarm a few minutes of checking. Change the two costs above and the cheapest threshold moves.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 8::COL;
SELECT 340::HEIGHT;
WITH s AS (SELECT * FROM dc_scored WHERE dc_in(language, team)),
g AS (SELECT t / 100.0 AS th FROM range(5, 100, 5) r(t)),
c AS (SELECT g.th, 100 * (dc_miss() * count(*) FILTER (WHERE s.p_refund < g.th AND s.refund)
                        + dc_false() * count(*) FILTER (WHERE s.p_refund >= g.th AND NOT s.refund)) / count(*) AS cost
      FROM g, s GROUP BY g.th),
o AS (SELECT arg_min(th, cost) AS best, min(cost) AS best_cost FROM c)
SELECT c.th ::XAXIS, round(c.cost, 1) ::LINECHART, 'tween'::TRANSITION,
       CASE WHEN c.th < 0.5 THEN o.best ELSE dc_t() END ::XLINE,
       ('Agent minutes lost per 100 tickets by threshold — cheapest at ' || o.best || ' (' || round(o.best_cost)::INTEGER
        || ' min), yours at ' || dc_t() || ' (dashed lines)') ::TITLE
FROM c, o ORDER BY c.th;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT CASE WHEN p_refund >= dc_t() THEN 'escalated' ELSE 'not escalated' END ::XAXIS,
       CASE WHEN refund THEN 'asked for a refund' ELSE 'no refund asked' END ::YAXIS,
       count(*) ::HEATMAP, ''::DATALABELS, 'Confusion at your threshold (tickets)'::TITLE
FROM dc_scored WHERE dc_in(language, team) GROUP BY ALL ORDER BY 1, 2;

SELECT 12::COL;
SELECT 'Every mistake at your threshold — the misses are tickets that report a defect and demand money back in the same breath'::TITLE,
       dc_verdict(p_refund, refund, dc_t()) AS "Verdict" ::BADGE, '#' || ticket_id AS "Ticket" ::TABLE, language AS "Language",
       round(100 * p_refund, 1) AS "P(refund)" ::PERCENT, team AS "Team", text AS "Ticket text"
FROM dc_scored WHERE dc_in(language, team) AND (p_refund >= dc_t()) <> refund
ORDER BY refund DESC, p_refund DESC;

-- ── 2 · Calibration ──────────────────────────────────────────────────────────
SELECT 'Calibration'::TAB;

SELECT 12::COL;
SELECT 'A calibrated model''s "0.9" means 9 in 10. The model is decisive (most tickets score below 0.1 or above 0.9) and right when it is, in all three languages; the few uncertain tickets mix a defect with a refund demand.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT anofox_plot_curve(c), 'Calibration — predicted vs observed refund rate'::TITLE
FROM (SELECT 'calibration' AS curve_type, 'laya-multilingual' AS model_id,
             avg(p_refund) AS x, avg(refund::INTEGER) AS y, count(*) AS weight
      FROM dc_scored WHERE dc_in(language, team) GROUP BY least(floor(p_refund * 5), 4)) c;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT anofox_plot_curve(c), 'ROC of the refund probability'::TITLE
FROM (WITH s AS (SELECT * FROM dc_scored WHERE dc_in(language, team)),
           th AS (SELECT DISTINCT p_refund AS t FROM s UNION ALL SELECT 1.01),
           pos AS (SELECT count(*) FILTER (WHERE refund) AS p, count(*) FILTER (WHERE NOT refund) AS n FROM s)
      SELECT 'roc' AS curve_type, 'laya-multilingual' AS model_id, th.t AS threshold,
             count(*) FILTER (WHERE s.p_refund >= th.t AND NOT s.refund) / nullif(any_value(pos.n), 0) AS x,
             count(*) FILTER (WHERE s.p_refund >= th.t AND s.refund) / nullif(any_value(pos.p), 0) AS y
      FROM th, s, pos GROUP BY th.t) c;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT th ::XAXIS, metric ::CATEGORY, value ::LINECHART, 'percent'::YFORMAT, dc_t() ::XLINE,
       'Precision and recall by threshold — yours dashed'::TITLE
FROM (SELECT t / 20.0 AS th, m.metric,
             round(100 * CASE m.metric
                 WHEN 'precision' THEN count(*) FILTER (WHERE p_refund >= t / 20.0 AND refund) / nullif(count(*) FILTER (WHERE p_refund >= t / 20.0), 0)
                 ELSE count(*) FILTER (WHERE p_refund >= t / 20.0 AND refund) / nullif(count(*) FILTER (WHERE refund), 0) END, 1) AS value
      FROM range(1, 20) r(t), (VALUES ('precision'), ('recall')) m(metric), dc_scored
      WHERE dc_in(language, team) GROUP BY t, m.metric)
ORDER BY th, metric;

SELECT 7::COL;
SELECT 320::HEIGHT;
SELECT language ::XAXIS, round(p_refund, 3) ::JITTER, CASE WHEN refund THEN 'asked for a refund' ELSE 'no refund asked' END ::CATEGORY,
       dc_t() ::REFLINE, 'P(refund) per ticket — decisive in every language; the misses sit near your threshold (dashed)'::TITLE
FROM dc_scored WHERE dc_in(language, team) ORDER BY refund, language;

SELECT 5::COL;
SELECT 'Quality per language (lower Brier and calibration error are better)'::TITLE,
       'Accuracy at your threshold. Brier: the mean squared gap between P(refund) and the label (0 = perfect, 0.25 = a coin toss). Calibration error: how far the mean P(refund) is from the share of refunds, averaged over five probability bins, weighted by their tickets.'::INFO,
       language AS "Language" ::TABLE, count(*) AS "Tickets" ::PLAIN,
       round(100 * avg(((p_refund >= dc_t()) = refund)::INTEGER), 1) AS "Accuracy" ::PERCENT,
       round(avg(power(p_refund - refund::INTEGER, 2)), 3) AS "Brier" ::COLORSCALE,
       round(any_value(ece), 3) AS "Calibration error" ::COLORSCALE
FROM dc_scored
JOIN (SELECT language AS l2, sum(abs(sp - sy)) / sum(n) AS ece
      FROM (SELECT language, sum(p_refund) AS sp, sum(refund::INTEGER) AS sy, count(*) AS n
            FROM dc_scored WHERE dc_in(language, team) GROUP BY language, least(floor(p_refund * 5), 4))
      GROUP BY language) e ON e.l2 = language
WHERE dc_in(language, team)
GROUP BY language ORDER BY language;

-- ── 3 · Routing ──────────────────────────────────────────────────────────────
SELECT 'Routing'::TAB;

SELECT 12::COL;
SELECT 'A `choice` question picks one of the teams written in the query. Defect tickets route well; "other" tickets are pulled to billing, and raising the confidence gate doesn''t help — better option descriptions would. Click a team bar to list only its routing mistakes.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 4::COL;
SELECT 320::HEIGHT;
SELECT 'clicked_team'::SELECT_AS;
SELECT team ::XAXIS, round(100 * avg((team_pred = team)::INTEGER), 1) ::BARCHART, 'percent'::YFORMAT,
       'Right team, by labelled team — "other" is the weak spot'::TITLE
FROM dc_scored WHERE dc_in(language, team) GROUP BY ALL ORDER BY 2 DESC;

SELECT 4::COL;
SELECT 320::HEIGHT;
SELECT team_pred ::XAXIS, team ::YAXIS, count(*) ::HEATMAP, ''::DATALABELS,
       'Labelled team (rows) vs the model''s choice'::TITLE
FROM dc_scored WHERE dc_in(language, team) GROUP BY ALL ORDER BY 1, 2;

SELECT 4::COL;
SELECT 320::HEIGHT;
SELECT gate ::XAXIS, metric ::CATEGORY, value ::LINECHART, 'percent'::YFORMAT, dc_gate() ::XLINE,
       'A confidence gate cuts coverage but buys no accuracy — wrong routings are as confident as right ones'::TITLE
FROM (SELECT g / 100.0 AS gate, m.metric,
             round(CASE m.metric WHEN 'accuracy of auto-routed' THEN 100 * avg((team_pred = team)::INTEGER) FILTER (WHERE p_team >= g / 100.0)
                                 ELSE 100 * count(*) FILTER (WHERE p_team >= g / 100.0) / count(*) END, 1) AS value
      FROM range(50, 100, 2) r(g), (VALUES ('accuracy of auto-routed'), ('share auto-routed')) m(metric), dc_scored
      WHERE dc_in(language, team) GROUP BY g, m.metric)
WHERE value IS NOT NULL ORDER BY gate, metric;

SELECT 12::COL;
SELECT ('Routing mistakes' || CASE WHEN coalesce(getvariable('clicked_team'), '') <> '' THEN ' — labelled ' || getvariable('clicked_team') ELSE '' END)::TITLE,
       CASE WHEN p_team >= dc_gate() THEN 'failed: auto-routed wrong' ELSE 'review: held for a human' END AS "Outcome" ::BADGE,
       '#' || ticket_id AS "Ticket" ::TABLE, language AS "Language", team AS "Labelled team", team_pred AS "Model's team",
       round(100 * p_team, 1) AS "Confidence" ::PERCENT, text AS "Ticket text"
FROM dc_scored WHERE dc_in(language, team) AND team_pred <> team AND coalesce(getvariable('clicked_team'), '') IN ('', team)
ORDER BY p_team DESC;

-- ── 4 · Frustration ──────────────────────────────────────────────────────────
SELECT 'Frustration'::TAB;

SELECT 12::COL;
SELECT 'A `score` question returns the expected level on a rubric (calm 0, annoyed 1, angry 2). It separates calm writers from upset ones but cannot grade how upset — good enough to order a callback queue, not to label tone.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT CASE frustration WHEN 0 THEN '0 calm' WHEN 1 THEN '1 annoyed' ELSE '2 angry' END ::XAXIS, frustration_score ::BOXPLOT,
       'Score by labelled level — calm is lower; annoyed and angry overlap'::TITLE
FROM dc_scored WHERE dc_in(language, team) ORDER BY 1;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT CASE frustration WHEN 0 THEN '0 calm' WHEN 1 THEN '1 annoyed' ELSE '2 angry' END ::CATEGORY, frustration_score ::DENSITY,
       'Distribution of the score per labelled level'::TITLE
FROM dc_scored WHERE dc_in(language, team) ORDER BY 1;

SELECT 12::COL;
SELECT 'Call back first — the 10 highest scores among open refund escalations'::TITLE,
       CASE frustration WHEN 0 THEN 'calm (low)' WHEN 1 THEN 'annoyed (medium)' ELSE 'angry (high)' END AS "Labelled tone" ::BADGE,
       '#' || ticket_id AS "Ticket" ::TABLE, language AS "Language", round(frustration_score, 2) AS "Score (0–2)" ::PLAIN,
       round(100 * p_refund, 1) AS "P(refund)" ::PERCENT, text AS "Ticket text"
FROM dc_scored WHERE dc_in(language, team) AND p_refund >= dc_t()
ORDER BY frustration_score DESC LIMIT 10;

-- ── 5 · All tickets ──────────────────────────────────────────────────────────
SELECT 'All tickets'::TAB;

SELECT 12::COL;
SELECT 'Every ticket with its labels, the model''s answers and the verdict at your rules'::TITLE,
       '#' || ticket_id AS "Ticket" ::TABLE, language AS "Language", text AS "Ticket text",
       dc_verdict(p_refund, refund, dc_t()) AS "Refund verdict" ::BADGE, round(100 * p_refund, 1) AS "P(refund)" ::PERCENT,
       CASE WHEN team_pred = team THEN 'ok' ELSE 'failed: ' || team_pred END AS "Routing" ::BADGE, team AS "Labelled team",
       round(frustration_score, 2) AS "Frustration (0–2)" ::PLAIN
FROM dc_scored WHERE dc_in(language, team) ORDER BY ticket_id;
