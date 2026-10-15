-- Author branding on top of a corporate theme. The theme is inlined here so the
-- example also runs in the browser; from a file it is
--   SELECT content::THEME FROM read_text('examples/theming/acme.json');
-- The "Powered by DataZoo GmbH" credit is always shown in addition.
SELECT 'ACME Analytics'::BRAND;
SELECT 'Sales cockpit'::DASHBOARD_TITLE;
SELECT 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHZpZXdCb3g9JzAgMCA0MCA0MCc+PHJlY3Qgd2lkdGg9JzQwJyBoZWlnaHQ9JzQwJyByeD0nOScgZmlsbD0nIzBiNWZmZicvPjxwYXRoIGQ9J00xMSAyOSAyMCAxMGw5IDE5aC01bC00LTktNCA5eicgZmlsbD0nI2ZmZicvPjwvc3ZnPg=='::LOGO;
SELECT 'data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHZpZXdCb3g9JzAgMCA0MCA0MCc+PHJlY3Qgd2lkdGg9JzQwJyBoZWlnaHQ9JzQwJyByeD0nOScgZmlsbD0nIzBiNWZmZicvPjxwYXRoIGQ9J00xMSAyOSAyMCAxMGw5IDE5aC01bC00LTktNCA5eicgZmlsbD0nI2ZmZicvPjwvc3ZnPg=='::FAVICON;
SELECT '{"name":"acme","extends":"light","palette":["#0b5fff","#e11d48","#16a34a","#f59e0b","#7c3aed","#0891b2"],"colors":{"bg":"#f4f6f9","bg2":"#f4f6f9","card":"#ffffff","ink":"#1f2937","muted":"#6b7280","line":"#e5e7eb","heading":"#111827","accent":"#0b5fff","accent2":"#3b82f6","positive":"#16a34a","negative":"#dc2626","warning":"#d97706","link":"#0b5fff"},"font":{"family":"Inter, system-ui, -apple-system, Segoe UI, sans-serif","display":"Inter, system-ui, sans-serif"},"radius":10,"chrome":{"gradient":false,"background":"plain","kpi_value":"accent","table_zebra":true,"shadow":"sm"},"brand":{"title_suffix":" \u00b7 ACME"},"dark":{"colors":{"bg":"#0f1729","bg2":"#0f1729","card":"#162033","ink":"#e5e7eb","muted":"#9ca3af","line":"#26324a","heading":"#f9fafb","accent":"#5b8cff","accent2":"#7aa2ff"}}}'::THEME;

CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('Q1','North',120),('Q1','South',95),('Q1','West',60),
  ('Q2','North',140),('Q2','South',110),('Q2','West',75),
  ('Q3','North',150),('Q3','South',102),('Q3','West',91),
  ('Q4','North',171),('Q4','South',128),('Q4','West',99)
) t(quarter, region, revenue);

SELECT 'Revenue'::SECTION;
SELECT 'Key figures'::GROUP;
SELECT sum(revenue)::METRIC, 'Revenue (k)'::LABEL FROM sales;
SELECT count(DISTINCT region)::METRIC, 'Regions'::LABEL FROM sales;
SELECT round(100.0 * sum(revenue) FILTER (WHERE quarter = 'Q4') / sum(revenue), 1)::PERCENT, 'Q4 share'::LABEL FROM sales;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT quarter::XAXIS, region::CATEGORY, sum(revenue)::BARCHART_STACKED, 'Revenue by region'::TITLE FROM sales GROUP BY ALL ORDER BY ALL;
SELECT 6::COL;
SELECT quarter::XAXIS, region::CATEGORY, sum(revenue)::LINECHART, 'Trend'::TITLE FROM sales GROUP BY ALL ORDER BY ALL;

SELECT 'Detail'::SECTION;
SELECT 'Q4 detail'::TITLE, region::TABLE, revenue FROM sales WHERE quarter = 'Q4' ORDER BY revenue DESC;
SELECT '**Note:** this callout, the table and the charts all follow the ACME theme.'::MARKDOWN;
