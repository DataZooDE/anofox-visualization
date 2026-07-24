-- @title Valuation & screens
SELECT 'How the index is valued. P/E is trimmed to below 80 for the charts — a '
       || 'handful of near-zero-earnings names run up to ~2,700 and would flatten '
       || 'everything else.' ::MARKDOWN;

-- Distribution of valuation by sector, side by side.
SELECT 'Valuation by sector' ::LABEL;
SELECT 6 ::COL;
SELECT sector ::XAXIS, pe ::BOXPLOT, 'P/E by sector (trimmed below 80)' ::TITLE
FROM co WHERE sector <> 'Other' AND pe IS NOT NULL AND pe < 80;
SELECT 6 ::COL;
SELECT sector ::XAXIS, 100 * div_yield ::BOXPLOT, 'Dividend yield by sector (%)' ::TITLE
FROM co WHERE sector <> 'Other' AND div_yield IS NOT NULL;

-- The multi-series scatter wants the full width to breathe.
SELECT 'Size vs valuation' ::LABEL;
SELECT 12 ::COL;
SELECT round(log10(market_cap), 2) ::XAXIS, pe ::SCATTER, sector ::CATEGORY,
       'log10(market cap) vs P/E, coloured by sector (P/E < 80)' ::TITLE
FROM co WHERE sector <> 'Other' AND pe IS NOT NULL AND pe < 80 AND market_cap > 0;

-- Value / income screens, side by side.
SELECT 'Screens' ::LABEL;
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
