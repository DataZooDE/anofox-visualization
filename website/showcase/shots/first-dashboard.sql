-- @title How are our channels doing?
CREATE TABLE sales AS SELECT * FROM (VALUES
  ('W1','app',30), ('W1','web',22), ('W2','app',35), ('W2','web',25), ('W3','app',41), ('W3','web',24)
) t(week, channel, revenue);

SELECT channel AS channel ::DROPDOWN FROM sales GROUP BY 1 ORDER BY 1;

SELECT sum(revenue)::MONEY, 'Revenue'::LABEL FROM sales WHERE channel = getvariable('channel');

SELECT week::XAXIS, sum(revenue)::LINECHART, 'Revenue by week'::TITLE
FROM sales WHERE channel = getvariable('channel') GROUP BY ALL ORDER BY week;
