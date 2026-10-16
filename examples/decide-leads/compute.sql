-- anofox-decide use case: inbound lead routing and qualification — the native
-- computation behind the stored results.
--
-- The model (laya-multilingual, ONNX Runtime on the CPU, a 0.68 GB download)
-- can't run in the browser, so the dashboard reads the tables this script
-- produces from web/data/decide-leads/ (regenerate:
-- scripts/gen-decide-cases.sh leads). Run from the repo root:
--   duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"                 # once
--   duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"    # once
--   duckdb -unsigned -c ".read examples/decide-leads/compute.sql"
--
-- A made-up vendor of warehouse software, "Stockline": 300 synthetic inbound
-- messages (web form, chat, e-mail; English) over six weeks. Most are sales
-- enquiries; the rest belong to support, the partner team or nobody
-- (students, job seekers, agencies). For every message the CRM holds what the
-- sales development reps (SDRs) found out later — the right team, budget approved?
-- decision maker? buying this quarter? — and whether it became an opportunity.
-- Two local models read only the message and answer the same questions:
-- laya-multilingual and laya-typed-decisions (English, 0.85 GB; the better
-- router in the extension's evaluation), so the dashboard can compare them.
--   duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-typed-decisions')" # once
LOAD anofox_decide;

-- ── Message fragments ────────────────────────────────────────────────────────
-- Sales enquiries are composed of sentences; `kind`/`val` is what a sentence
-- says ({k} = number of warehouses).
CREATE OR REPLACE TABLE ld_fragments AS SELECT * FROM (VALUES
 ('role', 'dm', 'en', 'I am Head of Logistics at our company.'),
 ('role', 'dm', 'en', 'As COO I am responsible for our warehouse operations.'),
 ('role', 'dm', 'en', 'I run operations and sign off on our software purchases.'),
 ('role', 'dm', 'en', 'I am the managing director of a mid-sized wholesaler.'),
 ('role', 'staff', 'en', 'I am a working student in our operations team.'),
 ('role', 'staff', 'en', 'My manager asked me to look into warehouse software options.'),
 ('role', 'staff', 'en', 'I work in the IT team and was asked to compare a few vendors.'),
 ('company', '', 'en', 'We run {k} warehouses and ship to retailers across Europe.'),
 ('company', '', 'en', 'We are an online retailer with {k} warehouses.'),
 ('company', '', 'en', 'Our 3PL business operates {k} sites for fashion and electronics clients.'),
 ('need', '', 'en', 'Our current system cannot handle returns and we still pick on paper.'),
 ('need', '', 'en', 'We are outgrowing spreadsheets and need proper stock control.'),
 ('need', '', 'en', 'Our old WMS is no longer supported by the vendor.'),
 ('need', '', 'en', 'Stock levels in our shop and in the warehouse never match.'),
 ('budget', 'yes', 'en', 'The budget for the project has been approved.'),
 ('budget', 'yes', 'en', 'Management has signed off the budget for a new system.'),
 ('budget', 'no', 'en', 'We do not have a budget yet, we are just collecting information.'),
 ('budget', 'no', 'en', 'Funding is not decided, this is an early look.'),
 ('timeline', 'quarter', 'en', 'We need to decide before the end of this quarter.'),
 ('timeline', 'quarter', 'en', 'Our contract ends next month, so we have to move fast.'),
 ('timeline', 'year', 'en', 'We are planning the switch for next year.'),
 ('timeline', 'year', 'en', 'The project is on the roadmap for the second half of next year.'),
 ('timeline', 'none', 'en', 'There is no concrete timeline yet.'),
 ('timeline', 'none', 'en', 'We have not set a date for this.'),
 ('ask', '', 'en', 'Can we book a demo?'),
 ('ask', '', 'en', 'Could you send me your pricing?'),
 ('ask', '', 'en', 'Please call me back.'),
) t(kind, val, lang, sentence);

-- Messages that are not sales enquiries, whole. {n} = a ticket number.
CREATE OR REPLACE TABLE ld_other AS SELECT * FROM (VALUES
 ('customer support', 'en', 'Our handheld scanners stopped syncing with Stockline since yesterday and ticket #{n} has no answer yet.'),
 ('customer support', 'en', 'After the last update the pick lists print without bin locations. Please help, we are a customer.'),
 ('customer support', 'en', 'How do I add a new user to our Stockline account?'),
 ('customer support', 'en', 'Our Stockline invoice export to the ERP fails with error 500 since this morning.'),
 ('partner team', 'en', 'We are a systems integrator and would like to offer Stockline to our clients. Do you have a partner programme?'),
 ('partner team', 'en', 'We build ERP connectors and want to integrate with your API as a technology partner.'),
 ('partner team', 'en', 'As a logistics consultancy we would like to recommend Stockline. What are your reseller terms?'),
 ('nobody', 'en', 'I am a student writing my thesis on warehouse automation. Do you have any whitepapers?'),
 ('nobody', 'en', 'We are a digital agency and can double your website traffic. Shall we have a quick call?'),
 ('nobody', 'en', 'Please remove me from your newsletter.'),
 ('nobody', 'en', 'I would like to apply for the sales job on your website. Who should I send my CV to?'),
) t(team, lang, message);

-- ── Leads ────────────────────────────────────────────────────────────────────
CREATE OR REPLACE MACRO ld_u(i, k) AS (hash(i * 6007 + k) % 1000000) / 1000000.0;

CREATE OR REPLACE TABLE ld_base AS
SELECT i AS n, 1 + i % 6 AS week,
       'en' AS lang,
       CASE WHEN ld_u(i, 2) < 0.62 THEN 'sales' WHEN ld_u(i, 2) < 0.78 THEN 'customer support'
            WHEN ld_u(i, 2) < 0.86 THEN 'partner team' ELSE 'nobody' END AS team,
       CASE WHEN ld_u(i, 3) < 0.50 THEN 'web form' WHEN ld_u(i, 3) < 0.80 THEN 'e-mail' ELSE 'chat' END AS source,
       -- what the SDRs found out (sales enquiries)
       ld_u(i, 4) < 0.45 AS decision_maker,
       CASE WHEN ld_u(i, 5) < 0.30 THEN 'yes' WHEN ld_u(i, 5) < 0.55 THEN 'no' ELSE 'unstated' END AS budget_said,
       CASE WHEN ld_u(i, 6) < 0.30 THEN 'quarter' WHEN ld_u(i, 6) < 0.65 THEN 'year' WHEN ld_u(i, 6) < 0.80 THEN 'none' ELSE 'unstated' END AS timeline_said,
       ld_u(i, 7) < 0.30 AS hidden_budget, ld_u(i, 8) < 0.25 AS hidden_quarter,
       greatest(1, round(exp(ld_u(i, 9) * 3.2)))::INTEGER AS warehouses
FROM range(300) r(i);

CREATE OR REPLACE MACRO ld_pick(knd, v, lg, seed) AS
    (SELECT sentence FROM ld_fragments f WHERE f.kind = knd AND f.val = v AND f.lang = lg ORDER BY hash(f.sentence || seed) LIMIT 1);

CREATE OR REPLACE TABLE ld_leads AS
WITH l AS (
    SELECT b.*,
           team = 'sales' AND (budget_said = 'yes' OR (budget_said = 'unstated' AND hidden_budget)) AS budget_approved,
           team = 'sales' AND (timeline_said = 'quarter' OR (timeline_said = 'unstated' AND hidden_quarter)) AS buys_this_quarter
    FROM ld_base b)
SELECT 'L-' || (5001 + n) AS lead_id, week, source, team, warehouses,
       team = 'sales' AND decision_maker AS decision_maker, budget_approved, buys_this_quarter,
       -- became an opportunity within 30 days (sales enquiries only)
       team = 'sales' AND ld_u(n, 10) < 1 / (1 + exp(-(-3.8 + 1.7 * budget_approved::INTEGER + 1.1 * decision_maker::INTEGER
                 + 1.5 * buys_this_quarter::INTEGER + 0.6 * (timeline_said = 'year')::INTEGER + 0.35 * ln(warehouses)))) AS converted,
       CASE WHEN team = 'sales' THEN 8000 + 2500 * warehouses END AS deal_value_eur,
       CASE WHEN team = 'sales' THEN concat_ws(' ',
               CASE WHEN ld_u(n, 11) < 0.8 THEN ld_pick('role', CASE WHEN decision_maker THEN 'dm' ELSE 'staff' END, lang, n) END,
               replace(ld_pick('company', '', lang, n), '{k}', warehouses::VARCHAR),
               CASE WHEN ld_u(n, 12) < 0.7 THEN ld_pick('need', '', lang, n) END,
               CASE WHEN budget_said <> 'unstated' THEN ld_pick('budget', budget_said, lang, n) END,
               CASE WHEN timeline_said <> 'unstated' THEN ld_pick('timeline', timeline_said, lang, n) END,
               ld_pick('ask', '', lang, n))
            ELSE replace((SELECT message FROM ld_other o WHERE o.team = l.team AND o.lang = l.lang ORDER BY hash(o.message || l.n) LIMIT 1),
                         '{n}', (40000 + n * 37)::VARCHAR) END AS message
FROM l;

-- ── The models' answers: four questions per message in one request ─────────
-- The team options carry a short description; bare team names route sales
-- enquiries that mention a problem to support.
CREATE OR REPLACE TABLE ld_scored AS
WITH a AS (
    SELECT lead_id, m.model, unnest(decide_answers(message, '[
        {"id": "team",      "kind": "choice", "instruction": "Which team should handle this message?",
         "options": ["sales: wants to buy, asks for a demo, a quote or pricing",
                     "customer support: an existing customer reports a problem",
                     "partner team: a reseller or integrator",
                     "nobody: student, job applicant or advertising"]},
        {"id": "budget",    "kind": "binary", "instruction": "A budget has been approved."},
        {"id": "authority", "kind": "binary", "instruction": "The writer is a director, head or managing director."},
        {"id": "timeline",  "kind": "binary", "instruction": "They want to decide this quarter or within weeks."}]', model := m.model), recursive := true)
    FROM ld_leads, (VALUES ('laya-multilingual'), ('laya-typed-decisions')) m(model))
SELECT l.*, a.model,
       split_part(max(choice) FILTER (WHERE question_id = 'team'), ':', 1) AS team_pred,
       round(max(probability) FILTER (WHERE question_id = 'team'), 4) AS p_team,
       round(max(distribution['sales: wants to buy, asks for a demo, a quote or pricing']) FILTER (WHERE question_id = 'team'), 4) AS p_sales,
       round(max(probability) FILTER (WHERE question_id = 'budget'), 4) AS p_budget,
       round(max(probability) FILTER (WHERE question_id = 'authority'), 4) AS p_authority,
       round(max(probability) FILTER (WHERE question_id = 'timeline'), 4) AS p_timeline
FROM ld_leads l JOIN a USING (lead_id)
GROUP BY ALL;

-- Quality of each reason against what the SDRs found (sales enquiries only),
-- per model, with the extension's metrics.
CREATE OR REPLACE TABLE ld_metrics AS
WITH r AS (
    SELECT model, 'budget approved' AS reason, p_budget AS p, budget_approved AS y FROM ld_scored WHERE team = 'sales'
    UNION ALL SELECT model, 'decision maker', p_authority, decision_maker FROM ld_scored WHERE team = 'sales'
    UNION ALL SELECT model, 'buys this quarter', p_timeline, buys_this_quarter FROM ld_scored WHERE team = 'sales')
SELECT model, reason, round(decide_accuracy(p, y), 4) AS accuracy, round(decide_brier_score(p, y), 4) AS brier,
       round(decide_ece(p, y), 4) AS ece, count(*) AS leads, count(*) FILTER (WHERE y) AS positives
FROM r GROUP BY ALL;
