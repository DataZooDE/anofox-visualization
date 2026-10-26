-- A two-step onboarding quiz for new staff, with answers recorded
-- append-only (#517). Served only with `serve.sh start --auth`: the
-- login proxy tells the server who is viewing, and `options.state`
-- (set only in that mode, since recording answers needs `auth`) makes
-- this dashboard's `::SUBMIT` buttons live. Step 2 only appears once
-- step 1 is answered — the author's own gate, read straight off
-- `viewer_state`, never a server-side "step" concept. "Who has finished
-- the quiz?" (head office's view of everyone's progress) is
-- `progress.sql`.

SELECT 'Staff onboarding quiz'::DASHBOARD_TITLE;
SELECT 'en-GB'::LOCALE;
SELECT 'Two quick questions — your answers are saved as you go, so you can '
    || 'leave and come back.'::MARKDOWN;

SELECT 'Step 1'::GROUP;
SELECT day ::BUTTONS, 'Which day do we count stock?'::LABEL
FROM (VALUES ('Monday'), ('Thursday'), ('Sunday')) t(day);

SELECT 'Check my answer' AS step_1 ::SUBMIT,
       'day'              AS r      ::SUBMIT_VARS,
       'Saved — on to step 2.'       ::SUBMIT_DONE;
SELECT 1::ENDGROUP;

-- Hidden until step 1 is actually recorded (an append-only read of this
-- viewer's own rows — the same rule the server enforces server-side,
-- just read back in SQL): never a separate "which step is this viewer
-- on" variable to keep in sync by hand. `dashboard --check` flags this
-- as `show_if/constant` ("reads no variable, so it never changes") —
-- a known false positive: its dependency analysis tracks viewer
-- *variables*, not `viewer_state`, which is exactly as dynamic per
-- request, just not through a `getvariable()` the lint can see.
SELECT (SELECT count(*) FROM viewer_state WHERE submit = 'step_1') > 0 ::SHOW_IF;
SELECT 'Step 2'::GROUP;
SELECT supplier ::BUTTONS, 'Where does next week''s flour come from?'::LABEL
FROM (VALUES ('Our own bakery'), ('A wholesaler'), ('Ask the shift lead')) t(supplier);

SELECT 'Check my answer' AS step_2 ::SUBMIT,
       'supplier'         AS r      ::SUBMIT_VARS,
       'All done — thanks!'          ::SUBMIT_DONE;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT submit AS "Step", name AS "Question", value AS "Your answer",
       ts AS "When" ::TABLE,
       'My progress'::TITLE
FROM viewer_state ORDER BY ts;
