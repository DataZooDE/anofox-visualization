-- Who has finished the onboarding quiz? Head office's view of
-- `tutorial.sql`'s answers (#517's "trainer" read). Note what this
-- dashboard does *not* do: no `WHERE user_name = …`, no group check in
-- its own SQL at all. `options.state.trainers` (`dashboards: ["progress"],
-- groups: ["admins"]`) decides, server-side, whose rows ever reach this
-- connection: a head-office viewer (group `admins`) gets every row
-- below; anyone else opening this exact same page gets only their own —
-- fails safe, never refused, and nothing here could get it wrong even by
-- accident, since the rows that aren't theirs were simply never put in
-- front of them.

SELECT 'Who has finished the onboarding quiz?'::DASHBOARD_TITLE;
SELECT 'en-GB'::LOCALE;
SELECT 'Signed in as **' || coalesce(getvariable('__claim_user'), 'nobody')
    || '**. Head office (the ''admins'' group) sees every row here; '
    || 'anyone else sees only their own.'::MARKDOWN;

SELECT 12::COL;
SELECT count(DISTINCT "user")::METRIC, 'Staff seen here'::LABEL FROM viewer_state;
SELECT count(*) FILTER (WHERE submit = 'step_2')::METRIC, 'Finished step 2'::LABEL
FROM viewer_state_latest;

SELECT 12::COL;
SELECT "user" AS "Staff member",
       max(ts) FILTER (WHERE submit = 'step_1') AS "Step 1 at",
       max(ts) FILTER (WHERE submit = 'step_2') AS "Step 2 at" ::TABLE,
       'Latest attempt per step'::TITLE
FROM viewer_state GROUP BY "user" ORDER BY "user";
