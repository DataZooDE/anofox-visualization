-- @from smoke-claude-sonnet-5-20261110-231930 tbl-suppliers: lead days and order lines beside the on-time bars, not drawn (#1100)
-- @expect chart/unused-measure SELECT columns 3, 4: a number with no role, not drawn
CREATE TABLE suppliers AS SELECT * FROM (VALUES
  ('Seals', 32.9, 11.4, 140), ('Fasteners', 94.0, 6.2, 310)) t(name, on_time, lead_days, lines);
SELECT name::XAXIS AS supplier, on_time::BARCHART AS "On-time rate", lead_days AS "Avg lead time (days)",
  lines AS "Order lines", 1::FLIP, 'Supplier on-time delivery'::TITLE FROM suppliers ORDER BY 2;
