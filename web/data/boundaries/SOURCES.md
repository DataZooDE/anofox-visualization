# Built-in boundaries — sources and licences

Built by `scripts/build-boundaries.sh` (DuckDB + spatial) on 2026-10-17.
Each layer is a CSV with the columns `key,codes,name_en,name_de,alt,wkt`
(WGS84 lon/lat). Geometries are coverage-simplified per layer, so shared
borders stay shared, then rounded to 3 decimals (about 100 m).

| Layer | Source | Features | Simplification | Licence | Shipped |
|---|---|---|---|---|---|
| `world` | Natural Earth 1:110m Admin 0 – Countries, v5.1.2 | 177 | none (rounded only) | public domain | yes; embedded in native builds |
| `world-50m` | Natural Earth 1:50m Admin 0 – Countries, v5.1.2 | 242 | 0.02° | public domain | yes; browser only (not embedded) |
| `de-states` | BKG VG2500, Länder (`vg2500_lan`), Stand 31.12.2025 | 16 | 0.005° | Datenlizenz Deutschland – Namensnennung 2.0 | yes; embedded |
| `de-districts` | BKG VG2500, Kreise (`vg2500_krs`), Stand 31.12.2025 | 400 | 0.005° | Datenlizenz Deutschland – Namensnennung 2.0 | yes; embedded |
| `nuts0` … `nuts3` | Eurostat/GISCO NUTS 2024, 1:20M | — | none | GISCO terms: **non-commercial only** | **no**: each user runs `scripts/fetch-nuts.sh` (gitignored) |

`../maps/world-population.csv` holds Natural Earth's `POP_EST` per country
(the chart gallery's world map), from the same 1:110m file.

## Codes and names

- `world`: `key` = `ADM0_A3`; `codes` = ISO 3166-1 alpha-2 / alpha-3
  (`ISO_A2_EH`, `ISO_A3_EH`, `ISO_A2`, `ISO_A3`; so France is `FR`/`FRA`,
  Norway `NO`/`NOR`); `name_en` = `NAME`, `name_de` = `NAME_DE`; `alt` =
  `NAME_EN`, `NAME_LONG`, `FORMAL_EN`. Default Natural Earth point of view.
- `de-states`: `key` = ISO 3166-2 (`DE-BY`); `codes` = the 2-digit AGS (`09`)
  and NUTS 1 (`DE2`); `name_de` = `GEN`, `name_en` = the English name.
- `de-districts`: `key` = the 5-digit AGS (`09162`); `codes` = NUTS 3
  (`DE212`); a NUTS 2 code (`DE21`) matches all districts in it;
  `name_en` = `name_de` = `GEN`; `alt` = `GEN (BEZ)` ("München (Landkreis)").

## URLs

- Natural Earth: <https://cdn.jsdelivr.net/gh/nvkelso/natural-earth-vector@v5.1.2/geojson/>
  `ne_110m_admin_0_countries.geojson`, `ne_50m_admin_0_countries.geojson`.
  Terms: <https://www.naturalearthdata.com/about/terms-of-use/> — "All
  versions of Natural Earth raster + vector map data found on this website
  are in the public domain." Credit is optional; maps print "Made with
  Natural Earth".
- BKG VG2500: <https://daten.gdz.bkg.bund.de/produkte/vg/vg2500/aktuell/vg2500_12-31.utm32s.gpkg.zip>
  (product page <https://gdz.bkg.bund.de>). Terms
  (`nutzungsbedingungen_vg2500.pdf`, Stand 30.06.2025): the data are
  provided under Datenlizenz Deutschland – Namensnennung 2.0
  (<https://www.govdata.de/dl-de/by-2-0>); every public display needs the
  source note and a note of modification, with "BKG" linked to
  <https://www.bkg.bund.de> and "dl-de/by-2-0" to the licence on a web page:

  > © BKG (Jahr des letzten Datenbezugs) dl-de/by-2-0, Datenquellen:
  > https://sgx.geodatenzentrum.de/web_public/gdz/datenquellen/datenquellen_vg_nuts.pdf

  The map writer prints, on every map drawn from a `de-*` layer:
  "© BKG (2026) dl-de/by-2-0, Datenquellen: https://sgx.geodatenzentrum.de/web_public/gdz/datenquellen/datenquellen_vg_nuts.pdf
  · geometry modified (simplified)" (German locale: "Geometrie verändert
  (vereinfacht)"), with the links. Update `BKG_YEAR` in
  `src/map/boundaries.rs` when the data is downloaded again.
- Eurostat/GISCO NUTS: see the terms quoted in `scripts/fetch-nuts.sh`.
