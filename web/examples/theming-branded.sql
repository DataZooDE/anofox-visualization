-- Theming: your branding. A sales cockpit for a made-up company, ACME, in
-- ACME's own indigo corporate theme (inlined as JSON so the example also
-- runs in the browser; from a file it is
--   SELECT content::THEME FROM read_text('examples/theming/acme.json');),
-- its name, title, logo and favicon. The "Powered by DataZoo GmbH" credit is
-- always shown in addition.
--
-- The data is synthetic and deterministic: two years of daily orders
-- (Oct 2024 – Sep 2026) for 4 regions × 3 channels × 5 product lines, with
-- growth, a Q4 peak, weekday rhythm and hash-based noise. Filters (period,
-- region, channels) drive every panel; the KPIs compare the period with the one
-- before it.

-- ── Brand ────────────────────────────────────────────────────────────────────
SELECT 'ACME Analytics'::BRAND;
SELECT 'Sales cockpit'::DASHBOARD_TITLE;
SELECT 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHZpZXdCb3g9JzAgMCA0MCA0MCc+PHJlY3Qgd2lkdGg9JzQwJyBoZWlnaHQ9JzQwJyByeD0nOScgZmlsbD0nIzQzMzhjYScvPjxwYXRoIGQ9J00xMSAyOSAyMCAxMGw5IDE5aC01bC00LTktNCA5eicgZmlsbD0nI2ZmZicvPjwvc3ZnPg=='::LOGO;
SELECT 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHZpZXdCb3g9JzAgMCA0MCA0MCc+PHJlY3Qgd2lkdGg9JzQwJyBoZWlnaHQ9JzQwJyByeD0nOScgZmlsbD0nIzQzMzhjYScvPjxwYXRoIGQ9J00xMSAyOSAyMCAxMGw5IDE5aC01bC00LTktNCA5eicgZmlsbD0nI2ZmZicvPjwvc3ZnPg=='::FAVICON;
SELECT '{"name":"acme","extends":"light","mode":"auto","palette":["#4338ca","#eb6834","#1baf7a","#eda100","#e87ba4","#008300","#0891b2","#e34948"],"sequential":["#e0e7ff","#3730a3"],"diverging":["#dc2626","#f1f2f6","#4338ca"],"colors":{"bg":"#f5f6fb","bg2":"#f5f6fb","card":"#ffffff","ink":"#111827","muted":"#6b7280","line":"#e4e6ef","heading":"#0b0d1a","accent":"#4338ca","accent2":"#4338ca","positive":"#15803d","negative":"#dc2626","warning":"#b45309","info":"#4338ca","link":"#3730a3","other":"#8b929e"},"chart":{"heading":"#0b0d1a","note":"#9ca3af","ink":"#111827","muted":"#6b7280","line":"#eceef4","seq_low":"#e0e7ff"},"font":{"family":"''Hanken Grotesk'',system-ui,-apple-system,''Segoe UI'',sans-serif","display":"''Hanken Grotesk'',system-ui,sans-serif"},"radius":10,"chrome":{"gradient":false,"background":"plain","kpi_value":"accent","table_zebra":false,"shadow":"sm"},"brand":{"title_suffix":" · ACME"},"dark":{"palette":["#6366f1","#d95926","#199e70","#c98500","#d55181","#008300","#0891b2","#e66767"],"colors":{"bg":"#0c0e1a","bg2":"#0c0e1a","card":"#151826","ink":"#e5e7f0","muted":"#a3a8bd","line":"#262a3d","heading":"#ffffff","accent":"#818cf8","accent2":"#818cf8","positive":"#4ade80","negative":"#f87171","warning":"#fbbf24","info":"#a5b4fc","link":"#a5b4fc","other":"#7d8296"},"chart":{"heading":"#ffffff","note":"#8b90a6","ink":"#e5e7f0","muted":"#a3a8bd","line":"#262a3d","seq_low":"#1e2240"}}}'::THEME;

