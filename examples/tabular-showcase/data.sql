-- Synthetic inputs of the anofox-tabular showcase (deterministic: hash-based
-- noise, no files). A made-up online shop moves its customer master to a new
-- CRM and asks: can we trust the data we are about to switch on?
CREATE OR REPLACE MACRO tq_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

-- 600 customers in the legacy CRM. Planted problems: ~7% malformed emails,
-- ~8% missing or truncated phones, ~6% VAT IDs with a wrong check digit.
-- Valid VAT IDs are synthetic: random digits plus the one check digit (or,
-- for FR, the two-digit key) that makes vat_is_valid() accept them.
CREATE OR REPLACE TABLE tq_customers_legacy AS
WITH c(country, cc, dial, ndig) AS (VALUES
    ('DE', 1, '+49 30 ', 8), ('FR', 2, '+33 1 ', 8), ('NL', 3, '+31 20 ', 7), ('IT', 4, '+39 06 ', 8)),
base AS (
    SELECT i, c.*, floor(10000000 * tq_unif('c', i, 'v'))::BIGINT AS v8,
           tq_unif('c', i, 'em') AS u_em, tq_unif('c', i, 'ph') AS u_ph, tq_unif('c', i, 'vt') AS u_vt
    FROM range(1, 601) t(i)
    JOIN c ON c.cc = 1 + floor(4 * tq_unif('c', i, 'cty'))::INTEGER
),
vat AS (  -- the check digit(s) that make each synthetic VAT ID valid
    SELECT b.i, min(cand) AS vat_ok
    FROM base b, LATERAL (
        SELECT CASE b.country
                 WHEN 'DE' THEN 'DE' || lpad(b.v8::VARCHAR, 8, '0') || d::VARCHAR
                 WHEN 'NL' THEN 'NL' || lpad(b.v8::VARCHAR, 8, '0') || d::VARCHAR || 'B01'
                 WHEN 'IT' THEN 'IT' || lpad(b.v8::VARCHAR, 7, '0') || '001' || d::VARCHAR
                 ELSE 'FR' || lpad(d::VARCHAR, 2, '0') || '3' || lpad((b.v8 % 100000000)::VARCHAR, 8, '0') END AS cand
        FROM range(0, CASE WHEN b.country = 'FR' THEN 100 ELSE 10 END) r(d)) x
    WHERE vat_is_valid(cand)
    GROUP BY b.i
)
SELECT b.i AS customer_id,
       ['Anna','Ben','Clara','David','Emma','Felix','Greta','Hugo','Ida','Jonas','Lea','Marco'][1 + (b.i % 12)] || ' ' ||
       ['Becker','Rossi','Martin','Visser','Huber','Dubois','Jansen','Wagner','Bianchi'][1 + ((b.i * 7) % 9)] AS name,
       CASE WHEN b.u_em < 0.04 THEN 'customer' || b.i || '(at)example.com'
            WHEN b.u_em < 0.07 THEN 'customer ' || b.i || '@example.com'
            ELSE 'customer' || b.i || '@example.com' END AS email,
       CASE WHEN b.u_ph < 0.05 THEN NULL
            WHEN b.u_ph < 0.08 THEN left(b.dial, 4) || '12345'
            ELSE b.dial || (2 * 10 ^ (b.ndig - 1) + floor(7 * 10 ^ (b.ndig - 1) * tq_unif('c', b.i, 'n')))::BIGINT::VARCHAR END AS phone,
       b.country,
       CASE WHEN tq_unif('c', b.i, 'sg') >= 0.3 THEN NULL               -- consumers have no VAT ID
            WHEN b.u_vt < 0.2 THEN left(v.vat_ok, length(v.vat_ok) - 1) || ((right(v.vat_ok, 1)::INTEGER + 3) % 10)::VARCHAR
            ELSE v.vat_ok END AS vat_id,
       DATE '2019-01-01' + floor(2000 * tq_unif('c', b.i, 'sd'))::INTEGER AS signup_date,
       CASE WHEN tq_unif('c', b.i, 'sg') < 0.3 THEN 'business' ELSE 'consumer' END AS segment
