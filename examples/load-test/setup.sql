-- Data for the load-test reference dashboard (scripts/load-test.mjs). Runs in
-- the session that then serves dash/ (the locked server snapshots it).
-- Deterministic: 2,000,000 order lines over two years, 4 regions x 3 channels
-- x 8 categories (`load-test.mjs --rows N` rewrites the range() below).
CREATE OR REPLACE TABLE orders AS
SELECT i AS order_id,
       DATE '2024-01-01' + (i % 731)::INT AS day,
       ['EU', 'Americas', 'APAC', 'MEA'][1 + (hash(i) % 4)::INT] AS region,
       ['web', 'app', 'store'][1 + (hash(i * 7) % 3)::INT] AS channel,
       ['Food', 'Drinks', 'Home', 'Garden', 'Toys', 'Books', 'Sports', 'Beauty'][1 + (hash(i * 13) % 8)::INT] AS category,
       1 + (hash(i * 31) % 5)::INT AS qty,
       round(5 + (hash(i * 17) % 20000) / 100.0, 2) AS price,
       'c' || lpad(((hash(i * 3) % 200000))::VARCHAR, 6, '0') AS customer
FROM range(2000000) t(i);
