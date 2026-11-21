-- @title Which cause-and-effect questions must our experts still settle?
-- @group How it works
-- @tags  diagram, bayesian network, bayesian, causal, experts, structure learning, write-back
-- @description A learned network leaves two arrows open; experts answer question cards until the decision no longer depends on them.
--
-- Which cause-and-effect questions must our experts still settle? A clinic
-- wants to know whether a quit-smoking programme pays off: it does when
-- quitting cuts shortness of breath by at least 10 points. A structure
-- learner found the network of the chest clinic (Lauritzen & Spiegelhalter
-- 1988) from patient records, but two of its arrows it could not orient:
-- smoking – lung cancer and smoking – bronchitis. Both directions fit the
-- data equally well (the arcs lie in its Markov equivalence class), and the
-- effect of smoking depends on them: 23 points if smoking causes both,
-- 3 points if bronchitis led people to smoke.
--
-- The dashboard is the producer's tables and the experts' answers:
-- - the network with each arc's status: directed, undirected (drawn without
--   arrowheads, coloured by the data's lean: the share of bootstrap
--   resamples that pointed one way) or expert (a ✓ badge with who set it);
-- - the questions, ranked: what each answer would settle and whether it
--   moves the decision;
-- - the answers, an input table: Submit writes a row into `answers`
--   (::SAVE of a ::FORM). In the browser it stays in this tab; on a locked
--   server options.writeback declares the table and stamps who (the login)
--   and when on every row, and only its approvers may tick "settle the
--   disagreement" ("approver_only": ["decides"]);
-- - the range of the effect over every graph the data and the answers
--   still allow, per round, against the decision threshold.
--
-- Everything below is plain SQL over plain tables. The joint distribution
-- comes from the network's textbook tables, the effects by back-door
-- adjustment in each graph of the class (smoking's parents). The learner's
-- leans and the three experts' answers are written by hand: dr-a settled
-- smoking → lung cancer, dr-b and dr-c disagree about bronchitis. Settle it:
-- pick the question, answer, and tick "settle the disagreement".

-- Bayesian networks are a plugin (#1275): a build without it says so.
SELECT 'bayes-net'::REQUIRES;

-- The network's nodes.
CREATE OR REPLACE TEMP TABLE cg_nodes AS
FROM (VALUES
  ('asia', 'Visited Asia', 'chance', 'history'),
  ('smoke', 'Smoker', 'chance', 'history'),
  ('tub', 'Tuberculosis', 'chance', 'disease'),
  ('lung', 'Lung cancer', 'chance', 'disease'),
  ('bronc', 'Bronchitis', 'chance', 'disease'),
  ('either', 'TB or cancer', 'deterministic', 'disease'),
  ('xray', 'Abnormal X-ray', 'chance', 'finding'),
  ('dysp', 'Short of breath', 'chance', 'finding')
) t(id, label, kind, "group"), (SELECT ['yes', 'no'] AS states);

-- What the structure learner found: the arcs the data orient (v-structures
-- and what follows from them), the one background knowledge requires (a
-- visit comes before the disease), and the two it can't orient, with the
-- share of 200 bootstrap resamples whose learned graph pointed from → to
-- (written by hand).
CREATE OR REPLACE TEMP TABLE cg_learned AS
FROM (VALUES
  (1, 'asia', 'tub', 'required', NULL),
  (2, 'smoke', 'lung', 'undirected', 0.74::DOUBLE),
  (3, 'smoke', 'bronc', 'undirected', 0.52),
  (4, 'tub', 'either', 'directed', NULL),
  (5, 'lung', 'either', 'directed', NULL),
  (6, 'either', 'xray', 'directed', NULL),
  (7, 'either', 'dysp', 'directed', NULL),
  (8, 'bronc', 'dysp', 'directed', NULL)
) t(k, "from", "to", status, lean);

-- The open questions, in plain words: a, b as in the arc (a_to_b = from → to).
CREATE OR REPLACE TEMP TABLE cg_open AS
FROM (VALUES
  ('smoke|lung', 'smoke', 'lung', 'Does smoking lead to lung cancer, or did lung cancer lead people to smoke?'),
  ('smoke|bronc', 'smoke', 'bronc', 'Does smoking lead to bronchitis, or did bronchitis lead people to smoke?')
) t(q, a, b, text);

-- The graphs in the class: each orients the open arcs one way. Bronchitis
-- → smoking ← lung cancer is not among them: it would make smoking a new
-- collider, which the data rule out.
CREATE OR REPLACE TEMP TABLE cg_graphs AS
FROM (VALUES
  ('G1', 'smoke', 'lung', 'a_to_b'), ('G1', 'smoke', 'bronc', 'a_to_b'),
  ('G2', 'smoke', 'lung', 'b_to_a'), ('G2', 'smoke', 'bronc', 'a_to_b'),
  ('G3', 'smoke', 'lung', 'a_to_b'), ('G3', 'smoke', 'bronc', 'b_to_a')
) t(graph, a, b, dir);

-- The joint distribution of the chest clinic (its textbook tables), every
-- one of the 2^8 cases.
CREATE OR REPLACE TEMP MACRO cg_p(x, p) AS CASE WHEN x THEN p ELSE 1 - p END;
CREATE OR REPLACE TEMP TABLE cg_joint AS
WITH b AS (SELECT unnest([true, false]) AS v)
SELECT a.v AS asia, t.v AS tub, s.v AS smoke, l.v AS lung, br.v AS bronc,
       (t.v OR l.v) AS either, x.v AS xray, d.v AS dysp,
       cg_p(a.v, 0.01) * cg_p(t.v, CASE WHEN a.v THEN 0.05 ELSE 0.01 END) * 0.5
       * cg_p(l.v, CASE WHEN s.v THEN 0.1 ELSE 0.01 END)
       * cg_p(br.v, CASE WHEN s.v THEN 0.6 ELSE 0.3 END)
       * cg_p(x.v, CASE WHEN t.v OR l.v THEN 0.98 ELSE 0.05 END)
       * cg_p(d.v, CASE WHEN (t.v OR l.v) AND br.v THEN 0.9 WHEN t.v OR l.v THEN 0.7
                        WHEN br.v THEN 0.8 ELSE 0.1 END) AS p
FROM b a, b t, b s, b l, b br, b x, b d;

-- Each node's belief (no evidence: its prior).
CREATE OR REPLACE TEMP TABLE cg_beliefs AS
SELECT node, CASE WHEN v THEN 'yes' ELSE 'no' END AS state, sum(p) AS probability, sum(p) AS prior
FROM (UNPIVOT cg_joint ON asia, tub, smoke, lung, bronc, either, xray, dysp INTO NAME node VALUE v)
GROUP BY ALL;

-- The target: how many points quitting cuts shortness of breath, in each
-- graph: P(breathless | do(smoke)) - P(breathless | do(no smoke)), adjusted
-- for smoking's parents in that graph (none, lung cancer or bronchitis).
CREATE OR REPLACE TEMP TABLE cg_effect AS
WITH adj AS (
  SELECT graph, bool_or(b = 'lung' AND dir = 'b_to_a') AS by_lung, bool_or(b = 'bronc' AND dir = 'b_to_a') AS by_bronc
  FROM cg_graphs WHERE a = 'smoke' GROUP BY graph
),
strata AS (
  SELECT g.graph, CASE WHEN by_lung THEN lung END AS zl, CASE WHEN by_bronc THEN bronc END AS zb, smoke,
         sum(p) AS p, sum(p) FILTER (WHERE dysp) AS pd
  FROM adj g, cg_joint GROUP BY ALL
),
z AS (
  SELECT graph, zl, zb, sum(p) AS pz,
         sum(pd) FILTER (WHERE smoke) / sum(p) FILTER (WHERE smoke) AS r1,
         sum(pd) FILTER (WHERE NOT smoke) / sum(p) FILTER (WHERE NOT smoke) AS r0
  FROM strata GROUP BY ALL
)
SELECT graph, round(100 * sum(pz * (r1 - r0)), 1) AS effect FROM z GROUP BY graph;

-- The experts' answers: the input table (a row per expert and question).
-- id is the question (a|b), on a locked server keyed together with who; the
-- rows below are a simulated first two rounds.
CREATE OR REPLACE TABLE answers AS
FROM (VALUES
  ('smoke|lung|dr-a', 'a_to_b', 'Cohort studies since the 1950s: the smoking comes first, and a cancer does not make people start.', 'dr-a', '2026-11-12', false),
  ('smoke|bronc|dr-b', 'a_to_b', 'Smoke inflames the airways; chronic bronchitis follows within years.', 'dr-b', '2026-11-14', false),
  ('smoke|bronc|dr-c', 'b_to_a', 'In our clinic, patients with a chronic cough often take up smoking to ease it.', 'dr-c', '2026-11-15', false)
) t(id, answer, rationale, who, "when", decides);

-- Every answer with its question's ends; an answer without a name is "you".
CREATE OR REPLACE TEMP VIEW cg_answers AS
SELECT split_part(id, '|', 1) AS a, split_part(id, '|', 2) AS b, answer,
       coalesce(nullif(trim(who), ''), 'you') AS who, "when", rationale,
       coalesce(decides, false) AS decides, id
FROM answers
WHERE answer IS NOT NULL;

-- Where each question stands: settled when an approver decided it or every
-- expert's latest firm answer agrees, disputed when they differ, else open.
CREATE OR REPLACE TEMP VIEW cg_status AS
WITH latest AS (
  SELECT * FROM cg_answers
  QUALIFY row_number() OVER (PARTITION BY a, b, who, decides ORDER BY "when" DESC, id DESC) = 1
),
firm AS (SELECT * FROM latest WHERE answer IN ('a_to_b', 'b_to_a', 'no_arc')),
dec AS (
  SELECT a, b, arg_max(answer, "when") AS answer, arg_max(who, "when") AS who,
         max("when") AS "when", arg_max(rationale, "when") AS rationale
  FROM firm WHERE decides GROUP BY a, b
),
agree AS (
  SELECT a, b, list(DISTINCT answer) AS answers, arg_min(who, "when") AS who,
         min("when") AS "when", arg_min(rationale, "when") AS rationale
  FROM firm WHERE NOT decides GROUP BY a, b
)
SELECT o.q, o.a, o.b, o.text,
       CASE WHEN d.answer IS NOT NULL OR len(g.answers) = 1 THEN 'settled'
            WHEN len(g.answers) > 1 THEN 'disputed' ELSE 'open' END AS status,
       coalesce(d.answer, CASE WHEN len(g.answers) = 1 THEN g.answers[1] END) AS answer,
       coalesce(d.who, g.who) AS who, coalesce(d."when", g."when") AS "when",
       coalesce(d.rationale, g.rationale) AS rationale
FROM cg_open o LEFT JOIN dec d USING (a, b) LEFT JOIN agree g USING (a, b);

-- The graphs every settled answer still allows.
CREATE OR REPLACE TEMP VIEW cg_fit AS
SELECT g.graph FROM cg_graphs g
LEFT JOIN cg_status s ON s.a = g.a AND s.b = g.b AND s.status = 'settled' AND s.answer IN ('a_to_b', 'b_to_a')
GROUP BY g.graph
HAVING bool_and(s.answer IS NULL OR s.answer = g.dir);

-- The arcs as they stand: an answer turns an open arc into an expert one
-- (its way round), "no direct link" drops it; an open arc every graph left
-- orients the same way follows from the answers.
CREATE OR REPLACE TEMP VIEW cg_arcs AS
WITH one_way AS (
  SELECT g.a, g.b, min(g.dir) AS dir FROM cg_graphs g JOIN cg_fit USING (graph)
  GROUP BY g.a, g.b HAVING count(DISTINCT g.dir) = 1
),
way AS (
  SELECT l.*, s.status AS q_status, s.who, s."when", s.rationale,
         CASE WHEN s.status = 'settled' THEN s.answer ELSE w.dir END AS dir
  FROM cg_learned l
  LEFT JOIN cg_status s ON s.a = l."from" AND s.b = l."to"
  LEFT JOIN one_way w ON w.a = l."from" AND w.b = l."to" AND l.status = 'undirected'
)
SELECT k,
       CASE WHEN dir = 'b_to_a' THEN "to" ELSE "from" END AS "from",
       CASE WHEN dir = 'b_to_a' THEN "from" ELSE "to" END AS "to",
       CASE WHEN q_status = 'settled' AND dir = 'no_arc' THEN 'forbidden'
            WHEN q_status = 'settled' THEN 'expert'
            WHEN dir IS NOT NULL THEN 'directed'
            ELSE status END AS status,
       lean,
       CASE WHEN q_status = 'settled' THEN who END AS who,
       CASE WHEN q_status = 'settled' THEN "when" END AS "when",
       CASE WHEN q_status = 'settled' THEN rationale END AS rationale
FROM way;

-- An effect range as text: "20–23.4 points", one value, or ruled out.
CREATE OR REPLACE TEMP MACRO cg_range(lo, hi) AS
  CASE WHEN lo IS NULL THEN 'ruled out' WHEN lo = hi THEN lo || ' points' ELSE lo || '–' || hi || ' points' END;

-- The questions, ranked: for each answer, the effect's range over the graphs
-- it leaves, whether that decides the programme, and how many other open
-- arcs it orients.
CREATE OR REPLACE TEMP VIEW cg_questions AS
WITH per AS (
  SELECT o.q, g.dir, e.graph, e.effect
  FROM cg_open o JOIN cg_graphs g ON g.a = o.a AND g.b = o.b
  JOIN cg_fit f ON f.graph = g.graph JOIN cg_effect e ON e.graph = g.graph
),
rng AS (SELECT q, dir, min(effect) AS lo, max(effect) AS hi FROM per GROUP BY ALL),
open_now AS (
  SELECT g.a || '|' || g.b AS q2 FROM cg_graphs g JOIN cg_fit USING (graph)
  GROUP BY ALL HAVING count(DISTINCT g.dir) > 1
),
orients AS (
  SELECT p.q, p.dir, count(*) AS n FROM (
    SELECT p.q, p.dir, g2.a || '|' || g2.b AS q2, count(DISTINCT g2.dir) AS nd
    FROM per p JOIN cg_graphs g2 ON g2.graph = p.graph AND g2.a || '|' || g2.b <> p.q
    GROUP BY ALL
  ) p JOIN open_now USING (q2) WHERE nd = 1 GROUP BY ALL
),
thr AS (SELECT coalesce(getvariable('cg_threshold'), 10)::DOUBLE AS t),
card AS (
  SELECT s.q, s.a, s.b, s.text, s.status, l.lean, na.label AS la, nb.label AS lb,
         ab.lo AS ab_lo, ab.hi AS ab_hi, ba.lo AS ba_lo, ba.hi AS ba_hi,
         coalesce(oab.n, 0) AS oriented_if_ab, coalesce(oba.n, 0) AS oriented_if_ba,
         (ab.lo >= t OR ab.hi < t) AS ab_decides, (ba.lo >= t OR ba.hi < t) AS ba_decides,
         t
  FROM cg_status s CROSS JOIN thr
  JOIN cg_learned l ON l."from" = s.a AND l."to" = s.b
  JOIN cg_nodes na ON na.id = s.a JOIN cg_nodes nb ON nb.id = s.b
  LEFT JOIN rng ab ON ab.q = s.q AND ab.dir = 'a_to_b'
  LEFT JOIN rng ba ON ba.q = s.q AND ba.dir = 'b_to_a'
  LEFT JOIN orients oab ON oab.q = s.q AND oab.dir = 'a_to_b'
  LEFT JOIN orients oba ON oba.q = s.q AND oba.dir = 'b_to_a'
)
SELECT row_number() OVER (ORDER BY status = 'settled', coalesce(ab_decides, false)::INT + coalesce(ba_decides, false)::INT DESC,
                                   oriented_if_ab + oriented_if_ba DESC, q) AS rank,
       *,
       status <> 'settled' AND (coalesce(ab_decides, false) OR coalesce(ba_decides, false))
         AND NOT (coalesce(ab_lo, 0) = coalesce(ba_lo, 0) AND coalesce(ab_hi, 0) = coalesce(ba_hi, 0)) AS changes_target
FROM card;

-- The effect's range per round: after the first k settled questions (in
-- the order they were settled), over the graphs those answers allow.
CREATE OR REPLACE TEMP VIEW cg_rounds AS
WITH st AS (
  SELECT a, b, answer, row_number() OVER (ORDER BY "when", q) AS k FROM cg_status WHERE status = 'settled'
),
r AS (SELECT 0 AS k UNION ALL SELECT k FROM st)
SELECT r.k AS round, min(e.effect) AS lo, max(e.effect) AS hi
FROM r, cg_effect e
WHERE NOT EXISTS (
  SELECT 1 FROM cg_graphs g JOIN st ON st.a = g.a AND st.b = g.b
  WHERE g.graph = e.graph AND st.k <= r.k AND st.answer IN ('a_to_b', 'b_to_a') AND st.answer <> g.dir
)
GROUP BY r.k;

-- Answers to check: unknown nodes, and an answer that would close a cycle
-- with the arcs that are directed now (the cycle named; the arc between the
-- two ends themselves doesn't count, an answer against it is a disagreement).
CREATE OR REPLACE TEMP VIEW cg_checks AS
WITH RECURSIVE directed AS (
  SELECT "from" AS u, "to" AS v FROM cg_arcs WHERE status NOT IN ('undirected', 'forbidden')
),
asked AS (
  SELECT who, a, b, CASE answer WHEN 'a_to_b' THEN a ELSE b END AS f, CASE answer WHEN 'a_to_b' THEN b ELSE a END AS t
  FROM cg_answers WHERE answer IN ('a_to_b', 'b_to_a')
),
walk(who, f, t, node, path, names) AS (
  SELECT x.who, x.f, x.t, x.t, [x.t], [nf.label, nt.label]
  FROM asked x JOIN cg_nodes nf ON nf.id = x.f JOIN cg_nodes nt ON nt.id = x.t
  UNION ALL
  SELECT w.who, w.f, w.t, d.v, list_append(w.path, d.v), list_append(w.names, n.label)
  FROM walk w JOIN directed d ON d.u = w.node JOIN cg_nodes n ON n.id = d.v
  -- not along the arc between the two: overruling it is a disagreement
  WHERE NOT list_contains(w.path, d.v) AND len(w.path) < 10
    AND NOT (d.u IN (w.f, w.t) AND d.v IN (w.f, w.t))
)
SELECT DISTINCT who, 'would close a cycle: ' || array_to_string(names, ' → ') AS problem
FROM walk WHERE node = f AND f <> t
UNION ALL
SELECT who, 'names ' || x || ', which is no node of the network'
FROM (SELECT who, unnest([a, b]) AS x FROM cg_answers) WHERE x NOT IN (SELECT id FROM cg_nodes);

SELECT 'Where the graph is still open'::SECTION;

SELECT 10 AS cg_threshold ::NUMBER, 'The programme pays off from (points less breathlessness)'::LABEL;

SELECT 4::SPAN;
SELECT CASE WHEN lo >= t THEN 'pays off' WHEN hi < t THEN 'does not pay off' ELSE 'open' END ::TEXT_LARGE,
       'The quit-smoking programme' ::LABEL,
       CASE WHEN lo = hi THEN 'Quitting cuts breathlessness by ' || lo || ' points.'
            ELSE 'Quitting cuts breathlessness by ' || lo || ' to ' || hi || ' points, depending on the open arrows.' END ::SUBTITLE
FROM (SELECT * FROM cg_rounds ORDER BY round DESC LIMIT 1),
     (SELECT coalesce(getvariable('cg_threshold'), 10)::DOUBLE AS t);

SELECT 8::SPAN;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(x ORDER BY x.k) FROM cg_arcs x),
         beliefs := (SELECT list(b) FROM cg_beliefs b),
         answers := (SELECT list(a) FROM cg_answers a))
       AS "The learned network: arrows without heads are open, ✓ was set by an expert"