FROM base b LEFT JOIN vat v USING (i);
-- (Changing the last digit breaks the check for every country, FR keys
-- included. A few business customers get no ID at all: no candidate passed.)

-- The new CRM after the migration run: 12 customers lost, 14 new sign-ups,
-- 27 changed — emails re-typed, segments re-coded, a batch of phones that
-- lost their country code.
CREATE OR REPLACE TABLE tq_customers_new AS
SELECT customer_id, name,
       CASE WHEN tq_unif('m', customer_id, 'e') < 0.025 THEN lower(replace(email, 'customer', 'c.')) ELSE email END AS email,
       CASE WHEN tq_unif('m', customer_id, 'p') < 0.02 AND phone IS NOT NULL THEN regexp_replace(phone, '^\+\d+ ', '0') ELSE phone END AS phone,
       country, vat_id, signup_date,
       CASE WHEN tq_unif('m', customer_id, 's') < 0.015 THEN 'smb' ELSE segment END AS segment
FROM tq_customers_legacy
WHERE tq_unif('m', customer_id, 'drop') >= 0.015
UNION ALL
SELECT 600 + i, 'New Customer ' || i, 'new' || i || '@example.com', '+49 30 ' || (30000000 + i * 7919)::VARCHAR,
       'DE', NULL, DATE '2026-09-01' + i::INTEGER, 'consumer'
FROM range(1, 15) t(i);

-- 120 days of orders (Jun–Sep 2026), ~40 a day with a weekly rhythm. Planted:
-- a feed outage on 2026-08-12 (only 9 orders arrive), a pricing bug on
-- 2026-09-03..05 (amounts ×10), 4 duplicated order IDs, 5 negative amounts
-- and ~2% orders whose customer does not exist (orphans).
CREATE OR REPLACE TABLE tq_orders AS
WITH days AS (
    SELECT DATE '2026-06-01' + d::INTEGER AS order_date, d,
           CASE WHEN DATE '2026-06-01' + d::INTEGER = DATE '2026-08-12' THEN 9
                ELSE round(34 + 10 * sin(2 * pi() * d / 7.0) + 8 * tq_unif('d', d, 'n'))::INTEGER END AS n
    FROM range(0, 120) t(d)
), o AS (
    SELECT order_date, d, k, row_number() OVER (ORDER BY order_date, k) AS seq
    FROM days, range(1, 70) r(k) WHERE k <= n
)
SELECT 100000 + CASE WHEN seq % 997 = 0 THEN seq - 1 ELSE seq END AS order_id,
       order_date,
       CASE WHEN tq_unif('o', seq, 'orph') < 0.02 THEN 900 + floor(50 * tq_unif('o', seq, 'c'))::INTEGER
            ELSE 1 + floor(600 * tq_unif('o', seq, 'c'))::INTEGER END AS customer_id,
       1 + floor(5 * tq_unif('o', seq, 'q') ^ 2)::INTEGER AS quantity,
       round(CASE WHEN seq % 811 = 0 THEN -1 ELSE 1 END
             * (1 + floor(5 * tq_unif('o', seq, 'q') ^ 2)) * (12 + 45 * tq_unif('o', seq, 'p'))
             * CASE WHEN order_date BETWEEN DATE '2026-09-03' AND DATE '2026-09-05' THEN 10 ELSE 1 END, 2) AS amount,
       CASE WHEN tq_unif('o', seq, 'st') < 0.86 THEN 'delivered' WHEN tq_unif('o', seq, 'st') < 0.95 THEN 'shipped'
            WHEN tq_unif('o', seq, 'st') < 0.995 THEN 'returned' ELSE 'unknwon' END AS status,
       ['web', 'app', 'marketplace'][1 + floor(3 * tq_unif('o', seq, 'ch'))::INTEGER] AS channel
FROM o;
