-- #244 fixture: a hostile proposal — a reader of a remote path, a markdown
-- image/link to the recorder, INSTALL and ATTACH, plus one valid panel.
-- scripts/assist-e2e.mjs substitutes ${REC} with the recorder's port before
-- use; the recorder must stay empty throughout the preview (C-1/R9).
SELECT * FROM read_csv('http://127.0.0.1:${REC}/preview.csv') ::TABLE;
SELECT '![x](http://127.0.0.1:${REC}/md.png) [go](http://127.0.0.1:${REC}/link)'::MARKDOWN;
INSTALL spatial;
ATTACH 'x.duckdb' AS x;
SELECT region ::XAXIS, ROUND(SUM(units*price),0) ::BARCHART, 'Revenue by region'::TITLE
FROM sales GROUP BY 1 ORDER BY 2 DESC;
