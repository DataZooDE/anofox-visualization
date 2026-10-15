-- anofox-decide showcase — the native computation behind the stored results.
--
-- Natural-language decisions need a decision model; this script uses the local
-- laya-multilingual model (ONNX Runtime, CPU, offline after a 0.68 GB
-- download), which can't run in the browser. The demo therefore reads the
-- tables this script produces from web/data/decide/
-- (regenerate: scripts/gen-showcase-data.sh decide). Run from the repo root:
--   duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"      # once
--   duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"  # once
--   duckdb -unsigned -c ".read examples/decide-showcase/compute.sql"
--
-- A made-up software shop's support inbox: 92 synthetic tickets in English,
-- German and French, each with the answer a support lead gave (refund asked
-- for? which team owns it? how upset is the writer?). The model answers the
-- same three questions; the dashboard shows how good its answers are.
LOAD anofox_decide;
SET anofox_decide_model = 'laya-multilingual';

-- Ticket templates: text, language and the labelled answers. {p} = product.
CREATE OR REPLACE TABLE dc_templates AS SELECT * FROM (VALUES
 -- refund requests (billing)
 ('en', 'I was charged twice for my {p} subscription this month. Please refund the duplicate payment.', true, 'billing', 1),
 ('en', 'This is the third time {p} has billed me after I cancelled. I want ALL my money back now.', true, 'billing', 2),
 ('en', 'Please cancel my {p} plan and reimburse the unused months.', true, 'billing', 0),
 ('en', '{p} does not do what your website promised. I would like a refund.', true, 'billing', 1),
 ('de', 'Sie haben mir {p} doppelt abgebucht. Bitte erstatten Sie den zweiten Betrag.', true, 'billing', 1),
 ('de', 'Ich habe {p} vor Wochen gekündigt und zahle immer noch! Ich will mein Geld zurück, sofort.', true, 'billing', 2),
 ('fr', 'J''ai été débité deux fois pour {p}. Merci de me rembourser le second paiement.', true, 'billing', 1),
 ('fr', 'Je veux être remboursé : {p} ne fonctionne pas comme annoncé, c''est inadmissible !', true, 'billing', 2),
 -- billing without a refund
 ('en', 'Our invoice for {p} shows the wrong VAT number. Could you issue a corrected invoice?', false, 'billing', 0),
 ('en', 'How do I change the credit card used for {p}?', false, 'billing', 0),
 ('de', 'Bitte schicken Sie die Rechnung für {p} an unsere Buchhaltung statt an mich.', false, 'billing', 0),
 ('fr', 'Pouvez-vous m''envoyer la facture de {p} pour le mois dernier ?', false, 'billing', 0),
 -- defects
 ('en', '{p} crashes every time I open the export dialog. This is blocking our month-end close!', false, 'defect', 2),
 ('en', 'Since the last update {p} shows a blank screen on login. Please fix this.', false, 'defect', 1),
 ('en', 'Small thing: the date picker in {p} starts weeks on Sunday even with a German locale.', false, 'defect', 0),
 ('de', '{p} stürzt beim Speichern ab, wir verlieren ständig Daten. Das ist eine Katastrophe!', false, 'defect', 2),
 ('de', 'In {p} wird der Bericht seit gestern nicht mehr exportiert, Fehler 500.', false, 'defect', 1),
 ('fr', '{p} plante à chaque ouverture du tableau de bord. Nous ne pouvons plus travailler !', false, 'defect', 2),
 ('fr', 'Le bouton d''export de {p} ne réagit plus depuis la mise à jour.', false, 'defect', 1),
 -- defect + refund
 ('en', '{p} has been down for three days. Refund this month, it is unacceptable.', true, 'defect', 2),
 ('de', '{p} funktioniert seit einer Woche nicht. Ich verlange eine Gutschrift für diesen Monat.', true, 'defect', 2),
 -- other
 ('en', 'Thanks for the quick help with {p} yesterday, everything works now!', false, 'other', 0),
 ('en', 'Do you offer training sessions for new {p} users?', false, 'other', 0),
 ('de', 'Haben Sie ein Büro in München? Wir würden {p} gern vor Ort besprechen.', false, 'other', 0),
 ('fr', 'Merci pour la démonstration de {p}, très claire !', false, 'other', 0),
 ('en', 'I have asked about {p} pricing for teams twice and nobody answers. Is anyone reading this?', false, 'other', 1)
) t(lang, template, refund, team, frustration);

-- Four products × the templates = 104 tickets; drop a few combinations so the
-- languages are not perfectly balanced (deterministic).
CREATE OR REPLACE TABLE dc_tickets AS
SELECT row_number() OVER (ORDER BY p.k, t.template) AS ticket_id, t.lang,
       replace(t.template, '{p}', p.product) AS text, t.refund, t.team, t.frustration
FROM dc_templates t, (VALUES (1, 'Ledgerly'), (2, 'Shiftboard'), (3, 'PayRoll Pro'), (4, 'DocuVault')) p(k, product)
WHERE (hash(t.template || p.product) % 13) <> 0;

-- The model's answers: three questions per ticket in one request.
CREATE OR REPLACE TABLE dc_scored AS
WITH a AS (
    SELECT ticket_id, unnest(decide_answers(text, '[
        {"id": "refund", "kind": "binary", "instruction": "The customer asks for money back (a refund, reimbursement or credit)."},
        {"id": "team",   "kind": "choice", "instruction": "Which team should handle this ticket?", "options": ["billing", "defect", "other"]},
        {"id": "mood",   "kind": "score",  "instruction": "How frustrated is the writer?", "levels": ["calm", "annoyed", "angry"]}]'), recursive := true)
    FROM dc_tickets)
SELECT t.*,
       max(probability) FILTER (WHERE question_id = 'refund') AS p_refund,
       max(choice) FILTER (WHERE question_id = 'team') AS team_pred,
       max(probability) FILTER (WHERE question_id = 'team') AS p_team,
       max(score) FILTER (WHERE question_id = 'mood') AS frustration_score
FROM dc_tickets t JOIN a USING (ticket_id)
GROUP BY ALL;

-- Quality of the refund probability: the extension's own metrics.
CREATE OR REPLACE TABLE dc_metrics AS
SELECT round(decide_accuracy(p_refund, refund), 4) AS accuracy,
       round(decide_brier_score(p_refund, refund), 4) AS brier,
       round(decide_ece(p_refund, refund), 4) AS ece,
       round(avg((team_pred = team)::INTEGER), 4) AS team_accuracy,
       count(*) AS tickets,
       'laya-multilingual' AS model
FROM dc_scored;
