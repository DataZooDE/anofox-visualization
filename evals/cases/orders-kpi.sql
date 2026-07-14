-- request: "Order KPIs and revenue by channel."
CREATE TABLE orders AS
SELECT (['web','app','store'])[1+(i%3)] AS channel, (20 + (i*7%180))::INTEGER AS amount
FROM range(0,60) t(i);
SELECT SUM(amount)::MONEY, 'Revenue'::LABEL FROM orders;
SELECT COUNT(*)::COMPACT, 'Orders'::LABEL FROM orders;
SELECT channel::XAXIS, SUM(amount)::BARCHART, '€'::YFORMAT, 'Revenue by channel'::TITLE FROM orders GROUP BY channel ORDER BY 2 DESC;
SELECT channel AS "Channel" ::TABLE, COUNT(*) AS "Orders", SUM(amount) AS "Revenue" FROM orders GROUP BY channel ORDER BY "Revenue" DESC;
