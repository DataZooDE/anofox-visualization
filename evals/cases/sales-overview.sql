-- request: "Sales overview: revenue KPIs, a monthly revenue trend, a per-channel
-- breakdown, and a detail table — filterable by region."
CREATE TABLE sales AS
SELECT (DATE '2023-01-01' + to_months(m::INTEGER)) AS month, r.region, c.channel, c.price,
       GREATEST(0,(400 + 15*m + 50*sin(2*pi()*(m%12)/12.0)
                   + ((hash(r.region||c.channel||m::VARCHAR)%80)::INTEGER-40)))::INTEGER AS units
FROM (VALUES ('EU'),('US'),('APAC')) r(region),
     (VALUES ('web',39.0),('app',29.0),('store',49.0)) c(channel,price), range(0,36) t(m);
SELECT 'Filter'::GROUP;
SELECT region AS region ::DROPDOWN FROM sales GROUP BY region ORDER BY region;
SELECT 1::ENDGROUP;
SELECT ROUND(SUM(units*price),0)::MONEY, 'Revenue'::LABEL FROM sales WHERE region=getvariable('region');
SELECT SUM(units)::COMPACT, 'Units'::LABEL FROM sales WHERE region=getvariable('region');
SELECT 12::SPAN;
SELECT month::XAXIS, channel::CATEGORY, ROUND(SUM(units*price),0)::LINECHART, '€'::YFORMAT, 'Monthly revenue'::TITLE
FROM sales WHERE region=getvariable('region') GROUP BY month,channel ORDER BY month,channel;
SELECT channel::XAXIS, ROUND(SUM(units*price),0)::BARCHART, '€'::YFORMAT, 'Revenue by channel'::TITLE
FROM sales WHERE region=getvariable('region') GROUP BY channel ORDER BY 2 DESC;
SELECT channel AS "Channel" ::TABLE, SUM(units) AS "Units", ROUND(SUM(units*price),0) AS "Revenue"
FROM sales WHERE region=getvariable('region') GROUP BY channel ORDER BY "Revenue" DESC;
