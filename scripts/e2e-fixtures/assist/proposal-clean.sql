-- #244 fixture: a strict-clean proposal (dashboard --check --strict passes)
-- over the sales-setup.sql table only, TEMP objects only — used by
-- tests/web/assist_preview_test.mjs and scripts/assist-e2e.mjs (M0/M3…).
-- @title How do regions and channels sell?
SELECT region AS region ::DROPDOWN FROM sales GROUP BY 1 ORDER BY 1;

SELECT 'Overview'::TAB;
SELECT ROUND(SUM(units*price),0) ::MONEY, 'Revenue' ::LABEL
FROM sales WHERE region = getvariable('region');
SELECT month ::XAXIS, ROUND(SUM(units*price),0) ::LINECHART, '€'::YFORMAT,
       'Monthly revenue'::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY month ORDER BY month;

SELECT 'Drill'::TAB;
SELECT 'region > channel'::DRILL;
SELECT CASE getvariable('drill_level') WHEN 'region' THEN region ELSE channel END AS place ::XAXIS,
       ROUND(SUM(units*price),0)::BARCHART, ('Revenue by ' || getvariable('drill_level'))::TITLE
FROM sales
WHERE getvariable('region') IN ('', region)
GROUP BY 1 ORDER BY 2 DESC;

SELECT 'Detail'::TAB;
SELECT month AS "Month" ::PAGED, channel AS "Channel", price AS "Price", units AS "Units"
FROM sales WHERE region = getvariable('region')
ORDER BY month, channel;

SELECT 'What this shows'::TAB;
SELECT 'This dashboard shows how regions and channels sell: a filter picks the region, the overview tab shows the revenue trend, the drill tab compares region and channel, and the detail tab lists every row.'::MARKDOWN,
       'What this shows'::TITLE;
