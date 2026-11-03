-- The drill-revenue-map example's states outside Germany (#956): code, name
-- and a made-up revenue (k€) for the nine other countries, from the built-in
-- world-states layer. Run from web/:
--   duckdb < ../examples/drill-revenue-map/states.sql
-- Overseas parts (Guyane, Réunion, the Dutch Caribbean) are left out: they
-- would shrink the European map to a corner.
INSTALL spatial; LOAD spatial;
COPY (
    SELECT key AS state_code, split_part(key, '-', 1) AS alpha2, name_en AS state,
           -- made-up, fixed by the code
           40 + (hash(key) % 2400)::INTEGER AS revenue
    FROM read_csv('data/boundaries/world-states.csv', columns = {
        'key': 'VARCHAR', 'codes': 'VARCHAR', 'name_en': 'VARCHAR', 'name_de': 'VARCHAR',
        'alt': 'VARCHAR', 'wkt': 'VARCHAR'}, header = true)
    WHERE split_part(key, '-', 1) IN ('FR', 'NL', 'BE', 'AT', 'CH', 'PL', 'DK', 'CZ', 'IT')
      AND key LIKE '__-%'
      AND ST_X(ST_Centroid(ST_GeomFromText(wkt))) BETWEEN -12 AND 35
      AND ST_Y(ST_Centroid(ST_GeomFromText(wkt))) BETWEEN 34 AND 72
    ORDER BY key
) TO 'data/drill-revenue-map/states.csv' (HEADER);
