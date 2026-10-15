-- Locale de-DE: a German branch report. KPIs, tables, chart axes, legends and
-- month names use German conventions (1.234.567,5 · 12.400 € · 12,5 % ·
-- 1,2 Mio. · 15.10.2026 · Okt. 2026), in the default look with an ink-black
-- accent.
--
-- Synthetic, deterministic data: daily sales of a made-up drugstore chain,
-- 12 branches × 4 product groups, Jan 2025 – Sep 2026. The filters (region,
-- product groups) drive every panel; KPIs compare Jan–Sep 2026 with Jan–Sep 2025.

SELECT 'de-DE'::LOCALE;
SELECT 'Filialbericht Januar – September 2026'::DASHBOARD_TITLE;
SELECT 'Drogeriemarkt Nordlicht · Controlling'::BRAND;
SELECT '{"name":"nordlicht","extends":"light","colors":{"accent":"#0a0a0a","accent2":"#0a0a0a","info":"#0a0a0a"},"dark":{"colors":{"accent":"#ededed","accent2":"#ededed","info":"#ededed"}}}'::THEME;  -- the default look with a black accent

-- ── Daten ────────────────────────────────────────────────────────────────────
CREATE OR REPLACE MACRO nl_u(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;

CREATE OR REPLACE TABLE nl_filialen AS SELECT * FROM (VALUES
    ('Hamburg', 'Nord', 820, 1.30), ('Bremen', 'Nord', 540, 0.85), ('Hannover', 'Nord', 610, 0.95),
    ('Berlin', 'Ost', 900, 1.40), ('Leipzig', 'Ost', 560, 0.90), ('Dresden', 'Ost', 520, 0.80),
    ('Köln', 'West', 780, 1.20), ('Düsseldorf', 'West', 700, 1.10), ('Dortmund', 'West', 600, 0.75),
    ('München', 'Süd', 860, 1.45), ('Stuttgart', 'Süd', 650, 1.05), ('Nürnberg', 'Süd', 580, 0.90)
) t(filiale, region, flaeche_m2, groesse);

CREATE OR REPLACE TABLE nl_umsatz AS
WITH d AS (SELECT (DATE '2025-01-01' + INTERVAL (i) DAY)::DATE AS tag, i FROM range(0, 638) t(i)),
w(warengruppe, basis, bon, marge, saison) AS (VALUES
    ('Drogerie', 4200, 14.5, 0.31, 0.10), ('Lebensmittel', 2600, 9.8, 0.19, 0.05),
    ('Haushalt', 1500, 17.0, 0.36, 0.20), ('Saison & Geschenke', 700, 22.0, 0.42, 1.6))
SELECT d.tag, f.filiale, f.region, w.warengruppe,
       round(w.basis * f.groesse
             * (1 + 0.04 * d.i / 365.0)                                                    -- leichtes Wachstum
             * (1 + w.saison * exp(-power((dayofyear(d.tag) - 350) / 18.0, 2)))           -- Weihnachten
             * CASE dayofweek(d.tag) WHEN 0 THEN 0 WHEN 6 THEN 1.35 WHEN 5 THEN 1.15 ELSE 0.95 END  -- sonntags zu
             * CASE WHEN f.filiale = 'Leipzig' AND d.tag >= DATE '2026-04-01' THEN 1.28 ELSE 1 END  -- Umbau fertig
             * CASE WHEN f.filiale = 'Dortmund' AND d.tag >= DATE '2026-02-01' THEN 0.86 ELSE 1 END -- neuer Wettbewerber
             * (0.85 + 0.3 * nl_u(d.i, f.filiale, w.warengruppe)), 2) AS umsatz,
       w.bon * (1 + 0.025 * d.i / 365.0) AS bon, w.marge + 0.02 * (nl_u(d.i, f.filiale, 'm') - 0.5) AS marge
FROM d, nl_filialen f, w;

CREATE OR REPLACE MACRO nl_in(rg, wg) AS
    getvariable('region') IN ('Alle Regionen', rg) AND list_contains(getvariable('warengruppe'), wg);
-- Vergleichszeitraum: Jan–Sep 2026 gegen Jan–Sep 2025
CREATE OR REPLACE MACRO nl_akt(t) AS t >= DATE '2026-01-01';
CREATE OR REPLACE MACRO nl_vj(t) AS t < DATE '2025-10-01';

-- ── Filter ───────────────────────────────────────────────────────────────────
SELECT 'Auswahl'::GROUP;
SELECT region ::DROPDOWN, 'Region'::LABEL FROM (SELECT 0 AS k, 'Alle Regionen' AS region UNION ALL SELECT DISTINCT 1, region FROM nl_filialen) ORDER BY k, region;
SELECT DISTINCT warengruppe ::MULTISELECT, 'Warengruppen'::LABEL FROM nl_umsatz ORDER BY warengruppe;
SELECT 1::ENDGROUP;

-- ── Kennzahlen ───────────────────────────────────────────────────────────────
SELECT 'Januar – September, gegenüber Vorjahr'::GROUP;
SELECT sum(umsatz) FILTER (WHERE nl_akt(tag))::MONEY, sum(umsatz) FILTER (WHERE nl_vj(tag))::DELTA, 'Umsatz'::LABEL
FROM nl_umsatz WHERE nl_in(region, warengruppe);
SELECT round(sum(umsatz / bon) FILTER (WHERE nl_akt(tag)))::COMPACT, sum(umsatz / bon) FILTER (WHERE nl_vj(tag))::DELTA, 'Kassenbons'::LABEL
FROM nl_umsatz WHERE nl_in(region, warengruppe);
SELECT round(sum(umsatz) FILTER (WHERE nl_akt(tag)) / sum(umsatz / bon) FILTER (WHERE nl_akt(tag)), 2)::METRIC,
       (sum(umsatz) FILTER (WHERE nl_vj(tag)) / sum(umsatz / bon) FILTER (WHERE nl_vj(tag)))::DELTA, 'Ø Bon in €'::LABEL
FROM nl_umsatz WHERE nl_in(region, warengruppe);
SELECT (100 * sum(umsatz * marge) FILTER (WHERE nl_akt(tag)) / sum(umsatz) FILTER (WHERE nl_akt(tag)))::PERCENT,
       (100 * sum(umsatz * marge) FILTER (WHERE nl_vj(tag)) / sum(umsatz) FILTER (WHERE nl_vj(tag)))::DELTA, 'Rohertragsquote'::LABEL
FROM nl_umsatz WHERE nl_in(region, warengruppe);
SELECT (sum(umsatz) FILTER (WHERE nl_akt(tag)) / (SELECT sum(flaeche_m2) FROM nl_filialen WHERE getvariable('region') IN ('Alle Regionen', region)))::MONEY,
       (sum(umsatz) FILTER (WHERE nl_vj(tag)) / (SELECT sum(flaeche_m2) FROM nl_filialen WHERE getvariable('region') IN ('Alle Regionen', region)))::DELTA,
       'Umsatz je m²'::LABEL
FROM nl_umsatz WHERE nl_in(region, warengruppe);
SELECT 1::ENDGROUP;

-- ── 1 · Überblick ────────────────────────────────────────────────────────────
SELECT 'Überblick'::TAB;

SELECT 8::COL;
SELECT 340::HEIGHT;
SELECT make_date(2026, month(tag), 1) ::XAXIS, CASE WHEN year(tag) = 2026 THEN '2026' ELSE 'Vorjahr 2025' END ::CATEGORY,
       sum(umsatz) / 1000 ::LINECHART, '€'::YFORMAT, 'Monatsumsatz in Tsd. € — 2026 liegt jeden Monat über dem Vorjahr'::TITLE
FROM nl_umsatz WHERE nl_in(region, warengruppe) AND month(tag) <= 9 GROUP BY ALL ORDER BY 1, 2;

SELECT 4::COL;
SELECT 340::HEIGHT;
SELECT warengruppe ::CATEGORY, sum(umsatz) ::DONUTCHART, 'Umsatzanteil je Warengruppe'::TITLE
FROM nl_umsatz WHERE nl_in(region, warengruppe) AND nl_akt(tag) GROUP BY ALL ORDER BY 2 DESC;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT monat ::XAXIS, wochentag ::YAXIS, round(avg(ums) / 1000) ::HEATMAP,
       'Ø Tagesumsatz in Tsd. € nach Wochentag und Monat — samstags der stärkste Tag, sonntags geschlossen'::TITLE
FROM (SELECT tag, sum(umsatz) AS ums,
             ['Jan.','Feb.','März','Apr.','Mai','Juni','Juli','Aug.','Sep.'][month(tag)] AS monat,
             ['Mo','Di','Mi','Do','Fr','Sa','So'][isodow(tag)] AS wochentag,
             month(tag) AS m, isodow(tag) AS wd
      FROM nl_umsatz WHERE nl_in(region, warengruppe) AND nl_akt(tag) AND isodow(tag) < 7 GROUP BY tag)
GROUP BY monat, wochentag ORDER BY any_value(m), any_value(wd);

-- ── 2 · Filialen ─────────────────────────────────────────────────────────────
SELECT 'Filialen'::TAB;

SELECT 5::COL;
SELECT 600::HEIGHT;
SELECT f.filiale ::XAXIS, sum(u.umsatz) / any_value(f.flaeche_m2) ::BARCHART, 1 ::FLIP, '€'::YFORMAT,
       (SELECT sum(umsatz) FROM nl_umsatz WHERE nl_akt(tag) AND list_contains(getvariable('warengruppe'), warengruppe))
         / (SELECT sum(flaeche_m2) FROM nl_filialen) ::REFLINE,
       'Umsatz je m² — gestrichelt: Schnitt der Kette'::TITLE
FROM nl_umsatz u JOIN nl_filialen f USING (filiale, region)
WHERE nl_in(u.region, u.warengruppe) AND nl_akt(u.tag) GROUP BY ALL ORDER BY 2;

SELECT 7::COL;
SELECT 'Filialen im Vergleich zum Vorjahr'::TITLE,
       filiale AS "Filiale" ::TABLE, region AS "Region",
       list(ums / tage ORDER BY monat) FILTER (WHERE monat >= DATE '2026-01-01') AS "Ø Tagesumsatz 2026" ::SPARKLINE,
       sum(ums) FILTER (WHERE monat >= DATE '2026-01-01') AS "Umsatz" ::MONEY,
       round(100 * (sum(ums) FILTER (WHERE monat >= DATE '2026-01-01') / sum(ums) FILTER (WHERE monat < DATE '2025-10-01') - 1), 1) AS "ggü. Vorjahr %" ::TREND,
       CASE WHEN sum(ums) FILTER (WHERE monat >= DATE '2026-01-01') / sum(ums) FILTER (WHERE monat < DATE '2025-10-01') >= 1.04 THEN 'Ziel erreicht'
            WHEN sum(ums) FILTER (WHERE monat >= DATE '2026-01-01') / sum(ums) FILTER (WHERE monat < DATE '2025-10-01') >= 1.0 THEN 'knapp'
            ELSE 'verfehlt' END AS "Ziel +4 %" ::BADGE
FROM (SELECT filiale, region, date_trunc('month', tag)::DATE AS monat, sum(umsatz) AS ums,
             count(DISTINCT tag) FILTER (WHERE umsatz > 0) AS tage
      FROM nl_umsatz WHERE nl_in(region, warengruppe) GROUP BY ALL)
GROUP BY filiale, region ORDER BY "Umsatz" DESC;

SELECT 12::COL;
SELECT 300::HEIGHT;
SELECT make_date(2026, month(tag), 1) ::XAXIS, filiale ::CATEGORY,
       round(100 * (sum(umsatz) FILTER (WHERE year(tag) = 2026) / sum(umsatz) FILTER (WHERE year(tag) = 2025) - 1), 1) ::LINECHART, 'percent'::YFORMAT,
       0 ::REFLINE, 'Wachstum ggü. Vorjahresmonat — Leipzig nach dem Umbau, Dortmund seit dem neuen Wettbewerber'::TITLE
FROM nl_umsatz WHERE filiale IN ('Leipzig', 'Dortmund') AND month(tag) <= 9
  AND list_contains(getvariable('warengruppe'), warengruppe)
GROUP BY ALL ORDER BY 1, 2;

-- ── 3 · Zahlenformate ────────────────────────────────────────────────────────
SELECT 'Zahlenformate'::TAB;

SELECT 'So formatiert de-DE'::GROUP;
SELECT 1234567.5::METRIC, '::METRIC'::LABEL;
SELECT 12400::MONEY, '::MONEY'::LABEL;
SELECT 12.5::PERCENT, '::PERCENT'::LABEL;
SELECT 1234567::COMPACT, '::COMPACT'::LABEL;
SELECT 1::ENDGROUP;

SELECT 6::COL;
SELECT '**Eine Zeile genügt:** `SELECT ''de-DE''::LOCALE;` stellt KPIs, Tabellenzellen (Zahlen und ISO-Daten) und Diagrammachsen samt Monatsnamen auf deutsche Konventionen um — im Browser, im CLI und in jedem Export. Ein Betrachter kann es mit `?locale=en-US` für sich überschreiben (nicht im gesperrten Server-Modus).'::MARKDOWN, 'Was ::LOCALE umstellt'::TITLE;

SELECT 6::COL;
SELECT 'Beispielwerte'::TITLE, art AS "Format" ::TABLE, wert AS "Wert" ::PLAIN, betrag AS "Betrag" ::MONEY, anteil AS "Anteil" ::PERCENT, datum AS "Datum"
FROM (VALUES (1, 'Kleinbetrag', 3.5, 3.5, 0.5, DATE '2026-01-02'),
             (2, 'Tausender', 12345.67, 12345.67, 12.5, DATE '2026-06-15'),
             (3, 'Millionen', 1234567.89, 1234567.89, 99.9, DATE '2026-10-15')) t(k, art, wert, betrag, anteil, datum)
ORDER BY k;
