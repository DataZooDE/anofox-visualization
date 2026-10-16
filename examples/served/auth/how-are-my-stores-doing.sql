-- How are my stores doing? — each region manager sees only their own stores.
-- Served only with `serve.sh start --auth`: the login proxy tells the server
-- who is viewing, and the server sets __claim_user / __claim_groups on every
-- request. A viewer can't set those variables (PR #78), so the filter below
-- holds; members of the `admins` group see every store.

SELECT 'How are my stores doing?'::DASHBOARD_TITLE;
SELECT 'en-GB'::LOCALE;
SELECT max(loaded_at)::ASOF, 'stale=8d' FROM sales;

CREATE OR REPLACE TEMP VIEW mine AS
SELECT st.store, st.region, st.city FROM stores st
WHERE coalesce(list_contains(getvariable('__claim_groups'), 'admins'), false)
   OR st.region IN (SELECT region FROM region_managers WHERE user_name = getvariable('__claim_user'));
CREATE OR REPLACE TEMP VIEW wk AS
SELECT date_trunc('week', s.day)::DATE AS week, m.region, s.store, sum(s.revenue) AS revenue, sum(s.units) AS units
FROM sales s JOIN mine m USING (store) GROUP BY ALL;

SELECT 12::COL;
SELECT 'Signed in as **' || coalesce(getvariable('__claim_user'), 'nobody') || '** · '
       || coalesce((SELECT string_agg(DISTINCT region, ', ' ORDER BY region) FROM mine), 'no stores')::MARKDOWN;

SELECT 'Last week'::GROUP;
SELECT sum(revenue) FILTER (WHERE week = (SELECT max(week) FROM wk))::MONEY,
       sum(revenue) FILTER (WHERE week = (SELECT max(week) - 7 FROM wk))::DELTA,
       'vs the week before'::DELTA_LABEL,
       'Revenue, my stores'::LABEL
FROM wk;
SELECT count(DISTINCT store)::METRIC, 'Stores'::LABEL FROM mine;
SELECT 1::ENDGROUP;

SELECT 12::COL;
SELECT week::XAXIS, store::CATEGORY, revenue::LINECHART, '£'::YFORMAT, 'Weekly revenue per store'::TITLE
FROM wk ORDER BY store, week;

SELECT 12::COL;
SELECT region AS "Region", store AS "Store" ::TABLE,
       last AS "Last week" ::MONEY, prev AS "Week before" ::MONEY,
       round(100 * (last / prev - 1), 1) AS "Change %" ::TREND,
       'My stores'::TITLE
FROM (SELECT region, store,
             sum(revenue) FILTER (WHERE week = (SELECT max(week) FROM wk)) AS last,
             sum(revenue) FILTER (WHERE week = (SELECT max(week) - 7 FROM wk)) AS prev
      FROM wk GROUP BY region, store)
ORDER BY region, store;
