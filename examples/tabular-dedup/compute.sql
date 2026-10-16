-- "How many duplicate customers do we have, and which should we merge?" —
-- the native computation behind the stored results.
--
-- Runs natively only: anofox_tabular has no DuckDB-Wasm build yet, so the
-- browser demo reads the tables this script produces from
-- web/data/tabular-dedup/ (regenerate: scripts/gen-tabular-cases.sh dedup).
-- Run from the repo root:
--   duckdb -unsigned -c "INSTALL anofox_tabular FROM 'https://get.erpl.io'"   # once
--   duckdb -unsigned -c ".read examples/tabular-dedup/compute.sql"
LOAD anofox_tabular;
.read examples/tabular-dedup/data.sql

-- ── 1. Normalise the match keys ────────────────────────────────────────────
-- anofox_tabular does the parsing: phone numbers to E.164
-- (phonenumber_format), VAT IDs to their canonical form (vat_normalize) and
-- only valid ones kept (vat_is_valid), emails checked (email_is_valid).
-- Names and streets are folded in SQL (case, umlauts, "Str.").
CREATE OR REPLACE TABLE dd_records AS
SELECT record_id, source, entity_id,
       first_name || ' ' || last_name AS name, company, street, postcode, city, phone, email, vat_id,
       CASE WHEN email_is_valid(email) THEN lower(trim(email)) END AS email_key,
       phonenumber_format(phone, 'DE', 'E164') AS phone_key,
       CASE WHEN vat_is_valid(vat_normalize(vat_id)) THEN vat_normalize(vat_id) END AS vat_key,
       dd_translit(lower(first_name)) || ' ' || dd_translit(lower(last_name)) AS name_key,
       dd_translit(lower(replace(street, 'str.', 'straße'))) AS street_key
FROM dd_records_raw;

-- ── 2. What exact-key matching sees, before and after normalising ──────────
-- duplicate_count on the raw columns and on the normalised keys: the same
-- check, run as one run_checks suite.
CREATE OR REPLACE TABLE dd_key_suite (
    check_name VARCHAR, check_type VARCHAR, table_name VARCHAR, column_name VARCHAR, params VARCHAR,
    lower_threshold DOUBLE, upper_threshold DOUBLE, monitor_only BOOLEAN, identifier_column VARCHAR, filter_expr VARCHAR);
INSERT INTO dd_key_suite VALUES
    ('Email|as entered',   'duplicate_count', 'dd_records', 'email',     NULL, NULL, 0, true, NULL, 'email IS NOT NULL'),
    ('Email|normalised',   'duplicate_count', 'dd_records', 'email_key', NULL, NULL, 0, true, NULL, 'email_key IS NOT NULL'),
    ('Phone|as entered',   'duplicate_count', 'dd_records', 'phone',     NULL, NULL, 0, true, NULL, 'phone IS NOT NULL'),
    ('Phone|normalised',   'duplicate_count', 'dd_records', 'phone_key', NULL, NULL, 0, true, NULL, 'phone_key IS NOT NULL'),
    ('VAT ID|as entered',  'duplicate_count', 'dd_records', 'vat_id',    NULL, NULL, 0, true, NULL, 'vat_id IS NOT NULL'),
    ('VAT ID|normalised',  'duplicate_count', 'dd_records', 'vat_key',   NULL, NULL, 0, true, NULL, 'vat_key IS NOT NULL'),
    ('Name|as entered',    'duplicate_count', 'dd_records', 'name',      NULL, NULL, 0, true, NULL, NULL),
    ('Name|normalised',    'duplicate_count', 'dd_records', 'name_key',  NULL, NULL, 0, true, NULL, NULL);
CREATE OR REPLACE TABLE dd_key_checks AS
SELECT split_part(check_name, '|', 1) AS match_key, split_part(check_name, '|', 2) AS form, value::BIGINT AS duplicates
FROM run_checks('dd_key_suite');

-- How well each system fills and formats the keys.
CREATE OR REPLACE TABLE dd_key_quality AS
SELECT source, 'Email' AS match_key, round(avg((email IS NOT NULL)::INTEGER), 4) AS filled,
       round(avg(email_is_valid(email)::INTEGER) FILTER (WHERE email IS NOT NULL), 4) AS valid
FROM dd_records GROUP BY source
UNION ALL
SELECT source, 'Phone', round(avg((phone IS NOT NULL)::INTEGER), 4),
       round(avg(phonenumber_is_valid(phone, 'DE')::INTEGER) FILTER (WHERE phone IS NOT NULL), 4)
FROM dd_records GROUP BY source
UNION ALL
SELECT source, 'VAT ID', round(avg((vat_id IS NOT NULL)::INTEGER), 4),
       round(avg(vat_is_valid(vat_normalize(vat_id))::INTEGER) FILTER (WHERE vat_id IS NOT NULL), 4)
