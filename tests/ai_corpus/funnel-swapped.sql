-- @from coverage-20261110-0707 funnel: the stages on ::FUNNEL, the counts on ::XAXIS
-- @expect sql-error put the text on ::XAXIS
CREATE TABLE stages AS SELECT * FROM (VALUES (1, 'All leads', 300), (2, 'Qualified', 120), (3, 'Converted', 11)) t(ord, stage, n);
SELECT stage::FUNNEL, n::XAXIS, 'Lead funnel'::TITLE FROM stages ORDER BY ord;