FROM cg_nodes n;

SELECT 'The questions'::SECTION;

SELECT 12::SPAN;
SELECT 'none'::TOOLBAR;
SELECT rank AS "#" ::TABLE, text AS "Question",
       la || ' → ' || lb || ': ' || CASE WHEN ab_lo IS NULL THEN 'ruled out'
         ELSE cg_range(ab_lo, ab_hi)
              || CASE WHEN ab_decides THEN ', decides' ELSE '' END
              || CASE WHEN oriented_if_ab > 0 THEN ', orients ' || oriented_if_ab || ' more' ELSE '' END END AS "If yes",
       lb || ' → ' || la || ': ' || CASE WHEN ba_lo IS NULL THEN 'ruled out'
         ELSE cg_range(ba_lo, ba_hi)
              || CASE WHEN ba_decides THEN ', decides' ELSE '' END
              || CASE WHEN oriented_if_ba > 0 THEN ', orients ' || oriented_if_ba || ' more' ELSE '' END END AS "If the other way",
       CASE WHEN changes_target THEN 'yes' ELSE 'no' END AS "Moves the decision",
       CASE WHEN lean >= 0.5 THEN round(100 * lean)::INT || ' % ' || la || ' → ' || lb
            ELSE round(100 * (1 - lean))::INT || ' % ' || lb || ' → ' || la END AS "The data lean",
       status AS "Status" ::BADGE
