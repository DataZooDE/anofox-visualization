-- @title Market overview
SELECT 'S&P 500 constituents — **' || format('{:,}', n_co) || '** companies across **'
       || n_sectors || '** GICS sectors, **$' || round(total_mcap / 1e12, 1)
       || ' T** total market capitalisation. Median P/E ' || round(med_pe, 1)
       || ', median dividend yield ' || round(100 * med_dy, 2)
       || '%. Data: the datasets/ S&P 500 financials snapshot.' ::MARKDOWN
FROM meta;

-- KPI strip (one compact band, not five stacked cards).
SELECT 'Market' ::GROUP;
SELECT count(*) ::COMPACT, 'Companies' ::LABEL FROM co;
SELECT n_sectors ::METRIC, 'GICS sectors' ::LABEL FROM meta;
SELECT round(sum(market_cap)) ::COMPACT, 'Total market cap ($)' ::LABEL FROM co;
SELECT round(median(pe), 1) ::METRIC, 'Median P/E' ::LABEL FROM co WHERE pe IS NOT NULL;
SELECT round(100 * median(div_yield), 2) ::METRIC, 'Median div yield (%)' ::LABEL
FROM co WHERE div_yield IS NOT NULL;
SELECT 1 ::ENDGROUP;

-- Sector charts, side by side. Horizontal bars (::FLIP) keep the long GICS
-- names legible at half width.
SELECT 'Sectors' ::LABEL;
SELECT 6 ::COL;
SELECT sector ::XAXIS, count(*) ::BARCHART, TRUE ::FLIP, 'Companies by sector' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY count(*);
SELECT 6 ::COL;
SELECT sector ::XAXIS, round(sum(market_cap) / 1e9) ::BARCHART, TRUE ::FLIP,
       'Total market cap by sector ($B)' ::TITLE
FROM co WHERE sector <> 'Other' GROUP BY sector ORDER BY sum(market_cap);

-- Valuation distributions, side by side.
SELECT 'Valuation distributions' ::LABEL;
SELECT 6 ::COL;
SELECT pe ::HISTOGRAM, 'P/E distribution (below the 95th percentile)' ::TITLE
FROM co
WHERE pe IS NOT NULL
  AND pe < (SELECT quantile_cont(pe, 0.95) FROM co WHERE pe IS NOT NULL);
SELECT 6 ::COL;
SELECT 100 * div_yield ::HISTOGRAM, 'Dividend yield distribution (%)' ::TITLE
FROM co WHERE div_yield IS NOT NULL;

SELECT 12 ::COL;
SELECT 'Top 20 companies by market capitalisation' ::LABEL;
SELECT symbol AS "Ticker" ::TABLE,
       name   AS "Company",
       sector AS "Sector",
       round(price, 2)          AS "Price",
       round(pe, 1)             AS "P/E",
       round(100 * div_yield, 2) AS "Div yield %",
       round(market_cap / 1e9, 1) AS "Market cap ($B)"
FROM co ORDER BY market_cap DESC NULLS LAST LIMIT 20;