FROM dd_records GROUP BY source;

-- ── 3. Candidate pairs and their similarity score ──────────────────────────
-- Blocking: only records that share a key (email, phone, VAT ID), the full
-- name and the city, or the postcode and the first three letters of the
-- surname are compared. The score is a weighted mean of the field similarities over
-- the fields both records have (Jaro-Winkler for names and streets).
CREATE OR REPLACE TABLE dd_pairs AS
WITH cand AS (
    SELECT a.record_id AS a_id, b.record_id AS b_id
    FROM dd_records a JOIN dd_records b ON a.record_id < b.record_id
     AND (a.email_key = b.email_key OR a.phone_key = b.phone_key OR a.vat_key = b.vat_key
          OR (a.name_key = b.name_key AND a.city = b.city)
          OR (a.postcode = b.postcode AND left(split_part(a.name_key, ' ', 2), 3) = left(split_part(b.name_key, ' ', 2), 3)))
),
f AS (
    SELECT c.a_id, c.b_id, a.source AS a_source, b.source AS b_source,
           a.entity_id = b.entity_id AS same_customer, coalesce(a.email = b.email, false) AS exact_email,
           coalesce(ea.household_of = b.entity_id OR eb.household_of = a.entity_id, false) AS same_household,
           jaro_winkler_similarity(a.name_key, b.name_key) AS name_sim,
           jaro_winkler_similarity(a.street_key, b.street_key) AS street_sim,
           (a.postcode = b.postcode)::INTEGER AS postcode_eq,
           CASE WHEN a.email_key IS NOT NULL AND b.email_key IS NOT NULL THEN (a.email_key = b.email_key)::INTEGER END AS email_eq,
           CASE WHEN a.phone_key IS NOT NULL AND b.phone_key IS NOT NULL THEN (a.phone_key = b.phone_key)::INTEGER END AS phone_eq,
           CASE WHEN a.vat_key IS NOT NULL AND b.vat_key IS NOT NULL THEN (a.vat_key = b.vat_key)::INTEGER END AS vat_eq
    FROM cand c JOIN dd_records a ON a.record_id = c.a_id JOIN dd_records b ON b.record_id = c.b_id
    JOIN dd_entities ea ON ea.entity_id = a.entity_id JOIN dd_entities eb ON eb.entity_id = b.entity_id
)
SELECT a_id, b_id, a_source, b_source,
       CASE WHEN same_customer THEN 'same customer' WHEN same_household THEN 'same household' ELSE 'different customers' END AS truth,
       exact_email, round(name_sim, 3) AS name_sim, round(street_sim, 3) AS street_sim, postcode_eq, email_eq, phone_eq, vat_eq,
       round((0.40 * name_sim + 0.15 * street_sim + 0.10 * postcode_eq
              + 0.15 * coalesce(email_eq, 0) + 0.12 * coalesce(phone_eq, 0) + 0.20 * coalesce(vat_eq, 0))
             / (0.65 + 0.15 * (email_eq IS NOT NULL)::INTEGER + 0.12 * (phone_eq IS NOT NULL)::INTEGER
                     + 0.20 * (vat_eq IS NOT NULL)::INTEGER), 4) AS score,
       CASE WHEN vat_eq = 1 THEN 'same VAT ID' WHEN email_eq = 1 THEN 'same email' WHEN phone_eq = 1 THEN 'same phone'
            ELSE 'name and address only' END AS evidence
FROM f;

-- (truth: the synthetic customer behind each record — in a real project, the
-- labels of a steward-reviewed sample. exact_email: the pair a plain
-- GROUP BY email would find.)

-- ── 4. Match clusters at every threshold (connected components) ────────────
-- Records linked by a pair at or above the threshold end up in one cluster:
-- the customer after the merge. Stored for thresholds 50, 55, …, 95 %;
-- singletons are left out (every record not listed is its own customer).
CREATE OR REPLACE TABLE dd_clusters AS
WITH RECURSIVE th AS (SELECT unnest(range(50, 100, 5)) AS threshold),
e AS (
    SELECT threshold, a_id AS x, b_id AS y FROM dd_pairs, th WHERE 100 * score >= threshold
    UNION ALL
    SELECT threshold, b_id, a_id FROM dd_pairs, th WHERE 100 * score >= threshold
),
cc(threshold, record_id, comp) AS (
    SELECT DISTINCT threshold, x, x FROM e
    UNION
    SELECT cc.threshold, e.y, cc.comp FROM cc JOIN e ON e.threshold = cc.threshold AND e.x = cc.record_id
)
SELECT threshold, record_id, min(comp) AS cluster_id
FROM cc GROUP BY ALL;
