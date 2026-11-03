-- @title Where does our revenue come from, from country down to district?
-- @group Sales & customers
-- @tags  maps, drill-down, regions
-- @description Revenue on a map: European countries first, click Germany for its states, click a state for its districts.
--
-- A map drill-down (#954): one ::REGION map, three levels. The level picks
-- the code column, and the code picks the built-in boundaries: ISO country
-- codes draw the world layer, 'DE-BY' codes the German states, district
-- codes (AGS) the German districts. A click goes one level deeper; the
-- breadcrumb goes back; the path is in the URL (?v.geo_path=DEU).
--
-- SYNTHETIC DATA, made up in the SQL below: revenue per German district from
-- its code, a fixed figure per other country. Only Germany has states and
-- districts in the built-in boundaries; another country drills to an empty
-- level with a note.
--
-- Natively, run from web/:  dashboard --check examples/drill-revenue-map.sql

SELECT 'Where does our revenue come from, from country down to district?'::DASHBOARD_TITLE;

-- ── Data ─────────────────────────────────────────────────────────────────────
-- Every German district with its state (the district code's first two digits
-- are the state's number, the second part of de-states' `codes`).
CREATE OR REPLACE TABLE rm_states AS
SELECT key AS state_code, name_en AS state, split_part(codes, '|', 2) AS land
FROM read_csv('data/boundaries/de-states.csv', types = {'codes': 'VARCHAR'});

CREATE OR REPLACE TABLE rm_districts AS
SELECT d.key AS ags, d.name_de AS district, s.state_code, s.state,
       -- made-up revenue in k€, fixed by the district code
       50 + (d.key::INTEGER * 7919) % 950 AS revenue
FROM read_csv('data/boundaries/de-districts.csv', types = {'key': 'VARCHAR'}) d
JOIN rm_states s ON s.land = substr(d.key, 1, 2);

CREATE OR REPLACE TABLE rm_countries AS
SELECT * FROM (VALUES
    ('FRA', 'France', 61200), ('NLD', 'Netherlands', 38400), ('BEL', 'Belgium', 17900),
    ('AUT', 'Austria', 22300), ('CHE', 'Switzerland', 26100), ('POL', 'Poland', 19800),
    ('DNK', 'Denmark', 11400), ('CZE', 'Czechia', 9700), ('ITA', 'Italy', 33500))
    t(iso3, country, revenue)
UNION ALL
SELECT 'DEU', 'Germany', sum(revenue) FROM rm_districts;

-- One row per place at the current level: its code (the map key and the
-- drill key), its name and revenue. (A static render — dashboard --svg, an
-- export — has no drill state: it shows the country level.)
CREATE OR REPLACE MACRO rm_level() AS TABLE
SELECT code, name, revenue FROM (
    SELECT 'country' AS lvl, iso3 AS code, country AS name, revenue FROM rm_countries
    UNION ALL
    SELECT 'state', state_code, state, sum(revenue) FROM rm_districts
    WHERE getvariable('country') = 'DEU' GROUP BY ALL
    UNION ALL
    SELECT 'district', ags, district, revenue FROM rm_districts
    WHERE getvariable('country') = 'DEU' AND state_code = getvariable('state')
)
WHERE lvl = coalesce(getvariable('geo_level'), 'country');

-- ── Page ─────────────────────────────────────────────────────────────────────
SELECT ('**Click a country** to see its states, **click a state** to see its districts; the breadcrumb above the map goes back. Only Germany has states and districts in the built-in boundaries. A click passes the region''s **code** (`DEU`, `DE-BY`), not its shown name, so the next level filters on it with `getvariable(''country'')` / `getvariable(''state'')`.')::MARKDOWN, 'How to read this'::TITLE;

SELECT 8::COL;
SELECT 'geo: country > state > district'::DRILL;
SELECT 'Nothing below this level in the sample: only Germany has states and districts.'::EMPTY;
SELECT code::REGION, name::LABEL,
       revenue AS "Revenue (k€)" ::BARCHART, 'quantile 5'::CLASSES,
       ('Revenue by ' || coalesce(getvariable('geo_level'), 'country'))::TITLE
FROM rm_level() ORDER BY revenue DESC;

SELECT 4::COL;
SELECT name::XAXIS, revenue AS "Revenue (k€)" ::BARCHART, 1::FLIP,
       ('Top 10 by ' || coalesce(getvariable('geo_level'), 'country'))::TITLE
FROM rm_level() ORDER BY revenue DESC LIMIT 10;
