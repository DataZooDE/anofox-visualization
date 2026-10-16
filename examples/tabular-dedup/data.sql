-- Synthetic inputs of the "duplicate customers" use case (deterministic:
-- hash-based noise, no files). A made-up garden-supply retailer, Grünwerk,
-- brings the customers of three systems into one CRM: the webshop, the ERP
-- (business accounts, invoicing) and the stores' loyalty cards. The same
-- person is in several systems — each with its own conventions — and
-- sometimes twice in the same one.
CREATE OR REPLACE MACRO dd_unif(a, b, c) AS
    ((hash(md5(concat_ws('|', a, b, c))) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
-- one element of a list, picked by a uniform draw (skew > 1 favours the head)
CREATE OR REPLACE MACRO dd_pick(xs, u, skew) AS xs[1 + floor(len(xs) * pow(u, skew))::INTEGER];
-- swap two neighbouring letters (a typo) at a position drawn from u
CREATE OR REPLACE MACRO dd_typo(s, u) AS
    CASE WHEN length(s) < 4 THEN s ELSE
        left(s, 1 + floor((length(s) - 2) * u)::INTEGER)
        || substr(s, 3 + floor((length(s) - 2) * u)::INTEGER, 1)
        || substr(s, 2 + floor((length(s) - 2) * u)::INTEGER, 1)
        || substr(s, 4 + floor((length(s) - 2) * u)::INTEGER) END;
CREATE OR REPLACE MACRO dd_translit(s) AS
    replace(replace(replace(replace(replace(replace(replace(replace(s, 'ä', 'ae'), 'ö', 'oe'), 'ü', 'ue'), 'Ä', 'Ae'), 'Ö', 'Oe'), 'Ü', 'Ue'), 'ß', 'ss'), 'ı', 'i');

-- 1,500 real customers (the truth, known here because the data is made
-- up): ~22 % businesses with a valid German VAT ID. Common names repeat
-- (several different "Thomas Müller"s), and ~6 % share a household with
-- another customer: same surname, address and landline, but a different
-- person who must not be merged.
CREATE OR REPLACE TABLE dd_entities AS
WITH cities(c, city, postcode, area, sub_len) AS (VALUES
    (1, 'Berlin', '10115', '30', 8), (2, 'Hamburg', '20095', '40', 8), (3, 'München', '80331', '89', 8),
    (4, 'Leipzig', '04109', '341', 7), (5, 'Stuttgart', '70173', '711', 7), (6, 'Frankfurt am Main', '60311', '69', 8),
    (7, 'Dresden', '01067', '351', 7), (8, 'Hannover', '30159', '511', 7)),
base AS (
    SELECT e,
           dd_unif('e', e, 'biz') < 0.22 AS is_business,
           dd_pick(['Thomas', 'Anna', 'Michael', 'Julia', 'Stefan', 'Laura', 'Andreas', 'Sarah', 'Markus', 'Katharina',
                    'Jürgen', 'Sabine', 'Jörg', 'Birgit', 'Matthias', 'Claudia', 'Frank', 'Monika', 'Uwe', 'Petra',
                    'Lukas', 'Lena', 'Jonas', 'Hannah', 'Felix', 'Marie', 'Tobias', 'Sophie', 'Björn', 'Gülsen'],
                   dd_unif('e', e, 'fn'), 1.6) AS first_name,
           dd_pick(['Müller', 'Schmidt', 'Schneider', 'Fischer', 'Weber', 'Meyer', 'Wagner', 'Becker', 'Schulz', 'Hoffmann',
                    'Schäfer', 'Koch', 'Bauer', 'Richter', 'Klein', 'Wolf', 'Schröder', 'Neumann', 'Schwarz', 'Zimmermann',
                    'Braun', 'Krüger', 'Hofmann', 'Hartmann', 'Lange', 'Schmitt', 'Werner', 'Schmitz', 'Krause', 'Meier',
                    'Lehmann', 'Köhler', 'Huber', 'Kaiser', 'Fuchs', 'Peters', 'Jäger', 'Weiß', 'Yılmaz', 'Nowak'],
                   dd_unif('e', e, 'ln'), 1.5) AS last_name,
           1 + floor(8 * pow(dd_unif('e', e, 'city'), 1.3))::INTEGER AS c,
           dd_pick(['Hauptstraße', 'Gartenstraße', 'Bahnhofstraße', 'Schillerstraße', 'Goethestraße', 'Lindenstraße',
                    'Bergstraße', 'Kirchstraße', 'Waldstraße', 'Friedrichstraße', 'Am Markt', 'Rosenweg', 'Birkenweg',
                    'Mühlenweg', 'Schulstraße', 'Ringstraße'], dd_unif('e', e, 'st'), 1.0) AS street_name,
           1 + floor(120 * pow(dd_unif('e', e, 'no'), 2))::INTEGER AS house_no,
           dd_unif('e', e, 'mob') < 0.45 AS mobile
    FROM range(1, 1501) t(e)
),
households AS (  -- 6 % of the customers get a second person in the same household
    SELECT 1500 + row_number() OVER (ORDER BY e) AS e, false AS is_business,
           dd_pick(['Thomas', 'Anna', 'Michael', 'Julia', 'Stefan', 'Laura', 'Sabine', 'Frank', 'Petra', 'Lena'],
                   dd_unif('h', e, 'fn'), 1.0) AS first_name,
           last_name, c, street_name, house_no, false AS mobile, e AS household_of
    FROM base WHERE NOT is_business AND NOT mobile AND dd_unif('h', e, 'hh') < 0.12
),
people AS (
    SELECT *, NULL::BIGINT AS household_of FROM base
    UNION ALL
    SELECT * FROM households
),
vat AS (  -- the check digit that makes a synthetic German VAT ID valid
    SELECT p.e, min(cand) AS vat_id
    FROM people p, LATERAL (
        SELECT 'DE' || lpad((10000000 + floor(89999999 * dd_unif('vat', p.e, 'n')))::BIGINT::VARCHAR, 8, '0') || d::VARCHAR AS cand
        FROM range(0, 10) r(d)) x
    WHERE p.is_business AND vat_is_valid(cand)
    GROUP BY p.e
)
SELECT * REPLACE (  -- an email address belongs to one person: number the clashes
           CASE WHEN row_number() OVER (PARTITION BY email ORDER BY entity_id) = 1 THEN email
                ELSE replace(email, '@', row_number() OVER (PARTITION BY email ORDER BY entity_id)::VARCHAR || '@') END AS email)
FROM (
SELECT p.e AS entity_id, p.is_business, p.household_of,
       p.first_name, p.last_name,
       CASE WHEN p.is_business THEN dd_pick(['Gartenbau ', 'Landschaftsbau ', 'Baumschule ', 'Floristik '], dd_unif('e', p.e, 'co'), 1.0)
                                    || p.last_name || dd_pick([' GmbH', ' GmbH & Co. KG', ' e.K.'], dd_unif('e', p.e, 'lf'), 1.0) END AS company,
       ct.city, lpad((ct.postcode::INTEGER + 2 * floor(30 * dd_unif('e', coalesce(p.household_of, p.e), 'pc')))::INTEGER::VARCHAR, 5, '0') AS postcode,
       p.street_name, p.house_no,
       CASE WHEN p.mobile THEN dd_pick(['151', '160', '171', '176'], dd_unif('e', p.e, 'mp'), 1.0)
                               || (20000000 + floor(79999999 * dd_unif('e', p.e, 'mn')))::BIGINT::VARCHAR
            ELSE ct.area || CASE ct.sub_len WHEN 8 THEN (20000000 + floor(79999999 * dd_unif('e', coalesce(p.household_of, p.e), 'ln#')))::BIGINT::VARCHAR
                                            ELSE (2000000 + floor(7999999 * dd_unif('e', coalesce(p.household_of, p.e), 'ln#')))::BIGINT::VARCHAR END
            END AS phone_national,
       CASE WHEN p.is_business THEN 'info@' || lower(dd_translit(p.last_name)) || '-' || dd_pick(['garten', 'gruen', 'pflanzen'], dd_unif('e', p.e, 'dom'), 1.0)
                                    || '-' || lower(dd_translit(left(ct.city, 3))) || '.de'
            ELSE lower(dd_translit(p.first_name)) || dd_pick(['.', '', '_'], dd_unif('e', p.e, 'sep'), 1.0) || lower(dd_translit(p.last_name))
                 || CASE WHEN dd_unif('e', p.e, 'num') < 0.4 THEN (1960 + floor(45 * dd_unif('e', p.e, 'yr')))::INTEGER::VARCHAR ELSE '' END
                 || '@' || dd_pick(['gmail.com', 'web.de', 'gmx.de', 't-online.de', 'posteo.de'], dd_unif('e', p.e, 'prov'), 1.3) END AS email,
       v.vat_id
FROM people p JOIN cities ct USING (c) LEFT JOIN vat v USING (e)
);

-- The customer records of the three systems, each with its conventions:
--   webshop  "Firstname Lastname", phone "+49 30 23456789", email always;
--            ~5 % sign up twice (a second account, often a new email);
--   ERP      business accounts (and a few private ones): surname in capitals,
--            "Str." abbreviated, phone "030/23456789", VAT ID "DE 123 456 789";
--   loyalty  umlauts written out (Mueller), phone "(030) 2345 6789", email
--            on half the cards only.
-- Every system also has typos, ~7 % old addresses (the customer moved), a
-- few private emails replaced by a work address and a few malformed ones.
CREATE OR REPLACE TABLE dd_records_raw AS
WITH presence AS (
    SELECT e.*, s.source, s.k
    FROM dd_entities e, (VALUES ('webshop', 1), ('webshop', 2), ('erp', 1), ('loyalty', 1)) s(source, k)
    WHERE CASE s.source || s.k
            WHEN 'webshop1' THEN dd_unif('in', e.entity_id, 'w') < 0.72
                                 OR (dd_unif('in', e.entity_id, 'l') >= 0.42 AND NOT e.is_business)  -- everyone is somewhere
            WHEN 'webshop2' THEN dd_unif('in', e.entity_id, 'w') < 0.72 AND dd_unif('in', e.entity_id, 'w2') < 0.05
            WHEN 'erp1'     THEN e.is_business OR dd_unif('in', e.entity_id, 'e') < 0.06
            ELSE dd_unif('in', e.entity_id, 'l') < 0.42 END
),
noisy AS (
    SELECT p.*, md5(p.entity_id || p.source || p.k) AS rk,
           dd_unif('r', p.entity_id, p.source || p.k || 'typo') AS u_typo,
           dd_unif('r', p.entity_id, p.source || p.k || 'move') < 0.07 AND p.source || p.k <> 'webshop1' AS moved,
           dd_unif('r', p.entity_id, p.source || p.k || 'mail') AS u_mail
    FROM presence p
)
SELECT
    CASE source WHEN 'webshop' THEN 'W' WHEN 'erp' THEN 'E' ELSE 'L' END || '-'
        || (10000 + row_number() OVER (PARTITION BY source ORDER BY hash(rk)))::VARCHAR AS record_id,
    source, entity_id,
    -- name: typos in ~9 %, the ERP's capitals, the loyalty cards' transliteration
    CASE source
        WHEN 'erp' THEN first_name
        WHEN 'loyalty' THEN dd_translit(first_name)
        ELSE first_name END AS first_name,
    CASE WHEN u_typo < 0.09 THEN dd_typo(n, dd_unif('r', entity_id, rk)) ELSE n END AS last_name,
    company,
    CASE WHEN source = 'erp' THEN replace(street, 'straße', 'str.') ELSE street END AS street,
    postcode, city,
    CASE WHEN dd_unif('r', entity_id, rk || 'ph') < CASE source WHEN 'webshop' THEN 0.25 WHEN 'erp' THEN 0.05 ELSE 0.35 END THEN NULL
         WHEN source = 'webshop' THEN '+49 ' || regexp_replace(phone_national, '^(1[5-7][0-9]|30|40|89|69|341|711|351|511)', '\1 ')
         WHEN source = 'erp' THEN '0' || regexp_replace(phone_national, '^(1[5-7][0-9]|30|40|89|69|341|711|351|511)', '\1/')
         ELSE '(0' || regexp_replace(phone_national, '^(1[5-7][0-9]|30|40|89|69|341|711|351|511)(\d{4})', '\1) \2 ') END AS phone,
    CASE WHEN source = 'loyalty' AND u_mail < 0.5 THEN NULL
         WHEN source = 'webshop' AND u_mail < 0.015 THEN regexp_replace(email, '\.([a-z]+)$', ',\1')   -- "gmail,com"
         WHEN u_mail > 0.93 AND NOT is_business THEN lower(dd_translit(first_name)) || '.' || lower(dd_translit(last_name)) || '@' || lower(dd_translit(last_name)) || '-werke.de'
         WHEN source = 'webshop' AND k = 2 AND u_mail > 0.4 THEN replace(email, '@', '.shop@')
         WHEN source = 'erp' THEN upper(left(email, 1)) || substr(email, 2)
         ELSE email END AS email,
    CASE WHEN vat_id IS NULL OR source <> 'erp' AND dd_unif('r', entity_id, rk || 'vat') < 0.6 THEN NULL
         WHEN source = 'erp' THEN 'DE ' || substr(vat_id, 3, 3) || ' ' || substr(vat_id, 6, 3) || ' ' || substr(vat_id, 9, 3)
         ELSE lower(vat_id) END AS vat_id
FROM (
    SELECT *,
           CASE source WHEN 'erp' THEN upper(last_name) WHEN 'loyalty' THEN dd_translit(last_name) ELSE last_name END AS n,
           CASE WHEN moved THEN dd_pick(['Lindenallee', 'Am Hang', 'Feldweg', 'Parkstraße'], dd_unif('mv', entity_id, rk), 1.0)
                                || ' ' || (1 + floor(40 * dd_unif('mvn', entity_id, rk)))::INTEGER
                ELSE street_name || ' ' || house_no END AS street
    FROM noisy
) r;
