-- A German-locale dashboard: KPIs, tables and chart axes use de-DE number and
-- date conventions (1.234.567,5 · 12.400 € · 12,5 % · 1,2 Mio. · 15.10.2026).
SELECT 'de-DE'::LOCALE;
SELECT 'Umsatzübersicht'::DASHBOARD_TITLE;

CREATE OR REPLACE TABLE umsatz AS
SELECT DATE '2026-01-01' + INTERVAL (i) MONTH AS monat,
       round(1000000 + 250000 * sin(i / 2.0) + 40000 * i, 2) AS betrag
FROM range(12) r(i);

SELECT 'Kennzahlen'::GROUP;
SELECT 1234567.5::METRIC, 'Summe'::LABEL;
SELECT 12400::MONEY, 'Umsatz'::LABEL;
SELECT 53800000::MONEY, 'Umsatz (Jahr)'::LABEL;
SELECT 12.5::PERCENT, 'Marge'::LABEL;
SELECT 1234567::COMPACT, 'Besuche'::LABEL;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT monat::XAXIS, betrag::LINECHART, '€'::YFORMAT, 'Umsatz je Monat'::TITLE FROM umsatz ORDER BY monat;
SELECT 6::COL;
SELECT monat::XAXIS, betrag / 1000::AREACHART, 'Umsatz (Tsd.)'::TITLE FROM umsatz ORDER BY monat;

SELECT 'Tabelle'::TITLE, monat::TABLE, betrag, betrag AS "in Euro" ::MONEY FROM umsatz ORDER BY monat LIMIT 6;
