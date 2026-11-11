-- @from smoke-claude-sonnet-5-20261110-231930 big-long-tail: ::PARETO over a top 20 of 1,500 products — the line ends at 100 % (#1100)
-- @expect chart/pareto-limit its cumulative line is the share of the rows shown
CREATE TABLE sales AS SELECT 'P-' || i AS product, 1000.0 / i AS total_qty FROM range(1, 1501) r(i);
SELECT product::XAXIS, total_qty::PARETO AS "Units sold", 'Sales are highly concentrated'::TITLE
FROM sales ORDER BY total_qty DESC LIMIT 20;
