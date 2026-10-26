-- @title Guided tours (#518)
-- @group Lint fixtures
--
-- `-- @tour <id> [title]` plus its step lines hits every `tour/*` code: a
-- clean tour (no diagnostic); a bad id; a duplicate id; an empty tour; an
-- unknown step target; an over-long step text; too many steps in one tour;
-- too many tours on one dashboard; `-- @tour_autostart` naming no tour; and
-- a stray, mis-indented step line with no `-- @tour` above it.
-- @owner Ops team
--   stray: this line looks like a tour step, but no -- @tour precedes it.
-- @tour_autostart no_such_tour

-- @name severity_kpi
SELECT 1 ::METRIC, 'Clean panel'::LABEL;

-- @tour cockpit_intro Cockpit in three steps
--   page: This page lists every monitored model.
--   severity_kpi: This is a model's severity.

-- @tour 1bad
--   page: Not a plain identifier.

-- @tour cockpit_intro
--   page: Reuses an earlier tour id.

-- @tour empty_tour

-- @tour unknown_target
--   no_such_panel: This name is on no statement's -- @name.

-- @tour long_text
--   page: This step's callout text is long on purpose, to trip the advisory length check. This step's callout text is long on purpose, to trip the advisory length check. This step's callout text is long on purpose, to trip the advisory length check. This step's callout text is long on purpose, to trip the advisory length check. This step's callout text is long on purpose, to trip the advisory length check. This step's callout text is long on purpose, to trip the advisory length check.

-- @tour too_many_steps
--   page: step 1
--   page: step 2
--   page: step 3
--   page: step 4
--   page: step 5
--   page: step 6
--   page: step 7
--   page: step 8
--   page: step 9
--   page: step 10
--   page: step 11
--   page: step 12
--   page: step 13
--   page: step 14
--   page: step 15
--   page: step 16
--   page: step 17
--   page: step 18
--   page: step 19
--   page: step 20
--   page: step 21

-- @tour filler_a
--   page: ok

-- @tour filler_b
--   page: ok

-- @tour filler_c
--   page: ok

-- @tour filler_d
--   page: ok
