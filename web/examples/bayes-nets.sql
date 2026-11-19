-- @title What does the evidence say about the cause?
-- @group How it works
-- @tags  diagram, bayesian network, bayesian
-- @description Two Bayesian networks with evidence entered: why the grass is wet, and what a breathless patient with an abnormal X-ray has.
--
-- What does the evidence say about the cause? Two classic Bayesian networks,
-- as an inference engine writes them (nodes, arcs, beliefs, evidence and
-- the conditional probability tables), drawn with anofox_plot_bayes_net
-- (#1275). Each node shows its belief per state as bars, the prior (before
-- any evidence) as a faint bar behind them; a shaded node is observed, its
-- state in bold. Hover a node for its belief against the prior and its
-- conditional probability table, an arc for its strength.
--
-- The beliefs are exact (enumeration over every case). The arc strengths
-- of the chest clinic are how often a test found each arc in 200 resamples
-- of 1,000 simulated patients (a chi-square test at 5 %, given the child's
-- other parents).

-- Bayesian networks are a plugin (#1275): a build without it says so.
SELECT 'bayes-net'::REQUIRES;

-- The sprinkler: a cloudy day makes rain likelier and the sprinkler less
-- likely; either wets the grass.
CREATE OR REPLACE TABLE bn_sprinkler_nodes AS
FROM (VALUES
  ('cloudy', 'Cloudy', 'chance', ['yes', 'no']),
  ('sprinkler', 'Sprinkler on', 'chance', ['yes', 'no']),
  ('rain', 'Rain', 'chance', ['yes', 'no']),
  ('grass', 'Wet grass', 'observed', ['yes', 'no'])
) t(id, label, kind, states);

CREATE OR REPLACE TABLE bn_sprinkler_arcs AS
FROM (VALUES ('cloudy', 'sprinkler'), ('cloudy', 'rain'), ('sprinkler', 'grass'), ('rain', 'grass')) t("from", "to");

CREATE OR REPLACE TABLE bn_sprinkler_evidence AS
FROM (VALUES ('grass', 'yes')) t(node, state);

CREATE OR REPLACE TABLE bn_sprinkler_beliefs AS
FROM (VALUES
  ('cloudy', 'yes', 0.5758, 0.5),
  ('cloudy', 'no', 0.4242, 0.5),
  ('sprinkler', 'yes', 0.4298, 0.3),
  ('sprinkler', 'no', 0.5702, 0.7),
  ('rain', 'yes', 0.7079, 0.5),
  ('rain', 'no', 0.2921, 0.5),
  ('grass', 'yes', 1.0, 0.6471),
  ('grass', 'no', 0.0, 0.3529)
) t(node, state, probability, prior);

-- parent_states in the order of the node's arcs (grass: sprinkler, rain)
CREATE OR REPLACE TABLE bn_sprinkler_tables AS
FROM (VALUES
  ('cloudy', []::VARCHAR[], 'yes', 0.5),
  ('cloudy', []::VARCHAR[], 'no', 0.5),
  ('sprinkler', ['yes'], 'yes', 0.1),
  ('sprinkler', ['yes'], 'no', 0.9),
  ('sprinkler', ['no'], 'yes', 0.5),
  ('sprinkler', ['no'], 'no', 0.5),
  ('rain', ['yes'], 'yes', 0.8),
  ('rain', ['yes'], 'no', 0.2),
  ('rain', ['no'], 'yes', 0.2),
  ('rain', ['no'], 'no', 0.8),
  ('grass', ['yes', 'yes'], 'yes', 0.99),
  ('grass', ['yes', 'yes'], 'no', 0.01),
  ('grass', ['yes', 'no'], 'yes', 0.9),
  ('grass', ['yes', 'no'], 'no', 0.1),
  ('grass', ['no', 'yes'], 'yes', 0.9),
  ('grass', ['no', 'yes'], 'no', 0.1),
  ('grass', ['no', 'no'], 'yes', 0.0),
  ('grass', ['no', 'no'], 'no', 1.0)
) t(node, parent_states, state, probability);

-- The chest clinic ("Asia", Lauritzen & Spiegelhalter 1988): a visit to
-- Asia and smoking raise the risk of tuberculosis, lung cancer and
-- bronchitis; an X-ray cannot tell tuberculosis from cancer, shortness of
-- breath comes with any of them.
CREATE OR REPLACE TABLE bn_asia_nodes AS
FROM (VALUES
  ('asia', 'Visited Asia', 'chance', ['yes', 'no'], 'history'),
  ('smoke', 'Smoker', 'chance', ['yes', 'no'], 'history'),
  ('tub', 'Tuberculosis', 'chance', ['yes', 'no'], 'disease'),
  ('lung', 'Lung cancer', 'chance', ['yes', 'no'], 'disease'),
  ('bronc', 'Bronchitis', 'chance', ['yes', 'no'], 'disease'),
  ('either', 'TB or cancer', 'deterministic', ['yes', 'no'], 'disease'),
  ('xray', 'Abnormal X-ray', 'observed', ['yes', 'no'], 'finding'),
  ('dysp', 'Short of breath', 'observed', ['yes', 'no'], 'finding')
) t(id, label, kind, states, "group");

CREATE OR REPLACE TABLE bn_asia_arcs AS
FROM (VALUES
  ('asia', 'tub', 0.0),
  ('smoke', 'lung', 1.0),
  ('smoke', 'bronc', 1.0),
  ('tub', 'either', 1.0),
  ('lung', 'either', 1.0),
  ('either', 'xray', 1.0),
  ('bronc', 'dysp', 1.0),
  ('either', 'dysp', 1.0)
) t("from", "to", strength);

CREATE OR REPLACE TABLE bn_asia_evidence AS
FROM (VALUES ('xray', 'yes'), ('dysp', 'yes')) t(node, state);

CREATE OR REPLACE TABLE bn_asia_beliefs AS
FROM (VALUES
  ('asia', 'yes', 0.014, 0.01),
  ('asia', 'no', 0.986, 0.99),
  ('smoke', 'yes', 0.7856, 0.5),
  ('smoke', 'no', 0.2144, 0.5),
  ('tub', 'yes', 0.1139, 0.0104),
  ('tub', 'no', 0.8861, 0.9896),
  ('lung', 'yes', 0.6213, 0.055),
  ('lung', 'no', 0.3787, 0.945),
  ('bronc', 'yes', 0.6819, 0.45),
  ('bronc', 'no', 0.3181, 0.55),
  ('either', 'yes', 0.7287, 0.0648),
  ('either', 'no', 0.2713, 0.9352),
  ('xray', 'yes', 1.0, 0.1103),
  ('xray', 'no', 0.0, 0.8897),
  ('dysp', 'yes', 1.0, 0.436),
  ('dysp', 'no', 0.0, 0.564)
) t(node, state, probability, prior);

CREATE OR REPLACE TABLE bn_asia_tables AS
FROM (VALUES
  ('asia', []::VARCHAR[], 'yes', 0.01),
  ('asia', []::VARCHAR[], 'no', 0.99),
  ('smoke', []::VARCHAR[], 'yes', 0.5),
  ('smoke', []::VARCHAR[], 'no', 0.5),
  ('tub', ['yes'], 'yes', 0.05),
  ('tub', ['yes'], 'no', 0.95),
  ('tub', ['no'], 'yes', 0.01),
  ('tub', ['no'], 'no', 0.99),
  ('lung', ['yes'], 'yes', 0.1),
  ('lung', ['yes'], 'no', 0.9),
  ('lung', ['no'], 'yes', 0.01),
  ('lung', ['no'], 'no', 0.99),
  ('bronc', ['yes'], 'yes', 0.6),
  ('bronc', ['yes'], 'no', 0.4),
  ('bronc', ['no'], 'yes', 0.3),
  ('bronc', ['no'], 'no', 0.7),
  ('either', ['yes', 'yes'], 'yes', 1.0),
  ('either', ['yes', 'yes'], 'no', 0.0),
  ('either', ['yes', 'no'], 'yes', 1.0),
  ('either', ['yes', 'no'], 'no', 0.0),
  ('either', ['no', 'yes'], 'yes', 1.0),
  ('either', ['no', 'yes'], 'no', 0.0),
  ('either', ['no', 'no'], 'yes', 0.0),
  ('either', ['no', 'no'], 'no', 1.0),
  ('xray', ['yes'], 'yes', 0.98),
  ('xray', ['yes'], 'no', 0.02),
  ('xray', ['no'], 'yes', 0.05),
  ('xray', ['no'], 'no', 0.95),
  ('dysp', ['yes', 'yes'], 'yes', 0.9),
  ('dysp', ['yes', 'yes'], 'no', 0.1),
  ('dysp', ['yes', 'no'], 'yes', 0.8),
  ('dysp', ['yes', 'no'], 'no', 0.2),
  ('dysp', ['no', 'yes'], 'yes', 0.7),
  ('dysp', ['no', 'yes'], 'no', 0.3),
  ('dysp', ['no', 'no'], 'yes', 0.1),
  ('dysp', ['no', 'no'], 'no', 0.9)
) t(node, parent_states, state, probability);

SELECT 'Why is the grass wet?'::SECTION;

SELECT 4::SPAN;
SELECT '**The question.** The grass is wet this morning. Did it rain, or was the sprinkler on?

**The network.** Clouds make rain likelier (80 % against 20 %) and the sprinkler less likely (10 % against 50 %); rain or the sprinkler wets the grass (90 %, 99 % with both).

**What the evidence does.** Seeing wet grass raises *both* causes: rain from 50 % to 71 %, the sprinkler from 30 % to 43 % — and, through rain, a cloudy sky from 50 % to 58 %. Hover a node for its table.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 400::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a) FROM bn_sprinkler_arcs a),
         beliefs := (SELECT list(b) FROM bn_sprinkler_beliefs b),
         evidence := (SELECT list(e) FROM bn_sprinkler_evidence e),
         tables := (SELECT list(t) FROM bn_sprinkler_tables t))
       AS "Wet grass observed: rain or sprinkler?"
FROM bn_sprinkler_nodes n;

SELECT 'What does the patient have?'::SECTION;

SELECT 4::SPAN;
SELECT '**The question.** A patient is short of breath and the X-ray is abnormal. Tuberculosis, lung cancer or bronchitis?

**What the evidence does.** Lung cancer rises from 6 % to 62 %, tuberculosis from 1 % to 11 %, bronchitis from 45 % to 68 % — and the patient is now likely a smoker (79 %). The double outline is a logical node: *TB or cancer* is yes when either is.

**The arcs.** The thin dashed arc from *Visited Asia* is one the data rarely confirms: in 1,000 patients only about ten went to Asia, too few to show its effect. Hover an arc for its strength.'::MARKDOWN;

SELECT 8::SPAN;
SELECT 520::HEIGHT;
SELECT anofox_plot_bayes_net(n,
         arcs := (SELECT list(a) FROM bn_asia_arcs a),
         beliefs := (SELECT list(b) FROM bn_asia_beliefs b),
         evidence := (SELECT list(e) FROM bn_asia_evidence e),
         tables := (SELECT list(t) FROM bn_asia_tables t))
       AS "Short of breath, abnormal X-ray: what does the patient have?"
FROM bn_asia_nodes n;
