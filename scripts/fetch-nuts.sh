#!/usr/bin/env bash
# Fetch the EU NUTS regions (Eurostat/GISCO, 2024, 1:20M) for your own use and
# write them as boundary layers next to the built-in pack:
#   web/data/boundaries/nuts0.csv … nuts3.csv
# Use them with `'nuts2'::BOUNDARIES` next to a `code::REGION` column
# (`'FR10'`, `'DE21'`, `'ITC4'`) in the browser builder and in servers built
# after the files are in place. They are NOT part of the anofox-visualization
# pack and must not be committed or redistributed with it (.gitignore keeps
# them out of the repository).
#
#   scripts/fetch-nuts.sh --accept-terms [workdir]
#
# Needs curl and a `duckdb` CLI (it installs the `spatial` extension).
#
# ---------------------------------------------------------------------------
# Terms of use. You download these data from Eurostat/GISCO yourself, and you
# — not DataZoo — hold the licence. As published on the GISCO "Statistical
# units" page (https://ec.europa.eu/eurostat/web/gisco/geodata/statistical-units,
# "Last page update: 26 May 2026", read on 17 October 2026), verbatim:
#
#   "In addition to the general copyright and licence policy that applies to
#   the Eurostat website, the following specific rules apply to the datasets
#   you are downloading. The download and use of these data are subject to you
#   accepting the following clauses:
#   The Commission agrees to grant the non-exclusive and non-transferable right
#   to use and process the Eurostat/GISCO geographical data downloaded from
#   this page (the 'data').
#   The permission to use the data is granted on condition that:
#     the data will not be used for commercial purposes
#     the source will be acknowledged. A copyright notice, as specified below,
#     must be visible on any printed or electronic publication using the data
#     downloaded from this page.
#   Copyright notice
#   When you use data downloaded from our webpages on: census coastal local
#   administrative units (LAU) territorial units for statistics (NUTS) cities
#   and functional urban areas
#   In any printed or electronic publication, in addition to any other rules
#   that apply to the Eurostat website, the data source must be acknowledged in
#   the legend of the map and in the introductory page of the publication with
#   the following copyright notice:
#   EN: © EuroGeographics for the administrative boundaries
#   FR: © EuroGeographics pour les limites administratives
#   DE: © EuroGeographics bezüglich der Verwaltungsgrenzen
#   For publications in languages other than English, French or German, the
#   translation of the copyright notice in the language of the publication must
#   be used.
#   If you intend to use the data commercially, please contact EuroGeographics
#   for information about their licence agreements."
#
# So: non-commercial use only (commercial use needs a EuroGeographics
# licence), and every map that shows these regions must carry
# "© EuroGeographics for the administrative boundaries" — the map writer adds
# it to the attribution line of any map drawn from a nuts* layer.
# ---------------------------------------------------------------------------
set -euo pipefail

if [ "${1:-}" != "--accept-terms" ]; then
  sed -n '/^# Terms of use/,/^# ------/p' "$0" | sed 's/^# \{0,1\}//'
  echo
  echo "Read the terms above. If you accept them, run: $0 --accept-terms [workdir]" >&2
  exit 2
fi
shift

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/web/data/boundaries"
WORK="${1:-$(mktemp -d)}"
BASE="https://gisco-services.ec.europa.eu/distribution/v2/nuts/geojson"
mkdir -p "$OUT" "$WORK"

for l in 0 1 2 3; do
  f="NUTS_RG_20M_2024_4326_LEVL_${l}.geojson"
  [ -s "$WORK/$f" ] || curl -fsSL --retry 3 -o "$WORK/$f" "$BASE/$f"
done

cd "$WORK"
duckdb -c "
INSTALL spatial; LOAD spatial;
$(for l in 0 1 2 3; do cat <<SQL
COPY (SELECT NUTS_ID AS key, CNTR_CODE AS codes, NAME_LATN AS name_en, NAME_LATN AS name_de,
             CASE WHEN NUTS_NAME <> NAME_LATN THEN NUTS_NAME ELSE '' END AS alt,
             ST_AsText(ST_MakeValid(ST_ReducePrecision(geom, 0.001))) AS wkt
      FROM ST_Read('NUTS_RG_20M_2024_4326_LEVL_${l}.geojson') ORDER BY key)
  TO '$OUT/nuts${l}.csv' (HEADER);
SQL
done)
"
ls -l "$OUT"/nuts*.csv
echo "Use them with e.g. SELECT nuts_id::REGION, value::BARCHART, 'nuts2'::BOUNDARIES …"
