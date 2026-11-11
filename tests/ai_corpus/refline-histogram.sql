-- @from full-claude-sonnet-5-20261111-125334 dist-mean-median: mean and median as ::REFLINE on a histogram — no line drawn
-- @expect chart/refline-histogram mark a value (mean, median) on its x axis with ::XLINE
CREATE TABLE claims AS SELECT * FROM (VALUES (1), (2), (2), (3), (10), (40)) t(days_to_report);
SELECT days_to_report::XAXIS, count(*)::HISTOGRAM, 9.7::REFLINE, 'Mean'::REFLINE_LABEL, 'Days to report'::TITLE
FROM claims GROUP BY ALL;
