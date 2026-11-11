-- @from smoke-claude-sonnet-5-20261110-231930 combo-y2: ::Y2 in a statement of its own, before the chart (#1100)
-- @expect chart/y2-no-chart put it in the chart's own SELECT
CREATE TABLE sales AS SELECT * FROM (VALUES
  (DATE '2024-01-01', 10, 39.0), (DATE '2024-02-01', 12, 41.0), (DATE '2024-03-01', 15, 38.5)) t(month, units, price);
SELECT 'Avg Price'::Y2;
SELECT month::XAXIS, sum(units) AS "Units"::BARCHART, (sum(price*units)/sum(units)) AS "Avg Price"::LINECHART,
  '$'::Y2FORMAT, 'Monthly units sold vs average price per unit'::TITLE FROM sales GROUP BY ALL ORDER BY month;