FROM cg_questions ORDER BY rank;

-- the question to answer (the top one first)
SELECT q ::DROPDOWN, '#' || rank || ' ' || text ::OPTION_LABEL, 'Question' ::LABEL
FROM cg_questions ORDER BY rank;

SELECT 7::SPAN;
SELECT field AS cg_answer ::FORM, type, value, options, labels, label, help,
       text ::TITLE, sub ::SUBTITLE
FROM (
  SELECT c.*, f.*,
         'If ' || la || ' → ' || lb || ': ' || cg_range(ab_lo, ab_hi)
         || '. If ' || lb || ' → ' || la || ': ' || cg_range(ba_lo, ba_hi)
         || '. The data lean ' || CASE WHEN lean >= 0.5 THEN round(100 * lean)::INT || ' % ' || la || ' → ' || lb
                                       ELSE round(100 * (1 - lean))::INT || ' % ' || lb || ' → ' || la END
         || CASE status WHEN 'disputed' THEN '. Experts disagree: see below.' WHEN 'settled' THEN '. Settled; a new answer reopens it only if it differs.' ELSE '.' END AS sub
  FROM cg_questions c,
  LATERAL (
    SELECT 1 AS ord, 'answer' AS field, 'buttons' AS type, NULL AS value,
           ['a_to_b', 'b_to_a', 'no_arc', 'unsure'] AS options,
           [c.la || ' → ' || c.lb, c.lb || ' → ' || c.la, 'No direct link', 'Not sure'] AS labels,
           'Your answer' AS label, NULL AS help
    UNION ALL SELECT 2, 'rationale', 'text', NULL, NULL, NULL, 'Why', 'What you know that the data don''t'
    UNION ALL SELECT 3, 'who', 'text', NULL, NULL, NULL, 'Your name', 'On a server the login is used instead'
      WHERE getvariable('__claim_user') IS NULL
    UNION ALL SELECT 4, 'when', 'date', current_date::VARCHAR, NULL, NULL, 'Date', NULL
      WHERE getvariable('__claim_user') IS NULL
    UNION ALL SELECT 5, 'decides', 'toggle', 'false', NULL, NULL, 'Settle the disagreement', 'As the approver: this answer decides the question'
      WHERE c.status = 'disputed'
  ) f
  WHERE c.q = getvariable('q')
) ORDER BY ord;

