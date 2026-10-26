-- @title Which check would have caught this drift?
-- @group How it works
-- @tags  show_if, quiz, interactivity
-- @description A one-question quiz: `::SHOW_IF` reveals the explanation only after an answer, and a `::BUTTONS` switch shows one of two boxes.
--
-- `::SHOW_IF` (#515): a panel, or a whole `::GROUP` box, shows only while a
-- BOOLEAN condition is TRUE — re-checked on every input change. See
-- docs/DOCS.md, "Show a panel only when …".

SELECT 'A model''s input stayed steady, but it started missing. **Which check would have caught it first?**'::MARKDOWN;

SELECT v AS q1 ::DROPDOWN, l ::OPTION_LABEL, 'Your answer'::LABEL
FROM (VALUES (1, '', '— choose —'),
             (2, 'input drift', NULL),
             (3, 'measured error', NULL)) t(o, v, l)
ORDER BY o;

-- The explanation only appears once the dropdown leaves its empty default
-- (a ::DROPDOWN defaults to its first option, so check `<> ''`, not `IS NOT NULL`).
SELECT getvariable('q1') <> '' ::SHOW_IF;
SELECT CASE getvariable('q1')
         WHEN 'measured error' THEN '**Right.** Input drift can sit still while the relationship between input and outcome moves — only watching the outcome catches that.'
         ELSE 'Not quite: input drift can stay flat while the model''s error grows underneath it.'
       END ::MARKDOWN;

SELECT v AS q1_view ::BUTTONS, l ::OPTION_LABEL, 'See it as a'::LABEL
FROM (VALUES (1, 'chart', 'Chart'), (2, 'table', 'Table')) t(o, v, l)
ORDER BY o;

SELECT getvariable('q1_view') = 'chart' ::SHOW_IF;
SELECT 'Chart view'::GROUP;
SELECT day ::XAXIS, metric ::CATEGORY, value ::LINECHART, 'Input vs. outcome, day by day'::TITLE
FROM (VALUES (1, 'input', 0.50), (2, 'input', 0.51), (3, 'input', 0.49), (4, 'input', 0.50),
             (1, 'outcome error', 0.02), (2, 'outcome error', 0.05), (3, 'outcome error', 0.11), (4, 'outcome error', 0.22)
      ) t(day, metric, value)
ORDER BY day;
SELECT 1::ENDGROUP;

SELECT getvariable('q1_view') = 'table' ::SHOW_IF;
SELECT 'Table view'::GROUP;
SELECT day AS "Day" ::TABLE, metric AS "Series", value AS "Value"
FROM (VALUES (1, 'input', 0.50), (2, 'input', 0.51), (3, 'input', 0.49), (4, 'input', 0.50),
             (1, 'outcome error', 0.02), (2, 'outcome error', 0.05), (3, 'outcome error', 0.11), (4, 'outcome error', 0.22)
      ) t(day, metric, value)
ORDER BY day, metric;
SELECT 1::ENDGROUP;
