SELECT 12::COL;
SELECT region ::XAXIS, sum(revenue) ::BARCHART, 'North leads, South close behind'::TITLE
FROM (VALUES ('North', 420), ('South', 380), ('East', 260), ('West', 190)) sales(region, revenue)
GROUP BY ALL ORDER BY 2 DESC;
