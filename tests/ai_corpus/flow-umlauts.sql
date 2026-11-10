-- @from coverage-20261110-0707 map-flows: place names with umlauts (crashed, #1055)
-- @expect no-crash
SELECT 'Hamburg' AS dc_name::ORIGIN, d AS dest::DEST, n::SIZE, 'Flows'::TITLE FROM (VALUES ('Lübeck', 3), ('Göttingen', 5), ('Fürth', 2)) t(d, n);
