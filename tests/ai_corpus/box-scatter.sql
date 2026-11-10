-- @from coverage-20261110-0707 dist-box follow-up: a ::SCATTER beside a box from statistics
-- @expect box/missing takes no chart measure
CREATE TABLE lead AS SELECT s, (i % 7 + CASE s WHEN 'A' THEN 3 ELSE 6 END)::DOUBLE AS days, CASE s WHEN 'A' THEN 7 ELSE 10 END AS promised FROM (VALUES ('A'), ('B')) t(s), range(30) r(i);
SELECT s::XAXIS, quantile_cont(days, 0.25)::Q1, median(days)::MEDIAN, quantile_cont(days, 0.75)::Q3, max(promised)::SCATTER, 'Lead time'::TITLE FROM lead GROUP BY ALL ORDER BY 1;
