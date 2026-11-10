-- @from coverage-20261110-0707 funnel: the version that worked
-- @expect clean
CREATE TABLE stages AS SELECT * FROM (VALUES (1, 'All leads', 300), (2, 'Qualified', 120), (3, 'Converted', 11)) t(ord, stage, n);
SELECT stage::XAXIS, n::FUNNEL, 'Lead funnel'::TITLE FROM stages ORDER BY ord;
