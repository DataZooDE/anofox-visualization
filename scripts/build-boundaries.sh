#!/usr/bin/env bash
# Build the built-in boundary pack web/data/boundaries/*.csv (plan-maps §4).
#
# Run by a maintainer when a source changes; the outputs are committed. Needs
# curl, unzip and a `duckdb` CLI (it installs the `spatial` extension).
#
#   scripts/build-boundaries.sh [workdir]     (downloads are kept in workdir)
#
# Layers (one CSV each, columns key,codes,name_en,name_de,alt,wkt):
#   world        Natural Earth 1:110m admin-0 countries   (public domain)
#   world-50m    Natural Earth 1:50m admin-0 countries    (public domain; an
#                optional layer: in web/data, not embedded in native builds)
#   de-states    BKG VG2500 Länder                        (dl-de/by-2-0)
#   de-districts BKG VG2500 Kreise                        (dl-de/by-2-0)
# and web/data/maps/world-population.csv (Natural Earth's POP_EST) for the
# chart gallery's world map.
#
# `key` is the layer's main code (ISO 3166-1 alpha-3 / ADM0_A3, ISO 3166-2
# `DE-BY`, 5-digit AGS); `codes` lists the other codes `|`-separated (ISO
# alpha-2, the 2-digit AGS of a state, NUTS 1/3); `alt` lists other names.
# Geometries: coverage-simplified per layer (shared borders stay shared),
# then rounded to 3 decimals (~100 m), WGS84 lon/lat.
#
# EU NUTS regions are NOT part of the pack (GISCO's terms forbid commercial
# use): see scripts/fetch-nuts.sh, which a user runs for themselves.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/web/data/boundaries"
WORK="${1:-$(mktemp -d)}"
mkdir -p "$OUT" "$WORK" "$ROOT/web/data/maps"

# Pinned sources
NE_VER="v5.1.2"
NE="https://cdn.jsdelivr.net/gh/nvkelso/natural-earth-vector@${NE_VER}/geojson"
BKG="https://daten.gdz.bkg.bund.de/produkte/vg/vg2500/aktuell/vg2500_12-31.utm32s.gpkg.zip"

fetch() { # url file
  [ -s "$WORK/$2" ] || curl -fsSL --retry 3 -o "$WORK/$2" "$1"
}
fetch "$NE/ne_110m_admin_0_countries.geojson" ne110.geojson
fetch "$NE/ne_50m_admin_0_countries.geojson" ne50.geojson
fetch "$BKG" vg2500.zip
[ -d "$WORK/vg2500" ] || unzip -q -o "$WORK/vg2500.zip" -d "$WORK/vg2500"
GPKG="$(find "$WORK/vg2500" -name 'DE_VG2500.gpkg' | head -1)"
STAND="$(find "$WORK/vg2500" -name 'aktualitaet.txt' -exec head -1 {} \; | tr -d '\r' | head -1)"

cd "$WORK"
duckdb -c "
INSTALL spatial; LOAD spatial;
-- Natural Earth: drop the -99 placeholders, keep distinct alternative codes/names
CREATE MACRO codes(l) AS array_to_string(list_distinct(list_filter(l, x -> x IS NOT NULL AND x <> '-99' AND x <> '')), '|');
CREATE TABLE src AS
SELECT 'world' AS layer, ADM0_A3 AS key, codes([ISO_A2_EH, ISO_A3_EH, ISO_A2, ISO_A3]) AS codes, NAME AS name_en, NAME_DE AS name_de,
       codes(list_filter([NAME_EN, NAME_LONG, FORMAL_EN], x -> x <> NAME)) AS alt, geom, 0.0 AS tol FROM ST_Read('ne110.geojson')
UNION ALL
SELECT 'world-50m', ADM0_A3, codes([ISO_A2_EH, ISO_A3_EH, ISO_A2, ISO_A3]), NAME, NAME_DE,
       codes(list_filter([NAME_EN, NAME_LONG, FORMAL_EN], x -> x <> NAME)), geom, 0.02 FROM ST_Read('ne50.geojson')
UNION ALL
SELECT 'de-states', 'DE-' || LKZ, codes([AGS, NUTS]), coalesce(en.n, GEN), GEN, '',
       ST_Transform(geom, 'EPSG:25832', 'EPSG:4326', true), 0.005
FROM ST_Read('$GPKG', layer='vg2500_lan') LEFT JOIN (VALUES
  ('BW','Baden-Württemberg'),('BY','Bavaria'),('BE','Berlin'),('BB','Brandenburg'),('HB','Bremen'),
  ('HH','Hamburg'),('HE','Hesse'),('MV','Mecklenburg-Western Pomerania'),('NI','Lower Saxony'),
  ('NW','North Rhine-Westphalia'),('RP','Rhineland-Palatinate'),('SL','Saarland'),('SN','Saxony'),
  ('ST','Saxony-Anhalt'),('SH','Schleswig-Holstein'),('TH','Thuringia')) en(k, n) ON en.k = LKZ
WHERE GF = 9
UNION ALL
SELECT 'de-districts', AGS, codes([NUTS]), GEN, GEN, GEN || ' (' || BEZ || ')',
       ST_Transform(geom, 'EPSG:25832', 'EPSG:4326', true), 0.005
FROM ST_Read('$GPKG', layer='vg2500_krs') WHERE GF = 9;
CREATE TABLE ids AS SELECT row_number() OVER () AS id, * FROM src;
-- coverage simplification keeps shared borders shared
CREATE TABLE simp AS
WITH g AS (SELECT layer, list(geom ORDER BY id) AS gs, list(id ORDER BY id) AS ids, any_value(tol) AS tol
           FROM ids WHERE tol > 0 GROUP BY layer),
d AS (SELECT ids, unnest(ST_Dump(ST_CoverageSimplify(gs, tol))) AS dd FROM g)
SELECT ids[dd.path[1]] AS id, ST_Union_Agg(dd.geom) AS sgeom FROM d GROUP BY ALL
UNION ALL SELECT id, geom FROM ids WHERE tol = 0;
CREATE TABLE res AS
SELECT i.layer, i.key, i.codes, i.name_en, i.name_de, i.alt,
       ST_AsText(ST_MakeValid(ST_ReducePrecision(s.sgeom, 0.001))) AS wkt
FROM ids i JOIN simp s USING (id) ORDER BY i.layer, i.key;
SELECT layer, count(*) AS n, count(DISTINCT key) AS keys,
       count(*) FILTER (WHERE wkt LIKE '%EMPTY%') AS empty FROM res GROUP BY ALL ORDER BY ALL;
COPY (SELECT * EXCLUDE layer FROM res WHERE layer = 'world') TO '$OUT/world.csv' (HEADER);
COPY (SELECT * EXCLUDE layer FROM res WHERE layer = 'world-50m') TO '$OUT/world-50m.csv' (HEADER);
COPY (SELECT * EXCLUDE layer FROM res WHERE layer = 'de-states') TO '$OUT/de-states.csv' (HEADER);
COPY (SELECT * EXCLUDE layer FROM res WHERE layer = 'de-districts') TO '$OUT/de-districts.csv' (HEADER);
-- the gallery's world map: Natural Earth's population estimate per country
COPY (SELECT ADM0_A3 AS iso3, NAME AS country, POP_EST::BIGINT AS population, POP_YEAR::INTEGER AS year
      FROM ST_Read('ne110.geojson') ORDER BY 1) TO '$ROOT/web/data/maps/world-population.csv' (HEADER);
"
echo "VG2500 Stand: ${STAND:-unknown}"
ls -l "$OUT"/*.csv
