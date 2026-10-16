-- Synthetic inputs of the "incoming feed" use case (deterministic: hash-based
-- noise, no files). A made-up wholesaler, Nordwind, sends a home-and-garden
-- shop its full price and stock list every night; the shop loads it into the
-- product catalogue at 07:00. 60 deliveries, 17 Aug – 15 Oct 2026, ~1,200
-- SKUs each, stacked into one staging table the way
-- read_csv([...], union_by_name = true) stacks the nightly files: a column
-- missing from a file loads as NULL for that delivery.
CREATE OR REPLACE MACRO fm_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

-- The supplier's catalogue: 1,230 SKUs in 8 categories, with a valid EAN-13
-- (the check digit is computed), a base price and a listing window (a few
-- SKUs are listed or delisted during the 60 days).
CREATE OR REPLACE TABLE fm_catalogue AS
WITH c(cat, k, base) AS (VALUES
    ('Kitchen', 1, 24.0), ('Audio', 2, 79.0), ('Lighting', 3, 32.0), ('Garden', 4, 41.0),
    ('Tools', 5, 55.0), ('Bathroom', 6, 29.0), ('Office', 7, 14.0), ('Toys', 8, 19.0)),
s AS (
    SELECT i, c.cat, c.base,
           '40' || lpad((1234567 + i * 7919 % 9000000)::VARCHAR, 10, '0') AS ean12
    FROM range(1, 1231) t(i)
    JOIN c ON c.k = 1 + floor(8 * fm_unif('sku', i, 'cat'))::INTEGER
)
SELECT i AS sku_no, 'NW-' || lpad(i::VARCHAR, 5, '0') AS sku,
       ean12 || ((10 - (list_sum([(ascii(ean12[p]) - 48) * CASE WHEN p % 2 = 0 THEN 3 ELSE 1 END
                                  FOR p IN range(1, 13)]) % 10)) % 10)::VARCHAR AS ean,
       cat AS category,
       cat || ' item ' || i AS product_name,
       round(base * exp(0.6 * (fm_unif('sku', i, 'p') - 0.5) + 0.4 * (fm_unif('sku', i, 'p2') - 0.5)), 2) AS base_price,
       CASE WHEN fm_unif('sku', i, 'new') < 0.02 THEN floor(60 * fm_unif('sku', i, 'start'))::INTEGER ELSE 0 END AS first_day,
       CASE WHEN fm_unif('sku', i, 'del') < 0.02 THEN floor(60 * fm_unif('sku', i, 'end'))::INTEGER ELSE 59 END AS last_day
FROM s;

-- One delivery per night. Planted incidents:
--   29 Aug  the file is cut off after 540 SKUs (a failed upload);
--    8 Sep  35 SKUs appear twice (a retry appended to the file);
--   16–18 Sep  half the Audio range is quoted in USD;
--   21 Sep  the file arrives late (07:35);
--   29 Sep – 1 Oct  a quarter of the Kitchen range is priced in cents (×100);
--    8 Oct  the supplier re-sends the 7 Oct file (prices a day stale);
--   15 Oct  (today) the supplier's new export: stock_qty renamed to
--           stock_level, a new promo_flag column, ~8 % of EANs missing,
--           7 prices without their decimal point, and the file is late (08:52).
CREATE OR REPLACE TABLE fm_feed AS
WITH days AS (
    SELECT d, DATE '2026-08-17' + d::INTEGER AS delivery_date,
           CASE WHEN d = 52 THEN 51 ELSE d END AS src_d               -- 8 Oct re-sends 7 Oct
    FROM range(0, 60) t(d)
),
-- daily price moves: ~3 % of SKUs change price on a given day by -12..+15 %
moves AS (
    SELECT k.sku_no, dd.d,
           sum(CASE WHEN fm_unif('mv', k.sku_no, dd2.d) < 0.03 THEN ln(1 + 0.27 * fm_unif('mvs', k.sku_no, dd2.d) - 0.12) ELSE 0 END) AS log_move
    FROM fm_catalogue k, range(0, 60) dd(d), range(0, 60) dd2(d)
    WHERE dd2.d <= dd.d
    GROUP BY ALL
),
rows AS (
    SELECT dy.d, dy.delivery_date, k.*, m.log_move, dy.src_d
    FROM days dy JOIN fm_catalogue k ON dy.src_d BETWEEN k.first_day AND k.last_day
    JOIN moves m ON m.sku_no = k.sku_no AND m.d = dy.src_d
    WHERE NOT (dy.d = 12 AND k.sku_no > 560)                           -- 29 Aug: cut off
),
base AS (
    SELECT r.d, r.delivery_date, r.sku_no, r.sku, r.ean, r.category, r.product_name,
           round(r.base_price * exp(r.log_move)
                 * CASE WHEN r.category = 'Kitchen' AND r.d BETWEEN 43 AND 45 AND r.sku_no % 4 = 0 THEN 100
                        WHEN r.d = 59 AND r.sku_no % 173 = 0 THEN 100 ELSE 1 END, 2) AS unit_price,
           CASE WHEN r.category = 'Audio' AND r.d BETWEEN 30 AND 32 AND r.sku_no % 2 = 0 THEN 'USD' ELSE 'EUR' END AS currency,
           greatest(0, round(120 * fm_unif('stk', r.sku_no, 'lvl') + 40 * sin(r.src_d / 5.0 + r.sku_no)
                              - 30 * fm_unif('stk', r.sku_no, r.src_d)))::INTEGER AS stock,
           (DATE '2026-08-17' + r.src_d::INTEGER - 1)::TIMESTAMP + INTERVAL 22 HOUR AS price_valid_from,
           CASE r.d WHEN 35 THEN TIMESTAMP '2026-09-21 07:35:00'
                    WHEN 59 THEN TIMESTAMP '2026-10-15 08:52:00'
                    ELSE r.delivery_date::TIMESTAMP + INTERVAL 5 HOUR
                         + to_minutes(floor(10 + 35 * fm_unif('arr', r.d, 'm'))::BIGINT) END AS received_at
    FROM rows r
)
SELECT delivery_date, received_at, sku,
       CASE WHEN d = 59 AND fm_unif('ean', sku_no, 'miss') < 0.09 THEN NULL
            WHEN fm_unif('ean', sku_no, d) < 0.002 THEN left(ean, 12) ELSE ean END AS ean,  -- a stray truncated EAN
       product_name, category, unit_price, currency,
       CASE WHEN d = 59 THEN NULL ELSE stock END AS stock_qty,
       CASE WHEN d = 59 THEN stock END AS stock_level,
       CASE WHEN d = 59 THEN fm_unif('promo', sku_no, d) < 0.08 END AS promo_flag,
       price_valid_from
FROM base
UNION ALL  -- 8 Sep: the retry appended 35 SKUs a second time
SELECT delivery_date, received_at, sku, ean, product_name, category, unit_price, currency,
       stock, NULL, NULL, price_valid_from
FROM base WHERE d = 22 AND sku_no % 31 = 0 AND sku_no <= 1085;

-- Today's and yesterday's file on their own, with only the columns each
-- file actually has (what schema_check and profile_table look at).
CREATE OR REPLACE TABLE fm_today AS
SELECT delivery_date, received_at, sku, ean, product_name, category, unit_price, currency,
       stock_level, promo_flag, price_valid_from
FROM fm_feed WHERE delivery_date = DATE '2026-10-15';
CREATE OR REPLACE TABLE fm_yesterday AS
SELECT delivery_date, received_at, sku, ean, product_name, category, unit_price, currency,
       stock_qty, price_valid_from
FROM fm_feed WHERE delivery_date = DATE '2026-10-14';
