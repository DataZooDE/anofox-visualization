-- How many duplicate customers do we have, and which should we merge? —
-- duplicate / entity matching with anofox_tabular.
--
-- STORED RESULTS, NOT LIVE. The normalised match keys, key checks, scored
-- candidate pairs and match clusters below were computed natively with the
-- anofox_tabular extension (examples/tabular-dedup/compute.sql, written to
-- web/data/tabular-dedup/*.csv by scripts/gen-tabular-cases.sh dedup).
-- anofox_tabular has no DuckDB-Wasm build, so the browser demo only reads,
-- filters and plots the stored tables. The inputs below re-slice stored
-- results (the threshold re-cuts the stored pair scores); they never re-run
-- the extension.
--
-- A made-up garden-supply retailer, Grünwerk, brings 2,502 customer records
-- from its webshop, ERP and loyalty cards into one CRM (synthetic and
-- deterministic: 1,594 real customers, of them 94 who share a household with
-- another customer).
--
-- Tabs: Overview · Match quality · Clusters · Review list.
-- Natively, run from web/:  dashboard --check ../examples/tabular-dedup/showcase.sql

SELECT 'anofox_tabular'::BRAND;
SELECT 'How many duplicate customers do we have, and which should we merge?'::DASHBOARD_TITLE;

-- ── Data: the stored anofox_tabular results ──────────────────────────────────
CREATE OR REPLACE TABLE dd_records AS FROM read_csv('data/tabular-dedup/dd_records.csv', types = {'postcode': 'VARCHAR'});
CREATE OR REPLACE TABLE dd_pairs AS FROM read_csv('data/tabular-dedup/dd_pairs.csv');
CREATE OR REPLACE TABLE dd_clusters AS FROM read_csv('data/tabular-dedup/dd_clusters.csv');
CREATE OR REPLACE TABLE dd_key_checks AS FROM read_csv('data/tabular-dedup/dd_key_checks.csv');
CREATE OR REPLACE TABLE dd_key_quality AS FROM read_csv('data/tabular-dedup/dd_key_quality.csv');
CREATE OR REPLACE TABLE dd_provenance AS FROM read_csv('data/tabular-dedup/provenance.csv');

-- Inputs. Every macro tolerates an unset variable (the native checker runs
-- the panels without the browser's controls).
CREATE OR REPLACE MACRO dd_t() AS coalesce(getvariable('threshold'), 85) / 100.0;
CREATE OR REPLACE MACRO dd_auto() AS greatest(coalesce(getvariable('auto_from'), 95) / 100.0, dd_t());
-- the clusters are stored per 5-point step: the nearest one
CREATE OR REPLACE MACRO dd_step() AS least(95, greatest(50, round(dd_t() * 20) * 5))::INTEGER;
CREATE OR REPLACE MACRO dd_sys(a, b) AS
    coalesce(len(getvariable('systems')) = 0 OR (list_contains(getvariable('systems'), a) AND list_contains(getvariable('systems'), b)), true);
CREATE OR REPLACE MACRO dd_sysname(s) AS CASE s WHEN 'webshop' THEN 'Webshop' WHEN 'erp' THEN 'ERP' ELSE 'Loyalty' END;

-- the true duplicate pairs (records of one customer), for the recall
CREATE OR REPLACE TABLE dd_true_pairs AS
SELECT sum(n * (n - 1) / 2)::BIGINT AS n FROM (SELECT count(*) AS n FROM dd_records GROUP BY entity_id);
-- precision and recall of every threshold from 50 to 99 %, and of a plain
-- GROUP BY email (exact_email) for comparison
CREATE OR REPLACE TABLE dd_curve AS
SELECT t AS threshold,
       count(*) FILTER (WHERE 100 * score >= t AND truth = 'same customer') / nullif(count(*) FILTER (WHERE 100 * score >= t), 0) AS precision,
       count(*) FILTER (WHERE 100 * score >= t AND truth = 'same customer') / (SELECT n FROM dd_true_pairs) AS recall,
       count(*) FILTER (WHERE 100 * score >= t AND truth = 'same household') AS household_pairs
FROM dd_pairs, range(50, 100) r(t) GROUP BY t;

-- ── Header: the question, inputs, KPIs ───────────────────────────────────────
SELECT 12::COL;
SELECT ('**Who asks:** Grünwerk''s CRM owner, before the webshop, ERP and loyalty-card customers move into one CRM. Every duplicate is a double mailing and a split purchase history; every wrong merge mixes two people''s orders and addresses. The decision is the **match threshold** — and which pairs a steward checks by hand.')::MARKDOWN;

SELECT 'Inputs'::GROUP;
SELECT 85 AS threshold ::NUMBER, 'Match threshold (%)' ::LABEL;
SELECT 95 AS auto_from ::NUMBER, 'Merge without review from (%)' ::LABEL;
SELECT dd_sysname(source) AS systems ::MULTISELECT, 'Systems' ::LABEL
FROM (VALUES ('webshop'), ('erp'), ('loyalty')) t(source);
SELECT 1::ENDGROUP;

SELECT 'At the chosen threshold'::GROUP;
SELECT (SELECT count(*) FROM dd_records) - (SELECT count(*) - count(DISTINCT cluster_id) FROM dd_clusters WHERE threshold = dd_step()) ::COMPACT,
       (SELECT count(*) FROM dd_records) ::DELTA,
       'customers after merging, vs. 2,502 records' ::LABEL;
SELECT 100 * count(*) FILTER (WHERE truth = 'same customer') / nullif(count(*), 0) ::PERCENT,
       (SELECT 100 * count(*) FILTER (WHERE truth = 'same customer') / count(*) FROM dd_pairs WHERE exact_email) ::DELTA,
       'precision: matched pairs that are one customer, vs. GROUP BY email' ::LABEL
FROM dd_pairs WHERE score >= dd_t();
SELECT 100 * count(*) FILTER (WHERE truth = 'same customer') / (SELECT n FROM dd_true_pairs) ::PERCENT,
       (SELECT 100 * count(*) FILTER (WHERE truth = 'same customer') / (SELECT n FROM dd_true_pairs) FROM dd_pairs WHERE exact_email) ::DELTA,
       'recall: duplicate pairs found, vs. GROUP BY email' ::LABEL
FROM dd_pairs WHERE score >= dd_t();
SELECT count(*) FILTER (WHERE score >= dd_auto()) ::METRIC, 'pairs merged without review' ::LABEL
FROM dd_pairs WHERE score >= dd_t() AND dd_sys(dd_sysname(a_source), dd_sysname(b_source));
SELECT count(*) FILTER (WHERE score < dd_auto()) ::METRIC, 'pairs for a steward to review' ::LABEL
FROM dd_pairs WHERE score >= dd_t() AND dd_sys(dd_sysname(a_source), dd_sysname(b_source));
SELECT count(*) ::METRIC, 'household pairs above the threshold (must not merge)' ::LABEL
FROM dd_pairs WHERE score >= dd_t() AND truth = 'same household' AND dd_sys(dd_sysname(a_source), dd_sysname(b_source));
SELECT 1::ENDGROUP;

-- ── 1 · Overview: how many duplicates, and where ─────────────────────────────
SELECT 'Overview'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** true duplicates score above 90 %, different customers mostly below 70 % — but people of one **household** (same surname, address and landline, different person) sit right in between. Every threshold trades duplicates left in against households wrongly merged. Move **Match threshold** to see the trade.'::MARKDOWN;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT floor(100 * score / 5) * 5 ::XAXIS, truth ::CATEGORY, count(*) ::BARCHART,
       100 * dd_t() ::XLINE, 'log10'::YSCALE,
       'Candidate pairs by score (log count) — households fill the gap between the two humps'::TITLE
FROM dd_pairs WHERE dd_sys(dd_sysname(a_source), dd_sysname(b_source))
GROUP BY ALL ORDER BY 1, min(CASE truth WHEN 'same customer' THEN 1 WHEN 'same household' THEN 2 ELSE 3 END);

SELECT 6::COL;
SELECT 320::HEIGHT;
WITH p AS (
    SELECT dd_sysname(least(a_source, b_source)) || ' × ' || dd_sysname(greatest(a_source, b_source)) AS systems,
           CASE WHEN truth = 'same customer' THEN 'one customer' ELSE 'wrong match' END AS kind, count(*) AS n
    FROM dd_pairs WHERE score >= dd_t() AND dd_sys(dd_sysname(a_source), dd_sysname(b_source)) GROUP BY ALL
)
SELECT systems ::XAXIS, kind ::CATEGORY, n ::BARCHART_STACKED, 1 ::FLIP,
       'Matched pairs by system — most duplicates span webshop and loyalty cards'::TITLE
FROM p ORDER BY sum(n) OVER (PARTITION BY systems) DESC, kind;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT CASE WHEN n >= 5 THEN '5+' ELSE n::VARCHAR END || ' records' ::XAXIS, count(*) ::BARCHART,
       'Merged customers by number of records — mostly pairs'::TITLE
FROM (SELECT cluster_id, count(*) AS n FROM dd_clusters WHERE threshold = dd_step() GROUP BY ALL)
GROUP BY ALL ORDER BY 1;

-- ── 2 · Match quality: choosing the threshold ────────────────────────────────
SELECT 'Match quality'::TAB;

SELECT 12::COL;
SELECT '**What to look for:** precision climbs and recall falls as the threshold rises; between **84 and 88 %** both stay above 90 %. A plain `GROUP BY email` is precise but finds only a fifth of the duplicates — the keys differ between the systems until anofox_tabular normalises them (`phonenumber_format` to E.164, `vat_normalize`, `email_is_valid`).'::MARKDOWN;

SELECT 12::COL;
SELECT 340::HEIGHT;
SELECT threshold ::XAXIS, metric ::CATEGORY, value ::LINECHART, 100 * dd_t() ::XLINE, 'percent'::YFORMAT,
       'Precision and recall by threshold — the chosen threshold is the dashed line'::TITLE
FROM (SELECT threshold, 'precision' AS metric, 100 * precision AS value FROM dd_curve
      UNION ALL SELECT threshold, 'recall', 100 * recall FROM dd_curve)
ORDER BY metric, threshold;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT match_key ::XAXIS, form ::CATEGORY, duplicates ::BARCHART,
       'Exact duplicates per key — normalised keys find far more'::TITLE
FROM dd_key_checks ORDER BY max(duplicates) OVER (PARTITION BY match_key) DESC, form;

SELECT 6::COL;
SELECT 320::HEIGHT;
SELECT match_key ::XAXIS, dd_sysname(source) ::YAXIS, round(100 * filled, 1) ::HEATMAP, 'percent'::YFORMAT, ''::DATALABELS,
       'Keys filled per system (%) — loyalty cards rarely carry an email'::TITLE
FROM dd_key_quality ORDER BY CASE source WHEN 'webshop' THEN 1 WHEN 'erp' THEN 2 ELSE 3 END, match_key;

SELECT 12::COL;
SELECT 300::HEIGHT;
WITH p AS (
    SELECT evidence, truth, count(*) AS n FROM dd_pairs
    WHERE score >= dd_t() AND dd_sys(dd_sysname(a_source), dd_sysname(b_source)) GROUP BY ALL
)
SELECT evidence ::XAXIS, truth ::CATEGORY, n ::BARCHART_STACKED, 1 ::FLIP,
       'Pairs above the threshold by their strongest evidence — wrong matches share only a phone or an address'::TITLE
FROM p ORDER BY sum(n) OVER (PARTITION BY evidence) DESC, CASE truth WHEN 'same customer' THEN 1 WHEN 'same household' THEN 2 ELSE 3 END;

-- ── 3 · Clusters: the customers after the merge ──────────────────────────────
SELECT 'Clusters'::TAB;

SELECT 12::COL;
SELECT ('**What to look for:** at a low threshold, pairs chain into clusters that join several people — the biggest cluster at the chosen step ('
        || dd_step() || ' %) holds **' || (SELECT max(n) FROM (SELECT count(*) AS n FROM dd_clusters WHERE threshold = dd_step() GROUP BY cluster_id))
        || ' records**. Below, the largest clusters record by record; ⚠ marks a cluster that mixes several real customers.')::MARKDOWN;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT threshold ::XAXIS, max(n) ::BARCHART, dd_step() ::XLINE,
       'Largest cluster by threshold step — below 75 % pairs chain into blobs'::TITLE
FROM (SELECT threshold, cluster_id, count(*) AS n FROM dd_clusters GROUP BY ALL)
GROUP BY ALL ORDER BY 1;

SELECT 6::COL;
SELECT 300::HEIGHT;
SELECT threshold ::XAXIS,
       (SELECT count(*) FROM dd_records) - (count(*) - count(DISTINCT cluster_id)) ::LINECHART, dd_step() ::XLINE,
       (SELECT count(DISTINCT entity_id) FROM dd_records) ::REFLINE,
       'Customers after merging by threshold step — the true 1,594 is the dashed line'::TITLE
FROM dd_clusters GROUP BY ALL ORDER BY 1;

SELECT 12::COL;
SELECT 'Cluster'::SUBTOTAL;
SELECT 'Customers > 1'::FLAG;
SELECT 'count'::TOTALS;
WITH c AS (
    SELECT c.cluster_id, c.record_id, count(*) OVER (PARTITION BY c.cluster_id) AS n,
           count(DISTINCT r.entity_id) OVER (PARTITION BY c.cluster_id) AS customers
    FROM dd_clusters c JOIN dd_records r USING (record_id) WHERE c.threshold = dd_step()
), top AS (
    SELECT DISTINCT cluster_id, n FROM c ORDER BY n DESC, cluster_id LIMIT 12
)
SELECT 'The 12 largest clusters at the chosen step, record by record'::TITLE,
       'Cluster ' || c.cluster_id AS "Cluster" ::TABLE, r.record_id AS "Record", dd_sysname(r.source) AS "System",
       r.name AS "Name", r.street AS "Street", r.postcode || ' ' || r.city AS "Place", r.phone AS "Phone", r.email AS "Email",
       c.customers AS "Customers" ::PLAIN
FROM c JOIN top USING (cluster_id) JOIN dd_records r USING (record_id)
ORDER BY top.n DESC, c.cluster_id, r.record_id;

-- ── 4 · Review list: the decision ────────────────────────────────────────────
SELECT 'Review list'::TAB;

SELECT 12::COL;
SELECT ('### ' || (SELECT count(*) FROM dd_pairs WHERE score >= dd_auto()) || ' pairs merge automatically, '
        || (SELECT count(*) FROM dd_pairs WHERE score >= dd_t() AND score < dd_auto()) || ' go to a steward. '
        || 'Between the threshold (' || round(100 * dd_t()) || ' %) and auto-merge (' || round(100 * dd_auto()) || ' %), a person decides — '
        || (SELECT count(*) FROM dd_pairs WHERE score >= dd_t() AND score < dd_auto() AND truth <> 'same customer')
        || ' of these are in fact different people. The list starts with the most doubtful pair.')::MARKDOWN;

SELECT 12::COL;
SELECT 'Label = same household, Label = different customers'::FLAG;
SELECT 'Score: bottom 5'::HIGHLIGHT;
SELECT 'none'::TOTALS;
SELECT 'Pairs for review (threshold ≤ score < auto-merge), the most doubtful first'::TITLE,
       p.a_id || ' + ' || p.b_id AS "Pair" ::TABLE,
       round(100 * p.score, 1) AS "Score" ::PLAIN,
       a.name AS "Name A", b.name AS "Name B",
       CASE WHEN a.street = b.street THEN a.street ELSE a.street || ' / ' || b.street END AS "Street",
       dd_sysname(p.a_source) || ' × ' || dd_sysname(p.b_source) AS "Systems",
       p.evidence AS "Evidence",
       p.truth AS "Label" ::BADGE
FROM dd_pairs p JOIN dd_records a ON a.record_id = p.a_id JOIN dd_records b ON b.record_id = p.b_id
WHERE p.score >= dd_t() AND p.score < dd_auto() AND dd_sys(dd_sysname(p.a_source), dd_sysname(p.b_source))
ORDER BY p.score, p.a_id
LIMIT 150;
