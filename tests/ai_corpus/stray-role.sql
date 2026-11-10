-- @from #1076 mutation fuzz: a role cast after the FROM clause (#1083)
-- @expect sql-error goes on a column of the SELECT list
SELECT d::XAXIS, n::BARCHART, 'Flows'::TITLE FROM (VALUES ('Lübeck', 10.69, 53.87, 3), ('Göttingen', 9.93, 51.53, 5), ('Fürth', 10.99, 49.48, 2)) t(d, lon, lat, n)
::SCATTER;
