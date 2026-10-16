-- A ::STORY on a DATE x axis (scripts/a11y-e2e.mjs --sql): each step must
-- light the bars of its day, not dim them all.
SELECT 'Story on a date axis'::DASHBOARD_TITLE;
SELECT 12::COL;
SELECT d ::XAXIS, c ::CATEGORY, v ::BARCHART,
       CASE d WHEN DATE '2026-09-03' THEN 'Sep 3: Store peaks'
              WHEN DATE '2026-09-06' THEN 'Sep 6: Online catches up' END ::STORY,
       'Daily orders per channel' ::TITLE
FROM (SELECT (DATE '2026-09-01' + INTERVAL (i) DAY)::DATE AS d, c, 20 + i * k + (i % 3) * 4 AS v
      FROM range(0, 8) t(i), (VALUES ('Online', 2), ('Store', 1)) u(c, k))
ORDER BY d, c;
