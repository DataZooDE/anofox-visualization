-- Decide showcase — natural-language decisions with anofox_decide.
--
-- STORED RESULTS, NOT LIVE. The model answers below were computed natively
-- with the anofox_decide extension and its local laya-multilingual decision
-- model (examples/decide-showcase/compute.sql, written to web/data/decide/*.csv
-- by scripts/gen-showcase-data.sh). The model runs on ONNX Runtime and can't
-- run in the browser, so this dashboard only reads and plots the stored
-- answers; every quality metric is computed in SQL from them.
--
-- A made-up software shop's support inbox: 92 synthetic tickets in English,
-- German and French, each labelled by a support lead (refund requested?
-- owning team? how upset?). The model answers the same three questions; how
-- far can its answers be trusted to route tickets automatically?
--
-- Tabs: 1 Overview · 2 Refund probability · 3 Routing · 4 Frustration ·
-- 5 Tickets. Natively, run from web/:  dashboard --check ../examples/decide-showcase/showcase.sql

-- ─────────────────────────────────────────────────────────────────────────────
-- Data: the stored anofox_decide answers
-- ─────────────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE dc_scored AS FROM read_csv('data/decide/dc_scored.csv');
CREATE OR REPLACE TABLE dc_metrics AS FROM read_csv('data/decide/dc_metrics.csv');
CREATE OR REPLACE TABLE dc_provenance AS FROM read_csv('data/decide/provenance.csv');

-- Calibration (5 equal-width bins) and ROC of the refund probability, in the
-- `curve` schema of the anofox_plot_curve contract plot.
CREATE OR REPLACE TABLE dc_calibration AS
SELECT 'calibration' AS curve_type, 'laya-multilingual' AS model_id,
       avg(p_refund) AS x, avg(refund::INTEGER) AS y, count(*) AS weight
FROM dc_scored GROUP BY least(floor(p_refund * 5), 4);
CREATE OR REPLACE TABLE dc_roc AS
WITH th AS (SELECT DISTINCT p_refund AS t FROM dc_scored UNION ALL SELECT 1.01),
     pos AS (SELECT count(*) FILTER (WHERE refund) AS p, count(*) FILTER (WHERE NOT refund) AS n FROM dc_scored)
SELECT 'roc' AS curve_type, 'laya-multilingual' AS model_id, th.t AS threshold,
       count(*) FILTER (WHERE s.p_refund >= th.t AND NOT s.refund) / any_value(pos.n) AS x,
       count(*) FILTER (WHERE s.p_refund >= th.t AND s.refund) / any_value(pos.p) AS y
FROM th, dc_scored s, pos GROUP BY th.t;
-- Precision / recall / share escalated for every threshold.
CREATE OR REPLACE TABLE dc_thresholds AS
SELECT t / 20.0 AS threshold,
       count(*) FILTER (WHERE p_refund >= t / 20.0 AND refund) / nullif(count(*) FILTER (WHERE p_refund >= t / 20.0), 0) AS precision,
       count(*) FILTER (WHERE p_refund >= t / 20.0 AND refund) / count(*) FILTER (WHERE refund) AS recall
FROM range(1, 20) r(t), dc_scored GROUP BY t;

-- ─────────────────────────────────────────────────────────────────────────────
-- Dashboard
-- ─────────────────────────────────────────────────────────────────────────────
SELECT 'Can a decision model triage the support inbox? 92 tickets, three questions, anofox_decide (stored results)'::LABEL;

SELECT 12::COL;
SELECT ('**Stored results, computed natively.** The answers on this page were computed with the **anofox_decide** DuckDB extension (version `' || (SELECT extension_version FROM dc_provenance) || '`) and its local **' || (SELECT model FROM dc_metrics) || '** decision model on a native DuckDB ' || (SELECT duckdb_version FROM dc_provenance) || ', by `examples/decide-showcase/compute.sql`; the demo reads them from `web/data/decide/`. They are **not** recomputed in your browser: the model runs on ONNX Runtime, outside DuckDB-Wasm''s reach. The tickets and their labels are synthetic. One call asks all three questions per ticket: `unnest(decide_answers(text, ''[{"id": "refund", "kind": "binary", …}, {"id": "team", "kind": "choice", "options": […]}, {"id": "mood", "kind": "score", "levels": […]}]''), recursive := true)`.')::MARKDOWN, 'About this dashboard'::TITLE;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT '1 Overview'::TAB;

SELECT 12::COL;
SELECT ('**Question:** can the model''s answers route support tickets without a human reading each one? Three questions, written in the query: a **binary** one (*the customer asks for money back*), a **choice** (*which team: billing, defect, other*) and a **score** on an ordered rubric (*calm, annoyed, angry*). **What to look for:** the refund probability is accurate (**' || round(100 * (SELECT accuracy FROM dc_metrics))::INTEGER || ' %** at a 0.5 threshold) and well calibrated (Brier **' || (SELECT brier FROM dc_metrics) || '**, ECE **' || (SELECT ece FROM dc_metrics) || '**), in all three languages; its few misses are mostly tickets that mix a **defect with a refund demand**. Routing is weaker (**' || round(100 * (SELECT team_accuracy FROM dc_metrics))::INTEGER || ' %**): "other" tickets are pulled to billing. The frustration score tells calm from upset writers but not annoyed from angry. Verdict: automate the refund queue, keep a human on routing and tone.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 'How good are the answers?'::GROUP;
SELECT (100 * (SELECT accuracy FROM dc_metrics))::PERCENT, 'refund: accuracy (decide_accuracy)'::LABEL;
SELECT (SELECT brier FROM dc_metrics)::METRIC, 'refund: Brier score (0 = perfect, 0.25 = coin)'::LABEL;
SELECT (SELECT ece FROM dc_metrics)::METRIC, 'refund: calibration error (decide_ece)'::LABEL;
SELECT (100 * (SELECT team_accuracy FROM dc_metrics))::PERCENT, 'team routing: accuracy'::LABEL;
SELECT (SELECT tickets FROM dc_metrics)::METRIC, 'labelled tickets (en / de / fr)'::LABEL;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT 380::HEIGHT;
SELECT CASE lang WHEN 'en' THEN 'English' WHEN 'de' THEN 'German' ELSE 'French' END ::XAXIS, question ::CATEGORY,
       acc ::BARCHART, 'percent'::YFORMAT,
       'Accuracy by language — refunds are recognised in every language; routing varies'::TITLE
FROM (SELECT lang, 'refund (p ≥ 0.5)' AS question, round(100 * avg(((p_refund >= 0.5) = refund)::INTEGER), 1) AS acc FROM dc_scored GROUP BY ALL
      UNION ALL SELECT lang, 'team routing', round(100 * avg((team_pred = team)::INTEGER), 1) FROM dc_scored GROUP BY ALL)
ORDER BY 1, 2;

-- ── 2 · Refund probability ───────────────────────────────────────────────────
SELECT '2 Refund probability'::TAB;

SELECT 12::COL;
SELECT ('**How:** the model returns a **probability**, not a label, so the threshold is yours. **What to look for:** in the calibration plot, each dot is a bin of tickets — mean predicted probability vs the share that really asked for a refund; dots on the diagonal mean *"0.9" really means 9 in 10*. The model is decisive (most tickets score below 0.1 or above 0.9) and right when it is. The ROC curve hugs the top-left corner. The threshold chart shows the trade-off: at **0.5**, **' || round(100 * (SELECT precision FROM dc_thresholds WHERE threshold = 0.5))::INTEGER || ' %** of the escalated tickets are refund requests (precision) and **' || round(100 * (SELECT recall FROM dc_thresholds WHERE threshold = 0.5))::INTEGER || ' %** of all refund requests are caught (recall); at **0.7** precision rises to **' || round(100 * (SELECT precision FROM dc_thresholds WHERE threshold = 0.7))::INTEGER || ' %** for a recall of **' || round(100 * (SELECT recall FROM dc_thresholds WHERE threshold = 0.7))::INTEGER || ' %**. Most misses are the mixed defect-and-refund tickets listed below.')::MARKDOWN, 'What to look for'::TITLE;

SELECT 4::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_curve(c), 'Calibration — predicted vs observed refund rate (dot size = tickets)'::TITLE
FROM dc_calibration c;

SELECT 4::COL;
SELECT 380::HEIGHT;
SELECT anofox_plot_curve(c), 'ROC of the refund probability'::TITLE
FROM dc_roc c;

SELECT 4::COL;
SELECT 380::HEIGHT;
SELECT threshold ::XAXIS, metric ::CATEGORY, value ::LINECHART, 'percent'::YFORMAT, 0.5 ::XLINE,
       'Precision and recall by threshold'::TITLE
FROM (SELECT threshold, 'precision' AS metric, round(100 * precision, 1) AS value FROM dc_thresholds
      UNION ALL SELECT threshold, 'recall', round(100 * recall, 1) FROM dc_thresholds)
ORDER BY threshold, metric;

SELECT 12::COL;
SELECT 'The misses at a 0.5 threshold — four mixed defect-and-refund tickets, a crash report and an invoice request'::TITLE,
       ticket_id AS "Ticket" ::TABLE, lang AS "Lang", refund AS "Refund (label)", round(p_refund, 3) AS "P(refund)" ::PLAIN, text AS "Ticket text"
FROM dc_scored WHERE (p_refund >= 0.5) <> refund ORDER BY p_refund DESC;

-- ── 3 · Routing ──────────────────────────────────────────────────────────────
SELECT '3 Routing'::TAB;

SELECT 12::COL;
SELECT '**How:** `decide_choice` (or a `choice` question in `decide_answers`) picks one of the options written in the query and reports its probability. **What to look for:** the diagonal of the confusion matrix is the correct routing. **Defect** tickets are routed well; **billing** tickets go to defect when they mention something broken; and most **"other"** tickets — thank-you notes, office visits, training questions — land in **billing**, so a catch-all option needs a better description. A threshold on the choice probability would help only a little: wrong routings have the same median confidence (≈ 0.94) as right ones, just a longer tail of hesitant answers.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT team_pred ::XAXIS, team ::YAXIS, count(*) ::HEATMAP, 12 ::DATALABELS,
       'Confusion matrix — labelled team (rows) vs model''s choice (columns)'::TITLE
FROM dc_scored GROUP BY ALL ORDER BY 1, 2;

SELECT 6::COL;
SELECT 400::HEIGHT;
SELECT CASE WHEN team_pred = team THEN 'right team' ELSE 'wrong team' END ::XAXIS, p_team ::BOXPLOT,
       'Choice probability — wrong routings are nearly as confident as right ones'::TITLE
FROM dc_scored;

-- ── 4 · Frustration ──────────────────────────────────────────────────────────
SELECT '4 Frustration'::TAB;

SELECT 12::COL;
SELECT '**How:** a `score` question takes an ordered rubric (here *calm, annoyed, angry*, levels 0–2) and returns the **expected level** — 1.4 means "between annoyed and angry". **What to look for:** if the score worked, the three boxes would climb from 0 to 2. They climb only a little: **calm** tickets score clearly lower than upset ones (median ≈ 0.9 vs ≈ 1.1), but **annoyed and angry** tickets are indistinguishable and every score sits near the middle level. Usable to separate calm from upset writers, not to grade how upset they are.'::MARKDOWN, 'What to look for'::TITLE;

SELECT 7::COL;
SELECT 400::HEIGHT;
SELECT CASE frustration WHEN 0 THEN '0 calm' WHEN 1 THEN '1 annoyed' ELSE '2 angry' END ::XAXIS, frustration_score ::BOXPLOT,
       'Model score by labelled frustration — calm is lower; annoyed and angry look alike'::TITLE
FROM dc_scored ORDER BY 1;

SELECT 5::COL;
SELECT 'Mean score per labelled level'::TITLE,
       CASE frustration WHEN 0 THEN '0 calm' WHEN 1 THEN '1 annoyed' ELSE '2 angry' END AS "Label" ::TABLE,
       round(avg(frustration_score), 2) AS "Mean score (0–2)" ::PLAIN, count(*) AS "Tickets" ::PLAIN
FROM dc_scored GROUP BY frustration ORDER BY frustration;

-- ── 5 · Tickets ──────────────────────────────────────────────────────────────
SELECT '5 Tickets'::TAB;

SELECT 12::COL;
SELECT 'Every ticket with the labels and the model''s answers. Sort by **P(refund)** to see the decisive answers at both ends; filter mentally by language to see that the multilingual model treats German and French like English.'::MARKDOWN, 'What this shows'::TITLE;

SELECT 12::COL;
SELECT ticket_id AS "Ticket" ::PAGED, lang AS "Lang", text AS "Ticket text",
       refund AS "Refund (label)", round(p_refund, 3) AS "P(refund)",
       team AS "Team (label)", team_pred AS "Team (model)", round(frustration_score, 2) AS "Frustration (0–2)"
FROM dc_scored ORDER BY ticket_id;
