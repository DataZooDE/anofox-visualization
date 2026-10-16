-- anofox-decide use case: insurance claim routing — the native computation
-- behind the stored results.
--
-- The model (laya-multilingual, ONNX Runtime on the CPU, a 0.68 GB download)
-- can't run in the browser, so the dashboard reads the tables this script
-- produces from web/data/decide-claims/ (regenerate:
-- scripts/gen-decide-cases.sh claims). Run from the repo root:
--   duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"                 # once
--   duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"    # once
--   duckdb -unsigned -c ".read examples/decide-claims/compute.sql"
--
-- A made-up property & casualty insurer: 320 synthetic first notices of loss
-- (motor, home, travel; English and German) over eight weeks. Each claim has
-- the structured intake fields (amount, policy age, reporting delay, prior
-- claims, injury box) and the claimant's own description; a latent "fraud"
-- flag (the outcome after investigation) shapes both, imperfectly, the way
-- real red flags do. The model reads only the description.
LOAD anofox_decide;
SET anofox_decide_model = 'laya-multilingual';

-- ── Description fragments ────────────────────────────────────────────────────
-- What happened, per line of business. {amt} = the claimed amount.
CREATE OR REPLACE TABLE cl_incidents AS SELECT * FROM (VALUES
 ('motor', 'en', 'Another car reversed into mine in the supermarket car park; rear bumper and tailgate are dented.'),
 ('motor', 'en', 'Rear-end collision at a red light, the car behind me did not brake in time.'),
 ('motor', 'en', 'I slid on black ice and hit a garden wall, the front wing and headlight are broken.'),
 ('motor', 'en', 'A stone from a lorry cracked the windscreen on the motorway.'),
 ('motor', 'en', 'My car was stolen from outside my house overnight.'),
 ('motor', 'en', 'The car caught fire while parked in a side street at night and burnt out completely.'),
 ('motor', 'de', 'Beim Ausparken hat ein anderer Wagen meine Fahrertür gestreift.'),
 ('motor', 'de', 'Auffahrunfall an der Ampel, der Hintermann hat zu spät gebremst.'),
 ('motor', 'de', 'Mein Auto wurde nachts vor dem Haus gestohlen.'),
 ('motor', 'de', 'Hagel hat Motorhaube und Dach meines Autos verbeult.'),
 ('home',  'en', 'A pipe burst under the kitchen sink and soaked the floor and the lower cabinets.'),
 ('home',  'en', 'The storm blew tiles off the roof and rain came into the attic.'),
 ('home',  'en', 'Burglars broke in through the patio door while we were at work and took jewellery and a laptop.'),
 ('home',  'en', 'A fire in the garage destroyed tools, two bikes and the freezer.'),
 ('home',  'en', 'The washing machine leaked and the water came through the ceiling of the flat below.'),
 ('home',  'de', 'Ein Rohrbruch im Bad hat den Boden und die Wand durchnässt.'),
 ('home',  'de', 'Einbrecher haben die Terrassentür aufgehebelt und Schmuck sowie einen Fernseher gestohlen.'),
 ('home',  'de', 'Der Sturm hat einen Baum auf unser Carport stürzen lassen.'),
 ('travel','en', 'My suitcase did not arrive at the airport and the airline has not found it.'),
 ('travel','en', 'My camera and phone were stolen from a beach bag in Barcelona.'),
 ('travel','en', 'I fell ill two days before departure and had to cancel the trip.'),
 ('travel','de', 'Mein Koffer ist am Flughafen nicht angekommen.'),
 ('travel','de', 'Aus dem Hotelzimmer wurden Laptop und Kamera gestohlen.')
) t(lob, lang, incident);

