-- @title Did I really finish this lesson? (a broken draft)
-- @group Lint fixtures
--
-- A `::GUIDE_STEP` dashboard with mistakes on purpose, for the lint parity
-- tests (tests/lint_step.rs, tests/web/lint_test.mjs): each block below
-- trips one or more `step/*` diagnostics (#516).

-- step/lone: a guided sequence with a single step.
SELECT 'Lone step'::GUIDE_STEP;
SELECT 'Nothing to go back or forward to'::MARKDOWN;
SELECT 'p1'::TAB;

-- step/empty: no rendering statement in this step.
SELECT 'Empty step'::GUIDE_STEP;
SELECT 'Filled step'::GUIDE_STEP;
SELECT 'Something to show'::MARKDOWN;
SELECT 'p2'::TAB;

-- step/duplicate-label: the same label twice in one sequence.
SELECT 'Repeat'::GUIDE_STEP;
SELECT 'a'::MARKDOWN;
SELECT 'Repeat'::GUIDE_STEP;
SELECT 'b'::MARKDOWN;
SELECT 'p3'::TAB;

-- step/computed-label: not a plain string literal.
SELECT v || '!' ::GUIDE_STEP FROM (VALUES ('Computed')) t(v);
SELECT 'x'::MARKDOWN;
SELECT 'p4'::TAB;

-- step/fit-inside: ::FIT does nothing inside a step.
SELECT 'Fit step'::GUIDE_STEP;
SELECT 'fit'::FIT;
SELECT 'y'::MARKDOWN;
SELECT 'p5'::TAB;

-- step/mixed-with-subtab: a ::SUBTAB closes an open sequence.
SELECT 'Mixed'::GUIDE_STEP;
SELECT 'z'::MARKDOWN;
SELECT 'sub'::SUBTAB;
SELECT 'p6'::TAB;

-- step/inside-subtab: a marker can never open inside a ::SUBTAB.
SELECT 'sub2'::SUBTAB;
SELECT 'Nested'::GUIDE_STEP;
SELECT 'w'::MARKDOWN;
SELECT 'p7'::TAB;

-- step/var-clash: the group's own variable is also an input's.
SELECT v AS step ::DROPDOWN FROM (VALUES ('a')) t2(v);
SELECT 'Clash 1'::GUIDE_STEP;
SELECT 'c1'::MARKDOWN;
SELECT 'Clash 2'::GUIDE_STEP;
SELECT 'c2'::MARKDOWN;
SELECT 'p8'::TAB;

-- step/gate-without-step: ::GATE beside something other than ::GUIDE_STEP.
SELECT true ::GATE, 'Stray gate'::MARKDOWN;
SELECT 'p9'::TAB;

-- step/gate-on-last: a ::GATE on the sequence's last step does nothing.
SELECT 'First'::GUIDE_STEP;
SELECT 'a2'::MARKDOWN;
SELECT 'Last' ::GUIDE_STEP, true ::GATE;
SELECT 'b2'::MARKDOWN;
SELECT 'p10'::TAB;

-- step/gate-constant: the gate reads no variable, so it never changes.
SELECT 'Gate1' ::GUIDE_STEP, true ::GATE;
SELECT 'c3'::MARKDOWN;
SELECT 'Gate2'::GUIDE_STEP;
SELECT 'd2'::MARKDOWN;
SELECT 'p11'::TAB;

-- step/gate-unknown-variable: `typo_q` is not a variable this dashboard sets.
SELECT 'Gate3' ::GUIDE_STEP, getvariable('typo_q') = 'yes' ::GATE;
SELECT 'e2'::MARKDOWN;
SELECT 'Gate4'::GUIDE_STEP;
SELECT 'f2'::MARKDOWN;
