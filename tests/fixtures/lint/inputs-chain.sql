-- @title Revenue for one product (inputs that feed each other)
-- @group Lint fixtures
--
-- Two inputs, the second filtered by the first, and panels that read both:
-- the lint primes each input's default before the next statement, so the
-- browser's lint_step needs a step per input (tests/lint_step.rs).

CREATE OR REPLACE TABLE orders AS
SELECT (DATE '2026-01-01' + INTERVAL (i % 60) DAY)::DATE AS day,
       ['Tea', 'Coffee', 'Cocoa'][1 + i % 3] AS product,
       ['Small', 'Large'][1 + (i // 3) % 2] AS size,
       (i % 17) + 1 AS qty
FROM range(0, 360) t(i);

SELECT product ::BUTTONS FROM (SELECT DISTINCT product FROM orders ORDER BY product);

SELECT size ::DROPDOWN FROM (SELECT DISTINCT size FROM orders WHERE product = getvariable('product') ORDER BY size);

SELECT sum(qty) ::METRIC, 'Units'::TITLE FROM orders WHERE product = getvariable('product') AND size = getvariable('size');

SELECT day ::XAXIS, sum(qty) ::LINECHART, 'Units per day'::TITLE FROM orders
WHERE product = getvariable('product') AND size = getvariable('size') GROUP BY day ORDER BY day;

-- empty unless both defaults are bound: an empty-panel only when they aren't
SELECT product ::XAXIS, count(*) ::BARCHART, 'Orders'::TITLE FROM orders
WHERE product = getvariable('product') AND size = getvariable('size') GROUP BY product;