SELECT 5::SPAN;
SELECT 'Submit answer' ::SAVE, 'cg_answer' ::SAVE_FORM, 'answers' ::SAVE_TABLES, 'q' ::SAVE_KEY;

SELECT 'Where experts disagree'::SECTION;
SELECT 12::SPAN;
SELECT 'none'::TOOLBAR;
SELECT s.text AS "Question", x.who AS "Expert",
       CASE x.answer WHEN 'a_to_b' THEN na.label || ' → ' || nb.label WHEN 'b_to_a' THEN nb.label || ' → ' || na.label
            WHEN 'no_arc' THEN 'No direct link' ELSE 'Not sure' END AS "Answer",
       x."when" AS "When", x.rationale AS "Why" ::TABLE
FROM cg_status s JOIN cg_answers x ON x.a = s.a AND x.b = s.b
JOIN cg_nodes na ON na.id = s.a JOIN cg_nodes nb ON nb.id = s.b
WHERE s.status = 'disputed' AND NOT x.decides
QUALIFY row_number() OVER (PARTITION BY x.a, x.b, x.who ORDER BY x."when" DESC, x.id DESC) = 1
ORDER BY s.q, x."when";

SELECT 'Settling the effect'::SECTION;

SELECT 7::SPAN;
SELECT round ::XAXIS, which ::CATEGORY, v ::LINECHART,
       (SELECT coalesce(getvariable('cg_threshold'), 10)) ::REFLINE, 'Pays off above' ::REFLINE_LABEL,
       (SELECT min(round) FROM cg_rounds, (SELECT coalesce(getvariable('cg_threshold'), 10) AS t) WHERE lo >= t OR hi < t) ::XLINE,
       'Decided' ::XLINE_LABEL,
       (SELECT string_agg(round::VARCHAR, ',' ORDER BY round) FROM cg_rounds) ::XBREAKS,
       'Questions settled (rounds)' ::XTITLE, 'Points less breathlessness' ::YTITLE,
       'The effect of quitting, over every graph still possible' ::TITLE
