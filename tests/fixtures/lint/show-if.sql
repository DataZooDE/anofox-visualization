-- @title Did the reader answer yet? (a broken draft)
-- @group Lint fixtures
--
-- A `::SHOW_IF` dashboard with mistakes on purpose, for the lint parity
-- tests (tests/lint_step.rs, tests/web/lint_test.mjs): each statement below
-- trips one `show_if/*` diagnostic (#515).

SELECT v AS q1 ::DROPDOWN, l ::OPTION_LABEL FROM
  (VALUES (1, '', '— choose —'), (2, 'a', NULL)) t(o, v, l) ORDER BY o;

-- show_if/not-alone: a ::SHOW_IF column beside another role is ignored.
SELECT getvariable('q1') <> '' ::SHOW_IF, 1 ::COL;
SELECT 'Shown regardless'::MARKDOWN;

-- show_if/constant: reads no variable, so it never changes.
SELECT true ::SHOW_IF;
SELECT 'Always the same answer'::MARKDOWN;

-- show_if/unknown-variable: `typo_q1` is not a variable this dashboard sets.
SELECT getvariable('typo_q1') <> '' ::SHOW_IF;
SELECT 'Checks a variable that is never set'::MARKDOWN;

-- show_if/not-boolean: the cell is a VARCHAR, not a BOOLEAN.
SELECT 'yes' ::SHOW_IF;
SELECT 'A non-boolean condition'::MARKDOWN;

-- show_if/no-row: the condition's own query returns no row.
SELECT getvariable('q1') <> '' ::SHOW_IF WHERE false;
SELECT 'Never reached — the condition has no row'::MARKDOWN;

-- show_if/bad-target: ::TAB can never be a ::SHOW_IF target.
SELECT getvariable('q1') <> '' ::SHOW_IF;
SELECT 'Detail'::TAB;
SELECT a ::XAXIS, b ::BARCHART FROM (VALUES (1, 2)) t(a, b);

-- show_if/no-target: nothing follows this condition at all.
SELECT getvariable('q1') <> '' ::SHOW_IF;
