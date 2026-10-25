-- @title Panel names (#518)
-- @group Lint fixtures
--
-- `-- @name <name>` tags the element a statement draws (a stable
-- `data-dp-name` anchor for e2e selectors and, later, a guided tour's
-- target). This fixture hits every `name/*` code: a well-formed, unique
-- name on a drawn statement (clean, no diagnostic); a name that isn't a
-- plain identifier (name/bad); the reserved `page` (name/reserved); a name
-- reused on another statement (name/duplicate); and a name on a statement
-- that draws nothing — setup, or an invisible layout directive
-- (name/not-drawn, twice).

-- @name severity_kpi
SELECT 1 ::METRIC, 'Clean'::LABEL;

-- @name 1bad
SELECT 2 ::METRIC, 'Not a plain identifier'::LABEL;

-- @name page
SELECT 3 ::METRIC, 'Reserved for a guided tour'::LABEL;

-- @name severity_kpi
SELECT 4 ::METRIC, 'Reuses an earlier name'::LABEL;

-- @name on_setup
SET VARIABLE later = 1;

-- @name on_span
SELECT 6::SPAN;
SELECT 5 ::METRIC, 'After an undrawn ::SPAN'::LABEL;