-- Optional sentences; `kind` says what the sentence signals, `fraud` whether
-- it is drawn for fraudulent (true) or genuine (false) claims.
CREATE OR REPLACE TABLE cl_fragments AS SELECT * FROM (VALUES
 -- evidence (mostly genuine)
 ('evidence', false, 'en', 'Photos, the quote and the police reference are attached.'),
 ('evidence', false, 'en', 'The other driver admitted fault and gave me his insurance details; a witness saw it.'),
 ('evidence', false, 'en', 'I have attached photos of the damage and the receipts.'),
 ('evidence', false, 'de', 'Fotos, Kostenvoranschlag und Polizeibericht habe ich angehängt.'),
 ('evidence', false, 'de', 'Fotos vom Schaden und die Belege liegen bei.'),
 -- no evidence (mostly fraud)
 ('no_evidence', true, 'en', 'There were no witnesses and unfortunately the receipts were lost as well.'),
 ('no_evidence', true, 'en', 'I don''t have any photos, I cleaned everything up straight away.'),
 ('no_evidence', true, 'en', 'The items were all paid in cash so I have no receipts.'),
 ('no_evidence', true, 'de', 'Zeugen gab es keine, und die Quittungen sind leider auch weg.'),
 ('no_evidence', true, 'de', 'Fotos habe ich keine, ich habe sofort alles aufgeräumt.'),
 -- reported late
 ('late', true, 'en', 'I only noticed it weeks later when I came back and am reporting it now.'),
 ('late', true, 'en', 'Sorry for the delay, I did not get round to reporting it for about a month.'),
 ('late', true, 'de', 'Ich melde den Schaden erst jetzt, mehrere Wochen später.'),
 -- new or raised cover, valuable new items
 ('cover', true, 'en', 'I had just increased the cover last month because I had bought expensive new equipment.'),
 ('cover', true, 'en', 'Everything was brand new and top of the range, bought only a few weeks ago.'),
 ('cover', true, 'de', 'Die Versicherungssumme hatte ich erst letzten Monat erhöht, alles war neu und teuer.'),
 -- injury
 ('injury', false, 'en', 'My passenger has neck pain and has seen a doctor.'),
 ('injury', false, 'en', 'I hurt my wrist and was treated in A&E.'),
 ('injury', false, 'de', 'Mein Beifahrer hat Nackenschmerzen und war beim Arzt.'),
 ('injury', true,  'en', 'All four of us in the car have whiplash and are seeing a specialist; the repair quote from my cousin''s garage is {amt} EUR.'),
 ('injury', true,  'de', 'Wir alle haben ein Schleudertrauma und sind in Behandlung; die Werkstatt meines Bekannten verlangt {amt} EUR.'),
 -- amount sentence (both)
 ('amount', false, 'en', 'The damage is estimated at {amt} EUR.'),
 ('amount', false, 'de', 'Der Schaden beträgt laut Kostenvoranschlag {amt} EUR.'),
 ('amount', true,  'en', 'I am claiming the full {amt} EUR, please pay quickly as I need the money.'),
 ('amount', true,  'de', 'Ich fordere die vollen {amt} EUR, bitte zahlen Sie schnell aus.')
) t(kind, fraud, lang, sentence);

-- ── Claims ───────────────────────────────────────────────────────────────────
-- 320 claims; u(k) is a deterministic uniform draw per claim and purpose.
CREATE OR REPLACE MACRO cl_u(i, k) AS (hash(i * 7919 + k) % 1000000) / 1000000.0;

CREATE OR REPLACE TABLE cl_base AS
SELECT i AS n,
       CASE WHEN cl_u(i, 1) < 0.50 THEN 'motor' WHEN cl_u(i, 1) < 0.85 THEN 'home' ELSE 'travel' END AS lob,
       CASE WHEN cl_u(i, 2) < 0.30 THEN 'de' ELSE 'en' END AS lang,
       1 + (i % 8) AS week
FROM range(320) r(i);

CREATE OR REPLACE TABLE cl_claims0 AS
SELECT b.*,
       -- fraud after investigation: ~11 %, more often in travel theft and new policies
       cl_u(n, 3) < CASE lob WHEN 'travel' THEN 0.20 WHEN 'motor' THEN 0.11 ELSE 0.08 END AS fraud,
       (SELECT incident FROM cl_incidents c WHERE c.lob = b.lob AND c.lang = b.lang
        ORDER BY hash(c.incident || b.n) LIMIT 1) AS incident
FROM cl_base b;

