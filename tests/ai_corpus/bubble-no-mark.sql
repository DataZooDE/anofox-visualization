-- @from smoke-claude-sonnet-5-20261110-231930 rel-bubble: ::XAXIS/::YAXIS/::SIZE but no chart role — drew the first ::LABEL as a heading (#1100)
-- @expect render-error no chart role: cast the measure to a mark, e.g. ::SCATTER
CREATE TABLE districts AS SELECT * FROM (VALUES
  ('Kiel', 12.0, 97.5, 247000), ('Lübeck', 40.0, 93.0, 216000), ('Celle', 210.0, 71.0, 69000)) t(district, dc_km, on_time_pct, population);
SELECT dc_km::XAXIS, on_time_pct::YAXIS AS "On-time rate (%)", population::SIZE, district::LABEL,
  'On-time delivery rate vs distance to distribution centre'::TITLE FROM districts;
