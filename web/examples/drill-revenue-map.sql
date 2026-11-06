-- @title Where does our revenue come from, from country down to district?
-- @group Sales & customers
-- @tags  maps, drill-down, regions
-- @description Revenue on a map: European countries first, click one for its states or provinces; Germany's states drill on to their districts.
--
-- A map drill-down (#954): one ::REGION map, three levels, and a ::TREEMAP
-- under the same drill path below it (#1002). The level picks
-- the code column, and the code picks the built-in boundaries: ISO country
-- codes draw the world layer, ISO 3166-2 codes ('FR-75', 'IT-MI') the states
-- and provinces of world-states (#956), 'DE-BY' the German states and
-- district codes (AGS) the German districts. A click goes one level deeper;
-- the breadcrumb goes back; the path is in the URL (?v.geo_path=FRA).
--
-- SYNTHETIC DATA: revenue per German district made up below from its code;
-- per state elsewhere from web/data/drill-revenue-map/states.csv (written by
-- examples/drill-revenue-map/states.sql from the world-states layer). Only
-- Germany has a level below its states; a click there shows a note.
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

-- The states and provinces of the other countries (made-up revenue).
CREATE OR REPLACE TABLE rm_world_states AS
FROM read_csv('data/drill-revenue-map/states.csv');

CREATE OR REPLACE TABLE rm_countries AS
SELECT c.iso3, c.alpha2, c.country, sum(s.revenue) AS revenue
FROM (VALUES ('FRA', 'FR', 'France'), ('NLD', 'NL', 'Netherlands'), ('BEL', 'BE', 'Belgium'),
             ('AUT', 'AT', 'Austria'), ('CHE', 'CH', 'Switzerland'), ('POL', 'PL', 'Poland'),
             ('DNK', 'DK', 'Denmark'), ('CZE', 'CZ', 'Czechia'), ('ITA', 'IT', 'Italy'))
     c(iso3, alpha2, country)
JOIN rm_world_states s ON s.alpha2 = c.alpha2
GROUP BY ALL
UNION ALL
SELECT 'DEU', 'DE', 'Germany', sum(revenue) FROM rm_districts;

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
    SELECT 'state', s.state_code, s.state, s.revenue FROM rm_world_states s
    JOIN rm_countries c ON c.alpha2 = s.alpha2 WHERE c.iso3 = getvariable('country')
    UNION ALL
    SELECT 'district', ags, district, revenue FROM rm_districts
    WHERE getvariable('country') = 'DEU' AND state_code = getvariable('state')
)
WHERE lvl = coalesce(getvariable('geo_level'), 'country');

-- ── Page ─────────────────────────────────────────────────────────────────────
SELECT ('**Click a country** to see its states or provinces; in Germany, **click a state** to see its districts. The breadcrumb above the map goes back. States and provinces come from the built-in *world-states* boundaries (Natural Earth), loaded only when the map first shows them; only Germany has a level below. A click passes the region''s **code** (`DEU`, `DE-BY`), not its shown name, so the next level filters on it with `getvariable(''country'')` / `getvariable(''state'')`.')::MARKDOWN, 'How to read this'::TITLE;

SELECT 8::COL;
SELECT 'geo: country > state > district'::DRILL;
SELECT 'Nothing below this level in the sample: only Germany has districts.'::EMPTY;
SELECT code::REGION, name::LABEL,
       revenue AS "Revenue (k€)" ::BARCHART, 'quantile 5'::CLASSES,
       ('Revenue by ' || coalesce(getvariable('geo_level'), 'country'))::TITLE
FROM rm_level() ORDER BY revenue DESC;

SELECT 4::COL;
SELECT name::XAXIS, revenue AS "Revenue (k€)" ::BARCHART, 1::FLIP,
       ('Top 10 by ' || coalesce(getvariable('geo_level'), 'country'))::TITLE
FROM rm_level() ORDER BY revenue DESC LIMIT 10;

-- The same drill as a ::TREEMAP (#1002): the same 'geo: …' path, so a click on
-- a tile or on the map moves both. ::XAXIS is the tile's key (the code the
-- drill passes on), ::LABEL the name it shows.
SELECT 12::COL;
-- @shared geo_level, geo_depth, geo_path, country, state, district
SELECT 'geo: country > state > district'::DRILL;
SELECT 'Nothing below this level in the sample: only Germany has districts.'::EMPTY;
SELECT code::XAXIS, name::LABEL, revenue AS "Revenue (k€)" ::TREEMAP,
       ('Share of revenue by ' || coalesce(getvariable('geo_level'), 'country') || ' (click a tile to drill, in step with the map)')::TITLE
FROM rm_level() ORDER BY revenue DESC;