FROM (UNPIVOT cg_rounds ON hi AS 'highest', lo AS 'lowest' INTO NAME which VALUE v)
ORDER BY round, which;

SELECT 5::SPAN;
SELECT '**Reading it.** Each round is one more settled question. The two lines are the smallest and the largest effect of quitting over the graphs the data and the answers still allow. While they straddle the threshold the decision depends on the open arrows; where both sit on one side, *Decided* marks the round from which further questions no longer change it.

**Who.** In the browser an answer names its expert in *Your name* and stays in this tab. On a locked server the `answers` table is declared in `options.writeback` with `"stamp": {"who": "user", "when": "ts"}`: who is the login, when the server''s clock, and each expert keeps a row of their own per question. With `"approver_only": ["decides"]` and `options.writeback.approvers` only an approver can settle a disagreement; in the browser anyone can.' ::MARKDOWN;

SELECT 'The answers'::SECTION;

SELECT 'all' AS cg_expert ::DROPDOWN, 'Expert' ::LABEL
UNION ALL SELECT DISTINCT who, NULL FROM cg_answers ORDER BY 1;

SELECT x.who AS "Who", s.text AS "Question",
       CASE x.answer WHEN 'a_to_b' THEN na.label || ' → ' || nb.label WHEN 'b_to_a' THEN nb.label || ' → ' || na.label
            WHEN 'no_arc' THEN 'No direct link' ELSE 'Not sure' END AS "Answer",
       CASE WHEN x.decides THEN 'decision' ELSE 'answer' END AS "Kind",
       x."when" AS "When", x.rationale AS "Why" ::TABLE
FROM cg_answers x JOIN cg_status s ON s.a = x.a AND s.b = x.b
JOIN cg_nodes na ON na.id = x.a JOIN cg_nodes nb ON nb.id = x.b
WHERE coalesce(getvariable('cg_expert'), 'all') IN ('all', x.who)
ORDER BY x."when" DESC, x.id DESC;

SELECT 'No answer to check: none names an unknown node or closes a cycle.' ::EMPTY;
SELECT 'Answers to check' ::TITLE, who AS "Who", problem AS "Problem" ::TABLE FROM cg_checks;