CREATE OR REPLACE TABLE cl_claims1 AS
SELECT c.*,
       -- structured intake fields: fraud makes red flags likelier, not certain
       round(CASE lob WHEN 'motor' THEN 2200 WHEN 'home' THEN 2600 ELSE 500 END
             * exp(1.3 * (cl_u(n, 4) + cl_u(n, 5) + cl_u(n, 6) - 1.5)) * CASE WHEN fraud THEN 1.7 ELSE 1 END, -1)::INTEGER AS amount_eur,
       CASE WHEN (fraud AND cl_u(n, 7) < 0.45) OR (NOT fraud AND cl_u(n, 7) < 0.16)
            THEN 5 + (cl_u(n, 8) * 80)::INTEGER ELSE 120 + (cl_u(n, 8) * 3000)::INTEGER END AS policy_age_days,
       CASE WHEN (fraud AND cl_u(n, 9) < 0.35) OR (NOT fraud AND cl_u(n, 9) < 0.12)
            THEN 15 + (cl_u(n, 10) * 40)::INTEGER ELSE (cl_u(n, 10) * 6)::INTEGER END AS days_to_report,
       CASE WHEN fraud THEN (cl_u(n, 11) * 2.8)::INTEGER ELSE (cl_u(n, 11) * cl_u(n, 12) * 3.2)::INTEGER END AS prior_claims_3y,
       -- which sentences the claimant writes
       cl_u(n, 13) < CASE WHEN fraud THEN 0.25 ELSE 0.85 END AS s_evidence,
       cl_u(n, 14) < CASE WHEN fraud THEN 0.55 ELSE 0.06 END AS s_no_evidence,
       cl_u(n, 15) < CASE WHEN fraud THEN 0.40 ELSE 0.04 END AS s_cover,
       lob = 'motor' AND cl_u(n, 16) < CASE WHEN fraud THEN 0.45 ELSE 0.18 END AS injury
FROM cl_claims0 c;

CREATE OR REPLACE MACRO cl_pick(knd, fr, lg, seed) AS
    (SELECT sentence FROM cl_fragments f WHERE f.kind = knd AND f.fraud = fr AND f.lang = lg ORDER BY hash(f.sentence || seed) LIMIT 1);

CREATE OR REPLACE TABLE cl_claims AS
SELECT 'CL-' || (24001 + n) AS claim_id, week, lob, lang, fraud, amount_eur, policy_age_days, days_to_report,
       prior_claims_3y, injury,
       replace(concat_ws(' ',
           incident,
           CASE WHEN injury THEN cl_pick('injury', fraud AND cl_u(n, 17) < 0.6, lang, n) END,
           CASE WHEN days_to_report > 14 AND cl_u(n, 18) < 0.7 THEN cl_pick('late', true, lang, n) END,
           CASE WHEN s_cover THEN cl_pick('cover', true, lang, n) END,
           CASE WHEN s_evidence AND NOT s_no_evidence THEN cl_pick('evidence', false, lang, n) END,
           CASE WHEN s_no_evidence THEN cl_pick('no_evidence', true, lang, n) END,
           CASE WHEN NOT (injury AND fraud AND cl_u(n, 17) < 0.6) THEN cl_pick('amount', fraud AND cl_u(n, 19) < 0.5, lang, n) END),
         '{amt}', format('{:,}', amount_eur)) AS description
FROM cl_claims1;

