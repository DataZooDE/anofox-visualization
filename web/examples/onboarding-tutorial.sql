-- @title How do I tell input drift from concept drift?
-- @group How it works
-- @tags  guide_step, tutorial, quiz, show_if, gate
-- @description A four-step guided tutorial: `::GUIDE_STEP` adds Back/Next, a progress strip, and a `::GATE` that holds Next until the quiz is answered correctly.
--
-- `::GUIDE_STEP` (#516): a run of statements becomes a guided sequence —
-- Back/Next, "Step n of N", and an optional `::GATE` that holds Next until
-- a condition is met. See docs/DOCS.md, "Guided steps".
--
-- `dashboard --check` warns `step/gate-not-boolean` on step 2 below: the
-- gate reads `q2`, which its own dropdown (right after the marker) hasn't
-- set yet by the time the checker's single sequential pass evaluates the
-- marker — the same ordering note as `::SHOW_IF`'s own quiz pattern. The
-- interactive browser never sees this: its input pre-pass binds every
-- dropdown's default *before* any query runs, gate included, so Next opens
-- correctly from the very first load.

SELECT 'The three doors' AS lesson ::GUIDE_STEP;
SELECT 'A model''s accuracy is slipping. There are three places to look: the ' ||
       '**input** (did the data feeding it change?), the **output** (did its ' ||
       'predictions drift?), or the **relationship** between them (did the world ' ||
       'change in a way the model never learned?). This tutorial walks through ' ||
       'telling these apart — answer a short check, then try it yourself.' ::MARKDOWN;

-- The gate lives on the SAME step as its own quiz (decision 2 of
-- plan-516.md): Next (to the sandbox) stays disabled until it is answered
-- correctly, right here.
SELECT 'Check your understanding' ::GUIDE_STEP,
       getvariable('q2') = 'measured error' ::GATE,
       'Answer correctly above to continue' ::GATE_MESSAGE;
SELECT v AS q2 ::DROPDOWN, l ::OPTION_LABEL, 'Which check sees concept drift?' ::LABEL
FROM (VALUES (1, '', '— choose —'),
             (2, 'input drift', NULL),
             (3, 'measured error', NULL)) t(o, v, l)
ORDER BY o;

-- The explanation only appears once the dropdown leaves its empty default
-- (a ::DROPDOWN defaults to its first option, so check `<> ''`, not `IS NOT NULL`).
SELECT getvariable('q2') <> '' ::SHOW_IF;
SELECT CASE getvariable('q2')
         WHEN 'measured error' THEN '**Right.** Concept drift is a change in the *relationship* between input and outcome — watching the error directly catches it even when the input distribution looks unchanged.'
         ELSE 'Not quite: input drift watches the data coming in, which can stay flat while the relationship underneath it moves. Try again above to continue.'
       END ::MARKDOWN;

SELECT 'Drift sandbox' ::GUIDE_STEP;
SELECT 'Flip between the two signals below. Notice the input barely moves while the error climbs — that is concept drift.' ::MARKDOWN;

SELECT v AS q3 ::BUTTONS, l ::OPTION_LABEL, 'See it as a' ::LABEL
FROM (VALUES (1, 'chart', 'Chart'), (2, 'table', 'Table')) t(o, v, l)
ORDER BY o;

SELECT getvariable('q3') = 'chart' ::SHOW_IF;
SELECT 'Chart view' ::GROUP;
SELECT day ::XAXIS, metric ::CATEGORY, value ::LINECHART, 'Input vs. outcome, day by day' ::TITLE
FROM (VALUES (1, 'input', 0.50), (2, 'input', 0.51), (3, 'input', 0.49), (4, 'input', 0.50),
             (1, 'outcome error', 0.02), (2, 'outcome error', 0.05), (3, 'outcome error', 0.11), (4, 'outcome error', 0.22)
      ) t(day, metric, value)
ORDER BY day;
SELECT 1 ::ENDGROUP;

SELECT getvariable('q3') = 'table' ::SHOW_IF;
SELECT 'Table view' ::GROUP;
SELECT day AS "Day" ::TABLE, metric AS "Series", value AS "Value"
FROM (VALUES (1, 'input', 0.50), (2, 'input', 0.51), (3, 'input', 0.49), (4, 'input', 0.50),
             (1, 'outcome error', 0.02), (2, 'outcome error', 0.05), (3, 'outcome error', 0.11), (4, 'outcome error', 0.22)
      ) t(day, metric, value)
ORDER BY day, metric;
SELECT 1 ::ENDGROUP;

SELECT 'Wrap-up' ::GUIDE_STEP;
SELECT 'Input drift and concept drift look alike from the outside — both cost accuracy — but need different fixes: input drift usually wants retraining on fresher data; concept drift means the *relationship* has to be relearned. Use the step list above to revisit any part of this.' ::MARKDOWN;
