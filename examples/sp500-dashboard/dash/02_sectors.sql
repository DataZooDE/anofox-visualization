-- @title Sector explorer

-- Sector selector — drives every panel on this page.
SELECT sector AS sector ::DROPDOWN, 'Sector' ::LABEL
FROM (SELECT DISTINCT sector FROM co WHERE sector <> 'Other') ORDER BY sector;

SELECT 'Showing **' || sel_sector() || '** — '
       || (SELECT count(*) FROM co WHERE sector = sel_sector()) || ' companies, $'
       || (SELECT round(sum(market_cap) / 1e9, 1) FROM co WHERE sector = sel_sector())
       || ' B total market cap. Pick another sector from the dropdown above.' ::MARKDOWN;

-- KPI strip for the selected sector.
SELECT 'Sector summary' ::GROUP;
SELECT count(*) ::COMPACT, 'Companies' ::LABEL FROM co WHERE sector = sel_sector();
SELECT round(sum(market_cap)) ::COMPACT, 'Total market cap ($)' ::LABEL
FROM co WHERE sector = sel_sector();
SELECT round(median(pe), 1) ::METRIC, 'Median P/E' ::LABEL
FROM co WHERE sector = sel_sector() AND pe IS NOT NULL;
SELECT round(100 * median(div_yield), 2) ::METRIC, 'Median div yield (%)' ::LABEL
FROM co WHERE sector = sel_sector() AND div_yield IS NOT NULL;
SELECT 1 ::ENDGROUP;

-- Top names + valuation scatter, side by side.
SELECT 6 ::COL;
SELECT name ::XAXIS, round(market_cap / 1e9, 1) ::BARCHART, TRUE ::FLIP,
       'Largest companies by market cap ($B) — top 15' ::TITLE
FROM co WHERE sector = sel_sector() ORDER BY market_cap DESC NULLS LAST LIMIT 15;
SELECT 6 ::COL;
SELECT pe ::XAXIS, 100 * div_yield ::SCATTER, 'P/E vs dividend yield (%)' ::TITLE
FROM co
WHERE sector = sel_sector() AND pe IS NOT NULL AND div_yield IS NOT NULL AND pe < 100;

SELECT 12 ::COL;
SELECT 'All companies in the selected sector' ::LABEL;
SELECT symbol       AS "Ticker" ::TABLE,
       name         AS "Company",
       sub_industry AS "Sub-industry",
       round(price, 2)          AS "Price",
       round(pe, 1)             AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %",
       round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE sector = sel_sector() ORDER BY market_cap DESC NULLS LAST;
