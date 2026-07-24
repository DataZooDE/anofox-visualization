-- S&P 500 financials — a single-file dashboard authored via the build-dashboard
-- skill + design contract (docs/dashboard-design.md), validated with
-- `dashboard --check` until clean. Data layer (tables `co`, `meta`, macro
-- `sel_sector()`) comes from init.sql; three ::TAB sections follow the inverted
-- pyramid: summary → breakdowns → detail.

SELECT 'S&P 500 constituents — **' || format('{:,}', n_co) || '** companies across **'
       || n_sectors || '** GICS sectors, **$' || round(total_mcap / 1e12, 1)
       || ' T** market cap. Median P/E ' || round(med_pe, 1)
       || ', median dividend yield ' || round(100 * med_dy, 2)
       || '%. Source: the datasets/ S&P 500 financials snapshot.' ::MARKDOWN
FROM meta;

-- ============================ Market overview ============================
SELECT 'Market overview' ::TAB;

SELECT 'Market' ::GROUP;
SELECT count(*) ::COMPACT, 'Companies' ::LABEL FROM co;
SELECT n_sectors ::METRIC, 'GICS sectors' ::LABEL FROM meta;
SELECT round(sum(market_cap)) ::COMPACT, 'Total market cap ($)' ::LABEL FROM co;
SELECT round(median(pe), 1) ::METRIC, 'Median P/E' ::LABEL FROM co WHERE pe IS NOT NULL;
SELECT round(100 * median(div_yield), 2) ::METRIC, 'Median div yield (%)' ::LABEL
FROM co WHERE div_yield IS NOT NULL;
SELECT 1 ::ENDGROUP;

-- Sector composition, side by side; horizontal bars sorted by value.
SELECT 6 ::COL;
SELECT sector ::XAXIS, count(*) ::BARCHART, TRUE ::FLIP, 'Companies by sector' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY count(*);
SELECT 6 ::COL;
SELECT sector ::XAXIS, round(sum(market_cap) / 1e9) ::BARCHART, TRUE ::FLIP,
       'Market cap by sector ($B)' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY sum(market_cap);

-- Valuation spread across the index.
SELECT 6 ::COL;
SELECT pe ::HISTOGRAM, 'P/E distribution (below the 95th percentile)' ::TITLE
FROM co WHERE pe IS NOT NULL
  AND pe < (SELECT quantile_cont(pe, 0.95) FROM co WHERE pe IS NOT NULL);
SELECT 6 ::COL;
SELECT 100 * div_yield ::HISTOGRAM, 'Dividend yield distribution (%)' ::TITLE
FROM co WHERE div_yield IS NOT NULL;

SELECT 12 ::COL;
SELECT 'Top 20 companies by market capitalisation' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company", sector AS "Sector",
       round(price, 2) AS "Price", round(pe, 1) AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %", round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co ORDER BY market_cap DESC NULLS LAST LIMIT 20;

-- ============================ Sector explorer ============================
SELECT 'Sector explorer' ::TAB;

SELECT 'Filter' ::GROUP;
SELECT sector AS sector ::DROPDOWN, 'Sector' ::LABEL
FROM (SELECT DISTINCT sector FROM co WHERE sector <> 'Other') ORDER BY sector;
SELECT 1 ::ENDGROUP;

SELECT 'Selected sector' ::GROUP;
SELECT count(*) ::COMPACT, 'Companies' ::LABEL FROM co WHERE sector = sel_sector();
SELECT round(sum(market_cap)) ::COMPACT, 'Total market cap ($)' ::LABEL
FROM co WHERE sector = sel_sector();
SELECT round(median(pe), 1) ::METRIC, 'Median P/E' ::LABEL
FROM co WHERE sector = sel_sector() AND pe IS NOT NULL;
SELECT round(100 * median(div_yield), 2) ::METRIC, 'Median div yield (%)' ::LABEL
FROM co WHERE sector = sel_sector() AND div_yield IS NOT NULL;
SELECT 1 ::ENDGROUP;

SELECT 6 ::COL;
SELECT name ::XAXIS, round(market_cap / 1e9, 1) ::BARCHART, TRUE ::FLIP,
       'Largest companies by market cap ($B) — top 15' ::TITLE
FROM co WHERE sector = sel_sector() ORDER BY market_cap DESC NULLS LAST LIMIT 15;
SELECT 6 ::COL;
SELECT pe ::XAXIS, 100 * div_yield ::SCATTER, 'P/E vs dividend yield (%)' ::TITLE
FROM co WHERE sector = sel_sector() AND pe IS NOT NULL AND div_yield IS NOT NULL AND pe < 100;

SELECT 12 ::COL;
SELECT 'All companies in the selected sector' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company", sub_industry AS "Sub-industry",
       round(price, 2) AS "Price", round(pe, 1) AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %", round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE sector = sel_sector() ORDER BY market_cap DESC NULLS LAST;

-- ============================ Valuation & screens ============================
SELECT 'Valuation & screens' ::TAB;

SELECT 12 ::COL;
SELECT 'P/E is trimmed to below 80 for the charts — a handful of near-zero-earnings '
       || 'names run up to ~2,700 and would flatten everything else.' ::MARKDOWN;

SELECT 6 ::COL;
SELECT sector ::XAXIS, pe ::BOXPLOT, 'P/E by sector (trimmed below 80)' ::TITLE
FROM co WHERE sector <> 'Other' AND pe IS NOT NULL AND pe < 80;
SELECT 6 ::COL;
SELECT sector ::XAXIS, 100 * div_yield ::BOXPLOT, 'Dividend yield by sector (%)' ::TITLE
FROM co WHERE sector <> 'Other' AND div_yield IS NOT NULL;

SELECT 12 ::COL;
SELECT round(log10(market_cap), 2) ::XAXIS, pe ::SCATTER, sector ::CATEGORY,
       'log10(market cap) vs P/E, coloured by sector (P/E < 80)' ::TITLE
FROM co WHERE sector <> 'Other' AND pe IS NOT NULL AND pe < 80 AND market_cap > 0;

SELECT 6 ::COL;
SELECT 'Highest dividend yield' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company",
       round(100 * div_yield, 2) AS "Div yield %", round(pe, 1) AS "P/E",
       round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE div_yield IS NOT NULL ORDER BY div_yield DESC LIMIT 15;
SELECT 6 ::COL;
SELECT 'Lowest P/E (positive earnings only)' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE, name AS "Company",
       round(pe, 1) AS "P/E", round(100 * div_yield, 2) AS "Div yield %",
       round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co WHERE pe IS NOT NULL AND pe > 0 AND eps > 0 ORDER BY pe ASC LIMIT 15;