-- ── Data ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE MACRO acme_u(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

CREATE OR REPLACE TABLE acme_orders AS
WITH d AS (SELECT (DATE '2024-10-01' + INTERVAL (i) DAY)::DATE AS day, i FROM range(0, 730) t(i)),
r(region, rf) AS (VALUES ('North', 1.25), ('South', 1.0), ('West', 0.85), ('East', 0.6)),
c(channel, cf, cm) AS (VALUES ('Online', 1.0, 0.0), ('Retail', 0.8, 0.04), ('Partner', 0.45, -0.06)),
p(product, base, aov, margin, trend, ret) AS (VALUES
    ('Laptops', 5200, 1150, 0.18, 0.10, 0.045), ('Monitors', 2100, 340, 0.22, 0.02, 0.035),
    ('Accessories', 1500, 48, 0.41, 0.16, 0.07), ('Software', 1800, 210, 0.68, 0.34, 0.01),
    ('Services', 1100, 520, 0.52, -0.08, 0.004))
SELECT d.day, r.region, c.channel, p.product,
       round(p.base * r.rf * c.cf
             * (1 + p.trend * d.i / 365.0)                                   -- growth per product line
             * (1 + 0.35 * exp(-power((dayofyear(d.day) - 345) / 22.0, 2)))  -- Q4 / December peak
             * CASE WHEN dayofweek(d.day) IN (0, 6) THEN 0.55 ELSE 1.08 END   -- weekends are quiet
             * CASE WHEN r.region = 'East' AND d.day >= DATE '2026-03-01' THEN 1.35 ELSE 1 END  -- new East office
             * (0.8 + 0.4 * acme_u(d.i, r.region, c.channel || p.product)), 2) AS revenue,
       p.aov, p.margin + c.cm + 0.03 * (acme_u(d.i, p.product, 'm') - 0.5) AS margin_pct,
       p.ret * (0.7 + 0.6 * acme_u(d.i, r.region, p.product || 'ret')) * CASE WHEN c.channel = 'Online' THEN 1.6 ELSE 1 END
           * CASE WHEN r.region = 'West' AND p.product = 'Monitors' THEN 2.2 ELSE 1 END AS return_rate  -- a bad monitor batch in the West
FROM d, r, c, p;

-- The analysis window: the ::PERIOD below (alias `period`) sets period_from /
-- period_to and the comparison window period_prev_from / period_prev_to (NULL
-- when the viewer picks "No comparison" — then the KPIs show no delta).
CREATE OR REPLACE MACRO acme_cur(d) AS
    d BETWEEN getvariable('period_from')::DATE AND getvariable('period_to')::DATE;
CREATE OR REPLACE MACRO acme_prev(d) AS
    d BETWEEN getvariable('period_prev_from')::DATE AND getvariable('period_prev_to')::DATE;
CREATE OR REPLACE MACRO acme_in(rg, ch) AS
    getvariable('region') IN ('All regions', rg) AND list_contains(getvariable('channel'), ch);

-- ── Filters (header, above the tabs) ─────────────────────────────────────────
SELECT 'Filters'::GROUP;
-- today= pins "today" to the last day of the data (a real dashboard leaves it out).
SELECT 'presets=12m,90d,30d,qtd,ytd,last_month,last_quarter; default=12m; grains=none; today=2026-09-30' AS period ::PERIOD, 'Period'::LABEL;
SELECT region ::DROPDOWN, 'Region'::LABEL FROM (SELECT 0 AS k, 'All regions' AS region UNION ALL SELECT DISTINCT 1, region FROM acme_orders) ORDER BY k, region;
SELECT DISTINCT channel ::MULTISELECT, 'Channels'::LABEL FROM acme_orders ORDER BY channel;
SELECT 1::ENDGROUP;

-- ── KPIs: the period vs the one before ───────────────────────────────────────
-- Each KPI compares with the previous period (::DELTA), says so
-- (::DELTA_LABEL — the period's own "vs Sep 2024 – Aug 2025"), shows the
-- change as a % and/or in its own unit (::DELTA_FORMAT), and the revenue and
-- orders carry a monthly trend line (::SPARKLINE); the margin has a goal.
CREATE OR REPLACE MACRO acme_monthly() AS TABLE
SELECT date_trunc('month', day) AS m, sum(revenue) AS rev, sum(revenue / aov) AS n
FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day) GROUP BY ALL;
SELECT 'vs. the previous period'::GROUP;
SELECT sum(revenue) FILTER (WHERE acme_cur(day))::MONEY, sum(revenue) FILTER (WHERE acme_prev(day))::DELTA, 'both'::DELTA_FORMAT,
       getvariable('period_prev_label')::DELTA_LABEL, (SELECT list(rev ORDER BY m) FROM acme_monthly())::SPARKLINE, 'Revenue'::LABEL
FROM acme_orders WHERE acme_in(region, channel);
SELECT round(sum(revenue / aov) FILTER (WHERE acme_cur(day)))::COMPACT, sum(revenue / aov) FILTER (WHERE acme_prev(day))::DELTA,
       getvariable('period_prev_label')::DELTA_LABEL, (SELECT list(n ORDER BY m) FROM acme_monthly())::SPARKLINE, 'Orders'::LABEL
FROM acme_orders WHERE acme_in(region, channel);
SELECT (sum(revenue) FILTER (WHERE acme_cur(day)) / sum(revenue / aov) FILTER (WHERE acme_cur(day)))::MONEY,
       (sum(revenue) FILTER (WHERE acme_prev(day)) / sum(revenue / aov) FILTER (WHERE acme_prev(day)))::DELTA, 'abs'::DELTA_FORMAT,
       getvariable('period_prev_label')::DELTA_LABEL, 'Average order'::LABEL
FROM acme_orders WHERE acme_in(region, channel);
SELECT (100 * sum(revenue * margin_pct) FILTER (WHERE acme_cur(day)) / sum(revenue) FILTER (WHERE acme_cur(day)))::PERCENT,
       (100 * sum(revenue * margin_pct) FILTER (WHERE acme_prev(day)) / sum(revenue) FILTER (WHERE acme_prev(day)))::DELTA, 'abs'::DELTA_FORMAT,
       getvariable('period_prev_label')::DELTA_LABEL, 40::GOAL, 'Gross margin'::LABEL
FROM acme_orders WHERE acme_in(region, channel);
SELECT (100 * sum(revenue) FILTER (WHERE acme_cur(day) AND product = 'Software') / sum(revenue) FILTER (WHERE acme_cur(day)))::PERCENT,
       (100 * sum(revenue) FILTER (WHERE acme_prev(day) AND product = 'Software') / sum(revenue) FILTER (WHERE acme_prev(day)))::DELTA,
       getvariable('period_prev_label')::DELTA_LABEL, 'Software share'::LABEL
FROM acme_orders WHERE acme_in(region, channel);
SELECT 1::ENDGROUP;

-- ── 1 · Overview ─────────────────────────────────────────────────────────────
SELECT 'Overview'::TAB;

SELECT 8::COL;
SELECT 340::HEIGHT;
-- full Monday-to-Sunday weeks only (a partial first/last week would dip)
SELECT date_trunc('week', day)::DATE ::XAXIS, 'Current year' ::CATEGORY, sum(revenue) / 1000 ::LINECHART, '$'::YFORMAT,
       'Weekly revenue ($k) — the last 12 months against the same weeks a year earlier'::TITLE
FROM acme_orders WHERE acme_in(region, channel) AND day >= DATE '2025-10-06' AND day < DATE '2026-09-28'
GROUP BY ALL
UNION ALL
SELECT (date_trunc('week', day) + INTERVAL 364 DAY)::DATE, 'Prior year', sum(revenue) / 1000, '$', 'Weekly revenue ($k) — the last 12 months against the same weeks a year earlier'
FROM acme_orders WHERE acme_in(region, channel) AND day >= DATE '2024-10-07' AND day < DATE '2025-09-29'
GROUP BY ALL ORDER BY 1, 2;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT channel ::CATEGORY, sum(revenue) ::DONUTCHART, 'Channel mix'::TITLE
FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day) GROUP BY ALL ORDER BY 2 DESC;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT product ::XAXIS, sum(revenue) / 1e6 ::BARCHART, 1 ::FLIP, '$'::YFORMAT, 'Revenue by product line ($M) — click a bar to filter the table'::TITLE
FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day) GROUP BY ALL ORDER BY 2;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT product ::XAXIS, round(100 * sum(revenue * margin_pct) / sum(revenue), 1) ::BARCHART, 30 ::REFLINE, 'percent'::YFORMAT, 1 ::FLIP,
       'Gross margin by product line — the dashed line is the 30 % target'::TITLE
FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day) GROUP BY ALL ORDER BY 2;

SELECT 12::COL;
SELECT 'Product lines by region'::TITLE,
       product AS "Product line" ::TABLE, region AS "Region",
       list(rev ORDER BY wk) FILTER (WHERE nd = 7) AS "Weekly revenue" ::SPARKLINE,
       sum(rev) AS "Revenue" ::MONEY,
       round(100 * (sum(rev) / NULLIF(any_value(prev), 0) - 1), 1) AS "vs. previous %" ::TREND,
       round(100 * sum(rev * m) / sum(rev), 1) AS "Margin" ::PERCENT,
       round(100 * sum(rev * rr) / sum(rev), 1) AS "Returns %" ::COLORSCALE,
       CASE WHEN sum(rev) / NULLIF(any_value(prev), 0) >= 1.05 THEN 'growing'
            WHEN sum(rev) / NULLIF(any_value(prev), 0) >= 0.97 THEN 'flat' ELSE 'shrinking' END AS "Status" ::BADGE
FROM (SELECT product, region, date_trunc('week', day) AS wk, sum(revenue) AS rev, sum(revenue * margin_pct) / sum(revenue) AS m,
             sum(revenue * return_rate) / sum(revenue) AS rr, count(DISTINCT day) AS nd
      FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day)
        AND (coalesce(getvariable('selected'), '') IN ('', product)  -- a click on a product bar filters; other clicks don't
             OR getvariable('selected') NOT IN ('Laptops', 'Monitors', 'Accessories', 'Software', 'Services'))
      GROUP BY ALL) w
