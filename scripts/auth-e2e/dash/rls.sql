-- @title Revenue (row filtered by login)
-- @vars channel
SELECT 'Signed in as ' || coalesce(getvariable('__claim_user'), '(nobody)') || ' | tenant ' || coalesce(getvariable('__claim_tenant'), '-') || ' | channel ' || coalesce(getvariable('channel'), 'all') ::LABEL;
SELECT region::XAXIS, sum(revenue)::BARCHART, 'Revenue by region (your regions only)'::TITLE
FROM orders
WHERE (region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))
       OR list_contains(getvariable('__claim_groups'), 'admins'))
  AND (getvariable('channel') IS NULL OR channel = getvariable('channel'))
GROUP BY region ORDER BY region;
