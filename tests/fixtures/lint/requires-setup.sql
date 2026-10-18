-- @title Which week sold the most? (optional inputs on setup)
-- @group Lint fixtures
--
-- `-- @requires` on setup statements, for the lint parity tests
-- (tests/lint_step.rs, tests/web/lint_test.mjs): a SET VARIABLE whose
-- subquery reads the file it requires is fine (#166); one that only builds a
-- path reads no file (`requires/on-setup`).

-- @requires data/supply-chain/weeks.csv
SET VARIABLE n_weeks = (SELECT count(*) FROM read_csv('data/supply-chain/weeks.csv'));

-- @requires data/supply-chain/weeks.csv
SET VARIABLE weeks_file = 'data/supply-chain/weeks.csv';

SELECT getvariable('n_weeks') ::METRIC, 'Weeks on file'::TITLE;
