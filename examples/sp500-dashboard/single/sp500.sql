-- S&P 500 financials — an interactive, cross-filtered dashboard.
-- A single sector filter drives every panel: pick it from the dropdown, or click
-- a sector bar / box plot (linking). The comparative sector charts are the click
-- SOURCES (they never self-filter, so they stay clickable); everything else is a
-- TARGET that filters to sel_sector() (NULL = all sectors). Data + the
-- sel_sector() macro come from init.sql.

-- Global sector filter (applies to every tab). A bare input renders as a slim
-- control bar — no banner card. 'All sectors' = no filter; clicking a sector
-- bar/box filters too. The KPI strip below carries the headline numbers, so no
-- separate summary banner is needed.
SELECT sector AS sector ::DROPDOWN, 'Sector' ::LABEL
FROM (SELECT 'All sectors' AS sector, 0 AS o
      UNION ALL SELECT DISTINCT sector, 1 FROM co WHERE sector <> 'Other')
ORDER BY o, sector;

-- ============================ Market overview ============================
SELECT 'Market overview' ::TAB;

-- KPI strip — reflects the active filter.
SELECT 'Market' ::GROUP;
SELECT count(*) ::COMPACT, 'Companies' ::LABEL
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector());
SELECT count(DISTINCT sub_industry) ::METRIC, 'Sub-industries' ::LABEL
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector());
SELECT round(sum(market_cap)) ::COMPACT, 'Market cap ($)' ::LABEL
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector());
SELECT round(median(pe), 1) ::METRIC, 'Median P/E' ::LABEL
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector()) AND pe IS NOT NULL;
SELECT round(100 * median(div_yield), 2) ::METRIC, 'Median div yield (%)' ::LABEL
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector()) AND div_yield IS NOT NULL;
SELECT 1 ::ENDGROUP;

-- Sector composition — the CLICK SOURCES (always all sectors, never self-filter).
SELECT 290 ::HEIGHT;
SELECT 6 ::COL;
SELECT sector ::XAXIS, count(*) ::BARCHART, TRUE ::FLIP, 'Companies by sector' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY count(*);
SELECT 290 ::HEIGHT;
SELECT 6 ::COL;
SELECT sector ::XAXIS, round(sum(market_cap) / 1e9) ::BARCHART, TRUE ::FLIP,
       'Market cap by sector ($B)' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY sum(market_cap);

-- Valuation spread — TARGETS (filter to the active sector).
SELECT 250 ::HEIGHT;
SELECT 6 ::COL;
SELECT pe ::HISTOGRAM, 'P/E distribution (below the 95th percentile)' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector()) AND pe IS NOT NULL
  AND pe < (SELECT quantile_cont(pe, 0.95) FROM co WHERE pe IS NOT NULL);
SELECT 250 ::HEIGHT;
SELECT 6 ::COL;
SELECT 100 * div_yield ::HISTOGRAM, 'Dividend yield distribution (%)' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector()) AND div_yield IS NOT NULL;

SELECT 12 ::COL;
SELECT 'Largest companies (top 20 by market cap)' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company", sector AS "Sector",
       round(price, 2) AS "Price", round(pe, 1) AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %", round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
ORDER BY market_cap DESC NULLS LAST LIMIT 20;

-- ============================ Companies ============================
SELECT 'Companies' ::TAB;

SELECT 320 ::HEIGHT;
SELECT 6 ::COL;
SELECT name ::XAXIS, round(market_cap / 1e9, 1) ::BARCHART, TRUE ::FLIP,
       'Largest companies by market cap ($B) — top 15' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
ORDER BY market_cap DESC NULLS LAST LIMIT 15;
SELECT 320 ::HEIGHT;
SELECT 6 ::COL;
SELECT pe ::XAXIS, 100 * div_yield ::SCATTER, 'P/E vs dividend yield (%)' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
  AND pe IS NOT NULL AND div_yield IS NOT NULL AND pe < 100;

SELECT 12 ::COL;
SELECT 'Companies (top 100 by market cap; all of a filtered sector)' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company", sector AS "Sector",
       sub_industry AS "Sub-industry", round(price, 2) AS "Price", round(pe, 1) AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %", round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
ORDER BY market_cap DESC NULLS LAST LIMIT 100;

-- ============================ Valuation & screens ============================
SELECT 'Valuation & screens' ::TAB;

SELECT 12 ::COL;
SELECT CASE WHEN sel_sector() IS NULL
         THEN 'Valuation by sector. P/E is trimmed to < 80 (a few near-zero-earnings names run to ~2,700). Pick or click a sector to drill into its sub-industries.'
         ELSE '**' || sel_sector() || '** — valuation by sub-industry. P/E trimmed to < 80.'
       END ::MARKDOWN;

-- Box plots — by sector when unfiltered, drilling to sub-industry when filtered.
SELECT 340 ::HEIGHT;
SELECT 6 ::COL;
SELECT (CASE WHEN sel_sector() IS NULL THEN sector ELSE sub_industry END) ::XAXIS,
       pe ::BOXPLOT,
       (CASE WHEN sel_sector() IS NULL THEN 'P/E by sector' ELSE 'P/E by sub-industry' END) ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
  AND sector <> 'Other' AND pe IS NOT NULL AND pe < 80;
SELECT 340 ::HEIGHT;
SELECT 6 ::COL;
SELECT (CASE WHEN sel_sector() IS NULL THEN sector ELSE sub_industry END) ::XAXIS,
       100 * div_yield ::BOXPLOT,
       (CASE WHEN sel_sector() IS NULL THEN 'Dividend yield by sector (%)' ELSE 'Dividend yield by sub-industry (%)' END) ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
  AND sector <> 'Other' AND div_yield IS NOT NULL;

SELECT 320 ::HEIGHT;
SELECT 12 ::COL;
SELECT round(log10(market_cap), 2) ::XAXIS, pe ::SCATTER,
       (CASE WHEN sel_sector() IS NULL THEN sector ELSE sub_industry END) ::CATEGORY,
       'log10(market cap) vs P/E (P/E < 80)' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
  AND sector <> 'Other' AND pe IS NOT NULL AND pe < 80 AND market_cap > 0;

-- Value / income screens, side by side. A ::TITLE column titles each table
-- without a full-width heading (which would force them to stack).
SELECT 6 ::COL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company",
       round(100 * div_yield, 2) AS "Div yield %", round(pe, 1) AS "P/E",
       round(market_cap / 1e9, 1) AS "Market cap ($B)", 'Highest dividend yield' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector()) AND div_yield IS NOT NULL
ORDER BY div_yield DESC LIMIT 12;
SELECT 6 ::COL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company",
       round(pe, 1) AS "P/E", round(100 * div_yield, 2) AS "Div yield %",
       round(market_cap / 1e9, 1) AS "Market cap ($B)", 'Lowest P/E (positive earnings)' ::TITLE
FROM co WHERE (sel_sector() IS NULL OR sector = sel_sector())
  AND pe IS NOT NULL AND pe > 0 AND eps > 0 ORDER BY pe ASC LIMIT 12;
