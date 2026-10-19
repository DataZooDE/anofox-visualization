-- @title Where do our deliveries and customers cluster, and which regions are underserved?
-- @group Logistics & production
-- @tags  maps, customers, delivery
-- @description Where customers cluster around our three DCs, which districts we hardly reach, and how late deliveries get with distance.
--
-- A wholesaler's customer and delivery coverage, district by district.
--
-- STORED DATA, SYNTHETIC. The tables in web/data/delivery-coverage/*.csv are
-- written by examples/delivery-coverage/synthetic.py (Python standard
-- library, seed 11): a made-up population per German district, 3 made-up
-- distribution centres, customers placed inside their districts and 12 weeks
-- of deliveries. The district and state shapes are the built-in boundaries
-- (BKG VG2500, drawn from ::REGION codes); nothing here is real data.
--
-- The question: where do our deliveries and customers cluster, and which
-- regions are underserved? Asked by the head of sales logistics, who plans
-- where the next depot goes.
--
-- Tabs: Coverage · Customers · On time · Underserved.
-- Natively, run from web/:  dashboard --check examples/delivery-coverage.sql

SELECT 'Where do our deliveries and customers cluster, and which regions are underserved?'::DASHBOARD_TITLE;

-- ── Data ─────────────────────────────────────────────────────────────────────
CREATE OR REPLACE TABLE dc_districts AS FROM read_csv('data/delivery-coverage/districts.csv', types = {'ags': 'VARCHAR'});
CREATE OR REPLACE TABLE dc_dcs AS FROM read_csv('data/delivery-coverage/dcs.csv');
CREATE OR REPLACE TABLE dc_customers AS FROM read_csv('data/delivery-coverage/customers.csv', types = {'ags': 'VARCHAR'});
CREATE OR REPLACE TABLE dc_deliveries AS FROM read_csv('data/delivery-coverage/deliveries.csv', types = {'ags': 'VARCHAR'});

-- Per district: customers, customers per 10,000 inhabitants, deliveries and on time.
CREATE OR REPLACE TABLE dc_cover AS
SELECT d.ags, d.district, d.state, d.population, d.dc_km, k.name AS dc_name,
       coalesce(c.n, 0) AS customers,
       10000.0 * coalesce(c.n, 0) / d.population AS per_10k,
       coalesce(v.deliveries, 0) AS deliveries,
       v.on_time / nullif(v.deliveries, 0) AS on_time
FROM dc_districts d
JOIN dc_dcs k USING (dc)
LEFT JOIN (SELECT ags, count(*) AS n FROM dc_customers GROUP BY ags) c USING (ags)
LEFT JOIN (SELECT ags, sum(deliveries) AS deliveries, sum(on_time) AS on_time FROM dc_deliveries GROUP BY ags) v USING (ags);

-- The distance bands the maps and charts share.
CREATE OR REPLACE MACRO dc_band(km) AS
    CASE WHEN km < 100 THEN '1 · under 100 km' WHEN km < 200 THEN '2 · 100–200 km'
         WHEN km < 300 THEN '3 · 200–300 km' ELSE '4 · over 300 km' END;

-- ── The answer ───────────────────────────────────────────────────────────────
SELECT ('**Answer:** our ' || (SELECT format('{:,}', count(*)) FROM dc_customers) ||
        ' customers cluster around the three DCs — ' ||
        (SELECT round(100 * avg((km < 150)::INTEGER))::INTEGER FROM dc_customers) ||
        ' % of them live within 150 km of theirs (*Customers*). The furthest regions are underserved: **' ||
        (SELECT count(*) FROM dc_cover WHERE customers = 0) || ' districts have no customer at all**, ' ||
        (SELECT round(100 * avg((per_10k < 0.2)::INTEGER))::INTEGER FROM dc_cover WHERE dc_km > 300) ||
        ' % of the districts over 300 km from a DC have fewer than 0.2 customers per 10,000 people, and most of them lie in ' ||
        (SELECT string_agg(state, ' and ' ORDER BY n DESC) FROM (
            SELECT CASE state WHEN 'DE-MV' THEN 'Mecklenburg-Vorpommern' WHEN 'DE-BB' THEN 'Brandenburg'
                              WHEN 'DE-SN' THEN 'Saxony' WHEN 'DE-ST' THEN 'Saxony-Anhalt' WHEN 'DE-TH' THEN 'Thuringia'
                              WHEN 'DE-BY' THEN 'Bavaria' WHEN 'DE-BW' THEN 'Baden-Württemberg' ELSE state END AS state,
                   count(*) AS n
            FROM dc_cover WHERE per_10k < 0.2 GROUP BY 1 ORDER BY n DESC LIMIT 2)) ||
        ' (*Underserved*). Distance costs punctuality too: ' ||
        (SELECT round(100 * sum(on_time * deliveries) / sum(deliveries))::INTEGER FROM dc_cover WHERE dc_km < 100) ||
        ' % of deliveries arrive on time within 100 km, ' ||
        (SELECT round(100 * sum(on_time * deliveries) / sum(deliveries))::INTEGER FROM dc_cover WHERE dc_km >= 300) ||
        ' % beyond 300 km (*On time*).')::MARKDOWN;

-- ── KPIs ─────────────────────────────────────────────────────────────────────
SELECT 'Coverage, last 12 weeks'::GROUP;
SELECT (SELECT count(*) FROM dc_customers) ::METRIC, 'Customers'::LABEL,
       'Active customers: shops, restaurants and canteens with at least one delivery in the last 12 weeks.'::INFO;
SELECT (SELECT sum(deliveries) FROM dc_deliveries) ::COMPACT, 'Deliveries'::LABEL,
       'Drops made in the last 12 weeks, all three DCs.'::INFO;
SELECT (SELECT 100 * sum(on_time) / sum(deliveries) FROM dc_deliveries) ::PERCENT, 'On time'::LABEL,
       '<85:bad; <90:warn; good'::THRESHOLDS,
       'Share of deliveries that arrived inside the customer''s delivery window.'::INFO;
SELECT (SELECT 100 * avg((customers = 0)::INTEGER) FROM dc_cover) ::PERCENT, 'Districts with no customer'::LABEL,
       '>=5:bad; >=2:warn; good'::THRESHOLDS,
       'Share of Germany''s 400 districts (Kreise and kreisfreie Städte) where we have no customer at all.'::INFO;
SELECT 1::ENDGROUP;

-- ── 1 · Coverage ─────────────────────────────────────────────────────────────
SELECT 'Coverage'::TAB;
SELECT 7::COL;
SELECT 560::HEIGHT;
SELECT ags ::REGION,
       round(per_10k, 2) AS "Customers per 10,000 people" ::BARCHART,
       'quantile 5'::CLASSES,
       district || ' (' || dc_name || ' DC, ' || round(dc_km)::INTEGER || ' km)' ::LABEL,
       'Customers per 10,000 people are highest around the DCs'::TITLE,
       'Customers in the district for every 10,000 inhabitants, in five classes of about 80 districts each (the palest includes the districts with none). The road distance is to the district''s nearest DC.'::INFO
FROM dc_cover ORDER BY ags;

SELECT 5::COL;
SELECT 560::HEIGHT;
-- people add up; the distance as a mean; the rate has no total
SELECT 'People: sum; km to DC: mean; Per 10k: none'::TOTALS;
SELECT district AS "District" ::TABLE, round(per_10k, 2) AS "Per 10k",
       round(dc_km)::INTEGER AS "km to DC", population AS "People" ::COMPACT,
       'The 15 least covered districts'::TITLE,
       'Fewest customers per 10,000 inhabitants first (most have none); ties broken by the larger population — more people we don''t reach.'::INFO
FROM dc_cover ORDER BY per_10k, population DESC LIMIT 15;

-- ── 2 · Customers ────────────────────────────────────────────────────────────
SELECT 'Customers'::TAB;
SELECT 8::COL;
SELECT 560::HEIGHT;
SELECT lat ::LAT, lon ::LON, band ::CATEGORY, label ::LABEL, 'de-states' ::BASEMAP, 0.7 ::ALPHA, ''::DATALABELS,
       'Customers cluster within 100 km of a DC'::TITLE,
       'Each dot is a customer, coloured by the road distance to the DC that serves it; the three DCs are labelled.'::INFO
FROM (SELECT lat, lon, dc_band(km) AS band, NULL AS label, 1 AS o FROM dc_customers
      UNION ALL
      SELECT lat, lon, '0 · DC', name || ' DC', 2 FROM dc_dcs)
ORDER BY o, band;

SELECT 4::COL;
SELECT 560::HEIGHT;
SELECT km AS "Road km to the DC" ::HISTOGRAM,
       'Most customers are close to a DC'::TITLE,
       'Customers by road distance to the DC that serves them.'::INFO
FROM dc_customers;

-- ── 3 · On time ──────────────────────────────────────────────────────────────
SELECT 'On time'::TAB;
SELECT 6::COL;
SELECT 520::HEIGHT;
SELECT state ::REGION, round(100 * sum(on_time * deliveries) / sum(deliveries), 1) AS "On time" ::BARCHART,
       ' %'::YFORMAT, '0,80,85,90,100'::CLASSES, ''::DATALABELS,
       'Deliveries far from a DC arrive late more often'::TITLE,
       'Share of deliveries into the state that arrived inside the delivery window, last 12 weeks. Classes: under 80 %, 80–85 %, 85–90 %, 90 % and more.'::INFO
FROM dc_cover WHERE deliveries > 0 GROUP BY state ORDER BY state;

SELECT 6::COL;
SELECT 520::HEIGHT;
SELECT week AS "Week" ::XAXIS, k.name || ' DC' ::CATEGORY,
       round(100.0 * sum(on_time) / sum(deliveries), 1) AS "On time" ::LINECHART, ' %'::YFORMAT,
       90 ::REFLINE, 'target 90 %'::REFLINE_LABEL,
       'Nürnberg stays below the 90 % target every week'::TITLE,
       'Weekly share of on-time deliveries per DC. Nürnberg''s area reaches furthest into the east.'::INFO
FROM dc_deliveries d JOIN dc_dcs k USING (dc) GROUP BY week, k.name ORDER BY k.name, week;

-- ── 4 · Underserved ──────────────────────────────────────────────────────────
SELECT 'Underserved'::TAB;
SELECT 6::COL;
SELECT 520::HEIGHT;
SELECT ags ::REGION, round(dc_km)::INTEGER AS "Road km to the nearest DC" ::BARCHART,
       '0,100,200,300,450'::CLASSES, district ::LABEL,
       'Far from every DC: the north-east and the east'::TITLE,
       'Road distance from the district''s centre to its nearest DC (straight line × 1.3).'::INFO
FROM dc_cover ORDER BY ags;

SELECT 6::COL;
SELECT 520::HEIGHT;
SELECT 50 * floor(dc_km / 50) + 25 AS "Road km to the nearest DC" ::XAXIS,
       round(10000.0 * sum(customers) / sum(population), 2) AS "Customers per 10,000 people" ::LINECHART,
       'Coverage falls off with distance'::TITLE,
       'Customers per 10,000 inhabitants of all districts in each 50-km band of road distance to their nearest DC: how much of the market we reach at each distance.'::INFO
FROM dc_cover GROUP BY 1 ORDER BY 1;