-- ── The model's answers: four questions per claim in one request ────────────
-- One generic question ("suspicious?") and three concrete red flags.
CREATE OR REPLACE TABLE cl_answers AS
WITH a AS (
    SELECT claim_id, unnest(decide_answers(description, '[
        {"id": "suspicious", "kind": "binary", "instruction": "This claim is suspicious."},
        {"id": "evidence",   "kind": "binary", "instruction": "The claimant provides evidence such as photos, invoices, a police report or a witness."},
        {"id": "recent",     "kind": "binary", "instruction": "The insured items were new or the cover was raised shortly before the loss."},
        {"id": "pressure",   "kind": "binary", "instruction": "The claimant pushes for a fast payout."}]'), recursive := true)
    FROM cl_claims)
SELECT c.*,
       round(max(probability) FILTER (WHERE question_id = 'suspicious'), 4) AS p_suspicious,
       round(max(probability) FILTER (WHERE question_id = 'evidence'), 4) AS p_evidence,
       round(max(probability) FILTER (WHERE question_id = 'recent'), 4) AS p_recent,
       round(max(probability) FILTER (WHERE question_id = 'pressure'), 4) AS p_pressure
FROM cl_claims c JOIN a USING (claim_id)
GROUP BY ALL;

-- ── Red-flag weights, fitted on the investigated claims ──────────────────────
-- Every signal becomes a weight of evidence (log-odds points against the base
-- fraud rate): rules from their hit rates, answers through a Platt fit
-- (decide_fit_calibration) that turns the model's probability into P(fraud).
-- Summed, they give a transparent fraud score whose terms are the reasons.
CREATE OR REPLACE MACRO cl_logit(p) AS ln(greatest(least(p, 0.9999), 0.0001) / (1 - greatest(least(p, 0.9999), 0.0001)));
CREATE OR REPLACE MACRO cl_platt(spec, p) AS
    1 / (1 + exp(-(split_part(split_part(spec, ':', 2), ',', 1)::DOUBLE * cl_logit(p)
                   + split_part(spec, ',', 2)::DOUBLE)));

CREATE OR REPLACE TABLE cl_flags AS
SELECT *, policy_age_days < 90 AS new_policy, days_to_report > 14 AS late_report,
       prior_claims_3y >= 2 AS repeat_claimant, amount_eur > 8000 AS large_loss,
       1 - p_evidence AS p_no_evidence
FROM cl_answers;

CREATE OR REPLACE TABLE cl_signals AS
WITH base AS (SELECT avg(fraud::INTEGER) AS rate FROM cl_flags),
rules AS (
    UNPIVOT (SELECT fraud, new_policy, late_report, repeat_claimant, large_loss FROM cl_flags)
    ON new_policy, late_report, repeat_claimant, large_loss INTO NAME signal VALUE hit),
fits AS (
    UNPIVOT (SELECT fraud, p_no_evidence, p_recent, p_pressure, p_suspicious FROM cl_flags)
    ON p_no_evidence, p_recent, p_pressure, p_suspicious INTO NAME signal VALUE p)
SELECT signal, 'rule' AS source,
       cl_logit((count(*) FILTER (WHERE hit AND fraud) + 1) / (count(*) FILTER (WHERE hit) + 2)) - cl_logit(any_value(rate)) AS w_hit,
       cl_logit((count(*) FILTER (WHERE NOT hit AND fraud) + 1) / (count(*) FILTER (WHERE NOT hit) + 2)) - cl_logit(any_value(rate)) AS w_miss,
       NULL::VARCHAR AS platt
FROM rules, base GROUP BY signal
UNION ALL
SELECT replace(signal, 'p_', ''), 'description', NULL, NULL, decide_fit_calibration(p, fraud)
FROM fits GROUP BY signal;

-- One row per claim: each signal's weight (log-odds points) and the scores.
CREATE OR REPLACE TABLE cl_scored AS
WITH w AS (PIVOT (SELECT signal, w_hit, w_miss, platt FROM cl_signals) ON signal USING any_value(w_hit) AS hit, any_value(w_miss) AS miss, any_value(platt) AS platt),
b AS (SELECT cl_logit(avg(fraud::INTEGER)) AS l0 FROM cl_flags),
x AS (
    SELECT f.*, b.l0,
           round(CASE WHEN new_policy THEN w.new_policy_hit ELSE w.new_policy_miss END, 3) AS w_new_policy,
           round(CASE WHEN late_report THEN w.late_report_hit ELSE w.late_report_miss END, 3) AS w_late_report,
           round(CASE WHEN repeat_claimant THEN w.repeat_claimant_hit ELSE w.repeat_claimant_miss END, 3) AS w_repeat_claimant,
           round(CASE WHEN large_loss THEN w.large_loss_hit ELSE w.large_loss_miss END, 3) AS w_large_loss,
           round(cl_logit(cl_platt(w.no_evidence_platt, p_no_evidence)) - b.l0, 3) AS w_no_evidence,
           round(cl_logit(cl_platt(w.recent_platt, p_recent)) - b.l0, 3) AS w_recent,
           round(cl_logit(cl_platt(w.pressure_platt, p_pressure)) - b.l0, 3) AS w_pressure
    FROM cl_flags f, w, b)
SELECT * EXCLUDE (l0, p_no_evidence),
       round(l0, 4) AS base_logit
FROM x;

-- Quality of each answer against the investigated outcome (the extension's
-- metrics), and of the generic "suspicious?" question.
CREATE OR REPLACE TABLE cl_metrics AS
WITH f AS (UNPIVOT (SELECT fraud, 1 - p_evidence AS no_evidence, p_recent AS recent, p_pressure AS pressure, p_suspicious AS suspicious FROM cl_answers)
           ON no_evidence, recent, pressure, suspicious INTO NAME question VALUE p)
SELECT question, round(decide_accuracy(p, fraud), 4) AS accuracy,
       round(decide_brier_score(p, fraud), 4) AS brier, round(decide_ece(p, fraud), 4) AS ece,
       count(*) AS claims, count(*) FILTER (WHERE fraud) AS fraud_claims, 'laya-multilingual' AS model
FROM f GROUP BY question;
