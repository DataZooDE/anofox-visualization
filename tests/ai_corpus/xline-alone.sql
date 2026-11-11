-- @from full-claude-sonnet-5-20261111-125334 ts-event-line: event lines in a statement of their own after the chart — drawn nowhere
-- @expect annotation/no-chart draws nothing — put them in the chart's own SELECT
CREATE TABLE monthly AS SELECT * FROM (VALUES (DATE '2025-01-01', 40), (DATE '2025-02-01', 38), (DATE '2025-03-01', 45)) t(m, n);
CREATE TABLE disruptions AS SELECT * FROM (VALUES (DATE '2025-01-20', 'Lunar New Year')) t(start_date, what);
SELECT m::XAXIS, n::BARCHART AS "Order lines", 'Order lines per month'::TITLE FROM monthly ORDER BY 1;
SELECT start_date::XLINE, what::XLINE_LABEL FROM disruptions;
