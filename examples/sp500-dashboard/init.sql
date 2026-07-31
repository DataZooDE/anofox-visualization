-- S&P 500 companies financials — example dashboard, data layer.
--
-- Two published CSVs from the `datasets` project are joined into one tidy
-- table `co`, plus a `meta` summary row and a `sel_sector()` picker macro:
--   data/constituents-financials.csv  price + valuation ratios per ticker
--   data/constituents-gics.csv        GICS sector / sub-industry / HQ / founded
--
-- Sources:
--   https://github.com/datasets/s-and-p-500-companies-financials
--   https://github.com/datasets/s-and-p-500-companies
--
-- Run read-write once (this file); `anofox_serve_dashboards` then snapshots the
-- result and serves it read-only. Paths are relative to this example's folder,
-- so start duckdb from examples/sp500-dashboard/ (serve.sh does this for you).

CREATE OR REPLACE TABLE co AS
WITH fin AS (
  SELECT Symbol            AS symbol,
         Name              AS name,
         Price             AS price,
         "Price/Earnings"  AS pe,
         "Dividend Yield"  AS div_yield,
         "Earnings/Share"  AS eps,
         "52 Week Low"     AS wk_low,
         "52 Week High"    AS wk_high,
         "Market Cap"      AS market_cap,
         EBITDA            AS ebitda,
         "Price/Sales"     AS ps,
         "Price/Book"      AS pb
  FROM read_csv_auto('data/constituents-financials.csv', header = true)
),
gics AS (
  SELECT Symbol                                                   AS symbol,
         "GICS Sector"                                            AS sector,
         "GICS Sub-Industry"                                      AS sub_industry,
         regexp_extract("Headquarters Location", ', ([^,]+)$', 1) AS hq_state,
         try_cast(regexp_extract(Founded, '(\d{4})', 1) AS INTEGER) AS founded
  FROM read_csv_auto('data/constituents-gics.csv', header = true)
)
SELECT f.symbol,
       f.name,
       COALESCE(g.sector, 'Other')            AS sector,
       COALESCE(g.sub_industry, 'Unclassified') AS sub_industry,
       f.price, f.pe, f.div_yield, f.eps, f.wk_low, f.wk_high,
       f.market_cap, f.ebitda, f.ps, f.pb,
       g.hq_state, g.founded,
       -- price position within the 52-week range: 0 = at the low, 1 = at the high
       CASE WHEN f.wk_high > f.wk_low
            THEN (f.price - f.wk_low) / (f.wk_high - f.wk_low) END AS range_pos
FROM fin f
LEFT JOIN gics g USING (symbol);

-- Headline numbers, read by every page's banner.
CREATE OR REPLACE TABLE meta AS SELECT
  (SELECT count(*)                     FROM co)                         AS n_co,
  (SELECT count(*)                     FROM co WHERE sector <> 'Other') AS n_classified,
  (SELECT count(DISTINCT sector)       FROM co WHERE sector <> 'Other') AS n_sectors,
  (SELECT sum(market_cap)              FROM co)                         AS total_mcap,
  (SELECT median(pe)        FROM co WHERE pe IS NOT NULL)               AS med_pe,
  (SELECT median(div_yield) FROM co WHERE div_yield IS NOT NULL)        AS med_dy;

-- The active sector filter, or NULL = all sectors. Unifies two inputs:
--   1. clicking a sector bar/box sets getvariable('selected') to the sector name
--      (a click on anything else — a company — matches no sector, so is ignored);
--   2. the dropdown sets getvariable('sector') ('All sectors' = no filter).
-- A click wins over the dropdown. NULL result => panels show every sector.
CREATE OR REPLACE MACRO sel_sector() AS
  COALESCE(
    (SELECT DISTINCT sector FROM co WHERE sector = NULLIF(getvariable('selected'), '')),
    NULLIF(getvariable('sector'), 'All sectors'));
