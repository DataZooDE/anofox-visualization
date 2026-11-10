-- @from coverage-20261110-0707 map-flows: the version that worked
-- @expect clean
SELECT {'lon': 10.0, 'lat': 53.55, 'name': 'Hamburg'}::ORIGIN, {'lon': lon, 'lat': lat, 'name': d}::DEST, n::SIZE, 'Flows'::TITLE FROM (VALUES ('Lübeck', 10.69, 53.87, 3), ('Göttingen', 9.93, 51.53, 5), ('Fürth', 10.99, 49.48, 2)) t(d, lon, lat, n);
