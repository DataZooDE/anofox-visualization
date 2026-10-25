-- @title A gated table's own ::ROWSELECT (#504)
-- @group Lint fixtures
--
-- `-- @requires` right above a content statement gates that statement out,
-- but a layout directive in front of it (here `::ROWSELECT`, with its own
-- `-- @shared`) is a separate statement the lint reaches regardless — it
-- must be gated out *with* the table it was waiting for, not carried over
-- to the next, unrelated table. Before the fix this showed up as a false
-- `interaction/shared-unused` on the "Method" table below: the gated
-- `series` table's `-- @shared series` got checked against "Method"
-- instead, since the gated table never consumed it.

-- @shared series
SELECT 'single'::ROWSELECT;

-- @requires data/does-not-exist/missing-504.parquet
SELECT unique_id AS series ::TABLE
FROM read_parquet('data/does-not-exist/missing-504.parquet');

SELECT method AS "Method" ::TABLE, 1 AS "Series"
FROM (VALUES ('a'), ('b')) t(method);