JOIN (SELECT product, region, sum(revenue) AS prev FROM acme_orders WHERE acme_in(region, channel) AND acme_prev(day) GROUP BY ALL)
  USING (product, region)
GROUP BY product, region ORDER BY "Revenue" DESC;

-- ── 2 · Regions ──────────────────────────────────────────────────────────────
SELECT 'Regions'::TAB;

SELECT 7::COL;
SELECT 360::HEIGHT;
SELECT strftime(date_trunc('month', day), '%Y-%m') ::XAXIS, region ::YAXIS,
       round(100 * (sum(revenue) / any_value(ly) - 1)) ::HEATMAP, 'percent'::YFORMAT,
       'Growth vs. the same month last year (%) — East jumps when its new office opens in March'::TITLE
FROM acme_orders o
JOIN (SELECT region AS r2, date_trunc('month', day + INTERVAL 1 YEAR) AS m2, sum(revenue) AS ly
      FROM acme_orders WHERE list_contains(getvariable('channel'), channel) GROUP BY ALL)
  ON r2 = region AND m2 = date_trunc('month', day)
WHERE list_contains(getvariable('channel'), channel)
GROUP BY ALL ORDER BY 1, 2;

SELECT 5::COL;
SELECT 360::HEIGHT;
SELECT region ::XAXIS, channel ::CATEGORY, sum(revenue) / 1e6 ::BARCHART_STACKED, '$'::YFORMAT, 'Revenue by region and channel ($M)'::TITLE
FROM acme_orders WHERE acme_in(region, channel) AND acme_cur(day) GROUP BY ALL ORDER BY 1, 2;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT date_trunc('month', day)::DATE ::XAXIS, region ::CATEGORY, sum(revenue) / 1e6 ::AREACHART_STACKED, '$'::YFORMAT,
       'Monthly revenue by region ($M), two years'::TITLE
FROM acme_orders WHERE list_contains(getvariable('channel'), channel) GROUP BY ALL ORDER BY 1, 2;

-- ── 3 · How this is branded ──────────────────────────────────────────────────
SELECT 'Branding'::TAB;

SELECT 7::COL;
SELECT '**One theme styles everything.** The `::THEME` JSON sets the palette (eight colourblind-checked series colours, with their own steps for dark mode), a cool indigo-tinted page, white cards with a soft shadow, indigo KPI numbers and a 10 px radius — ACME''s identity on top of the default graphite look. `::BRAND`, `::DASHBOARD_TITLE`, `::LOGO` and `::FAVICON` add the company''s identity to the header and the browser tab. Toggle 🌙 to see the dark variant: the charts are redrawn with the dark palette, not just recoloured. From a file, the theme is one line: `SELECT content::THEME FROM read_text(''examples/theming/acme.json'')`.'::MARKDOWN, 'How this dashboard is branded'::TITLE;

SELECT 5::COL;
SELECT 'Theme tokens'::TITLE, token AS "Token" ::TABLE, light AS "Light", dark AS "Dark"
FROM (VALUES (1, 'page', '#f5f6fb', '#0c0e1a'), (2, 'card', '#ffffff', '#151826'), (3, 'ink', '#111827', '#e5e7f0'),
             (4, 'accent', '#4338ca', '#818cf8'), (5, 'hairline', '#e4e6ef', '#262a3d'), (6, 'radius', '10 px', '10 px'),
             (7, 'shadow', 'small', 'small'), (8, 'KPI numbers', 'accent', 'accent')) t(k, token, light, dark)
ORDER BY k;
