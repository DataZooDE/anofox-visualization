-- @title How does the model reason from our inspections?
-- @group How it works
-- @tags  diagram, factor graph, bayesian
-- @description Three Bayesian models as factor graphs, each with the problem it solves: a defect share, a cycle time, when a machine's condition changed.
--
-- How does the model reason from our inspections? Three models as an
-- inference library writes them (nodes, edges and posterior marginals),
-- drawn with anofox_plot_factor_graph (#1151):
--   1. the defect rate θ: a Beta prior and four inspections (defective or
--      not). θ meets every inspection at its equality node; hover it for
--      what the model believes now (mean ± sd);
--   2. a machine's cycle time, its mean m and precision τ learned together
--      (mean-field): the estimates settle over six iterations — hover m or
--      τ for its posterior mean per iteration, click it for its prior and
--      posterior beside the graph and its mean per iteration under it;
--   3. a machine's hidden state over five shifts: the states form one row,
--      each shift's reading drops below its state, and the transition and
--      reading matrices (learned) sit in the parameter band above.
-- Step / Play walk through the messages the inference passed, in order:
-- each lights its edge, moving the message's way (schedule :=, #1152).
-- Factors are boxes, variables circles (filled when observed), equality
-- nodes small squares; edges carry the factor's interface names.
--
-- The tables are stored in data/factor-graphs/ (CSV, as the library wrote
-- them).

CREATE OR REPLACE TABLE fg_coin_nodes AS FROM read_csv('data/factor-graphs/coin-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_edges AS FROM read_csv('data/factor-graphs/coin-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_marginals AS FROM read_csv('data/factor-graphs/coin-marginals.csv');
CREATE OR REPLACE TABLE fg_hmm_nodes AS FROM read_csv('data/factor-graphs/hmm-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_hmm_edges AS FROM read_csv('data/factor-graphs/hmm-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_coin_schedule AS FROM read_csv('data/factor-graphs/coin-schedule.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_hmm_schedule AS FROM read_csv('data/factor-graphs/hmm-schedule.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_cycle_nodes AS FROM read_csv('data/factor-graphs/cycle-nodes.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_cycle_edges AS FROM read_csv('data/factor-graphs/cycle-edges.csv', all_varchar = true);
CREATE OR REPLACE TABLE fg_cycle_marginals AS FROM read_csv('data/factor-graphs/cycle-marginals.csv');
-- #1172: what the library adds — each message's value, the prediction at
-- each observation, each observation's influence, the free energy
CREATE OR REPLACE TABLE fg_coin_messages AS FROM read_csv('data/factor-graphs/coin-messages.csv');
CREATE OR REPLACE TABLE fg_coin_predictive AS FROM read_csv('data/factor-graphs/coin-predictive.csv');
CREATE OR REPLACE TABLE fg_coin_influence AS FROM read_csv('data/factor-graphs/coin-influence.csv');
CREATE OR REPLACE TABLE fg_cycle_messages AS FROM read_csv('data/factor-graphs/cycle-messages.csv');
CREATE OR REPLACE TABLE fg_cycle_predictive AS FROM read_csv('data/factor-graphs/cycle-predictive.csv');
CREATE OR REPLACE TABLE fg_cycle_influence AS FROM read_csv('data/factor-graphs/cycle-influence.csv');
CREATE OR REPLACE TABLE fg_cycle_free_energy AS FROM read_csv('data/factor-graphs/cycle-free_energy.csv');
CREATE OR REPLACE TABLE fg_cycle_schedule AS FROM read_csv('data/factor-graphs/cycle-schedule.csv', all_varchar = true);

SELECT 'The defect rate after four inspections'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** Customers sent back units of a new product. Before we claim against the supplier we need to know what share of returned units has a real defect (not "no fault found") — and how sure we can be.

**The model.** θ is the defect share; each checked return $y_i$ is 1 (defective) or 0:

$$\theta \sim \mathrm{Beta}(2,\,3), \qquad y_i \mid \theta \sim \mathrm{Bernoulli}(\theta), \quad i = 1, \dots, 4$$

**Why this prior.** A Beta prior is *conjugate* to the Bernoulli: the posterior is again a Beta, so the messages are exact and the counts simply add. Its two numbers read as earlier evidence — 2 defective and 3 fine units, as if from earlier products: about 40 %, but weighted like only five checks, so loose (90 % between 10 % and 75 %). Unlike a flat Beta(1, 1) it rules out "never" and "always".

**What comes out.** Three of the four returns were defective ($k = 3$ of $n = 4$):

$$\theta \mid y \sim \mathrm{Beta}(2 + k,\; 3 + n - k) = \mathrm{Beta}(5,\,4), \qquad \mathbb{E}[\theta \mid y] = \tfrac{5}{9} \approx 56\,\%, \quad \mathrm{sd} \approx 16\,\%$$

Five checks'' worth of prior and four of data: the answer lands between the prior''s 40 % and the data''s 75 %, still wide — check more returns before claiming.'::MARKDOWN,
       'What this graph solves'::TITLE;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_coin_edges e),
         marginals := (SELECT list(m) FROM fg_coin_marginals m),
         schedule := (SELECT list(s) FROM fg_coin_schedule s),
         messages := (SELECT list(m) FROM fg_coin_messages m),
         predictive := (SELECT list(p) FROM fg_coin_predictive p),
         influence := (SELECT list(i) FROM fg_coin_influence i))
       AS "What is the defect rate?"
FROM fg_coin_nodes n;

SELECT 'A machine''s cycle time: mean and spread, learned together'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** To plan a shift we need to know how long one cycle on a machine takes and how much it varies: the mean sets the output per shift, the spread the buffer to plan in and the limit at which a slow cycle should raise an alarm.

**The model.** Six timed cycles $y_i$ (minutes: 4.1, 3.8, 4.4, 4.0, 3.6, 4.3) around a mean $m$ with precision $\tau$ (one over the variance):

$$m \sim \mathcal{N}(0,\,100), \qquad \tau \sim \Gamma(1,\,1), \qquad y_i \mid m, \tau \sim \mathcal{N}(m,\,\tau^{-1}), \quad i = 1, \dots, 6$$

**Why these priors.** Both are *vague*: we want the six cycles to decide. $\mathcal{N}(0, 100)$ has an sd of 10 min — nearly flat over any plausible cycle time, its precision 0.01 against the data''s $6\,\mathbb{E}[\tau] \approx 17$; and $\Gamma(1, 1)$ (shape, rate) on the precision says "a spread of about a minute", worth only two observations. Both are conjugate (Normal for a mean, Gamma for a precision), so each update has a closed form. The price of the simple choice: $\mathcal{N}(0, 100)$ also allows negative times — harmless here, a log-normal would be stricter.

**How it is solved.** Mean-field, $q(m, \tau) = q(m)\,q(\tau)$: each factor is updated from the other''s current estimate, round after round:

$$q(m) = \mathcal{N}\!\left(\frac{\mathbb{E}[\tau] \sum_i y_i}{0.01 + 6\,\mathbb{E}[\tau]},\ \frac{1}{0.01 + 6\,\mathbb{E}[\tau]}\right), \qquad q(\tau) = \Gamma\!\left(1 + \tfrac{6}{2},\ 1 + \tfrac{1}{2} \sum_i \mathbb{E}\big[(y_i - m)^2\big]\right)$$

**What comes out.** After six rounds: a cycle takes $m = 4.03 \pm 0.24$ min on average; $\mathbb{E}[\tau] = 2.85$, so one cycle varies by about $1/\sqrt{\tau} \approx \pm 0.6$ min.'::MARKDOWN,
       'What this graph solves'::TITLE;

-- The priors as the model states them: m ~ N(0, variance 100), τ ~ Γ(shape 1, rate 1).
CREATE OR REPLACE TABLE fg_cycle_priors AS
  FROM (VALUES ('v6', 'Normal', 0.0, 10.0), ('v9', 'Gamma', 1.0, 1.0)) p(variable, family, mean, sd);
-- The variable a click on the graph picks (its equality node e6 / e9), else m.
CREATE OR REPLACE MACRO fg_pick() AS coalesce(
  (SELECT e.variable FROM fg_cycle_edges e JOIN fg_cycle_nodes n ON n.id = e.variable
   WHERE e.factor = getvariable('fg_node') AND n.kind = 'variable' LIMIT 1),
  (SELECT id FROM fg_cycle_nodes WHERE id = getvariable('fg_node') AND kind = 'variable'),
  'v6');
-- A density at x for a family, from its mean and sd (Normal, Gamma).
CREATE OR REPLACE MACRO fg_density(family, x, mu, sd) AS CASE family
  WHEN 'Gamma' THEN CASE WHEN x > 0 THEN exp(((mu / sd) ^ 2 - 1) * ln(x) - x * mu / sd ^ 2
                                         + (mu / sd) ^ 2 * ln(mu / sd ^ 2) - lgamma((mu / sd) ^ 2)) ELSE 0 END
  ELSE exp(-0.5 * ((x - mu) / sd) ^ 2) / (sd * sqrt(2 * pi())) END;

SELECT 8::SPAN;
SELECT 420::HEIGHT;
SELECT 'fg_node'::SELECT_AS;
SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_cycle_edges e),
         marginals := (SELECT list(m) FROM fg_cycle_marginals m),
         schedule := (SELECT list(s) FROM fg_cycle_schedule s),
         messages := (SELECT list(m) FROM fg_cycle_messages m),
         predictive := (SELECT list(p) FROM fg_cycle_predictive p),
         influence := (SELECT list(i) FROM fg_cycle_influence i))
       AS "How long does a cycle take, and how steady is it? (click m or τ)"
FROM fg_cycle_nodes n;

-- Beside it, the clicked variable: what the model believed before the data
-- (its prior) and after (its posterior, the last iteration).
SELECT 4::SPAN;
SELECT 420::HEIGHT;
WITH post AS (SELECT * FROM fg_cycle_marginals WHERE variable = fg_pick() ORDER BY iteration DESC LIMIT 1),
pri AS (SELECT * FROM fg_cycle_priors WHERE variable = fg_pick()),
lab AS (SELECT replace(replace(label, '\tau', 'τ'), '\', '') AS v FROM fg_cycle_nodes WHERE id = fg_pick()),
rng AS (SELECT greatest(CASE WHEN post.family = 'Gamma' THEN 0 ELSE -1e300 END,
                        least(pri.mean - 3 * pri.sd, post.mean - 4 * post.sd), post.mean - 12 * post.sd) AS lo,
               least(greatest(pri.mean + 3 * pri.sd, post.mean + 4 * post.sd), post.mean + 12 * post.sd) AS hi
        FROM post, pri),
xs AS (SELECT lo + (hi - lo) * i / 120.0 AS x FROM rng, range(0, 121) r(i))
SELECT round(x, 4)::XAXIS, k::CATEGORY, round(d, 5)::LINECHART,
       ('What we believe about ' || (SELECT v FROM lab) || ': before and after the data')::TITLE,
       'The prior is what the model assumed before any cycle was measured; the posterior what it believes after the six. The narrower and taller the curve, the surer the model.'::INFO
FROM (SELECT x, 'before (prior)' AS k, fg_density(pri.family, x, pri.mean, pri.sd) AS d FROM xs, pri
      UNION ALL
      SELECT x, 'after six cycles (posterior)', fg_density(post.family, x, post.mean, post.sd) FROM xs, post)
ORDER BY k, x;

-- Under them, how it settled: the mean ± sd per iteration.
-- Under them, the clicked variable in three views: how it settled, which
-- cycles drove it, and whether the iterations converged.
SELECT 'How it settled' AS fg_more ::VIEW;
SELECT 12::SPAN;
SELECT 220::HEIGHT;
SELECT iteration::XAXIS, round(mean, 4)::LINECHART,
       round(mean - sd, 4)::BAND_LOWER, round(mean + sd, 4)::BAND_UPPER,
       ((SELECT replace(replace(label, '\tau', 'τ'), '\', '') FROM fg_cycle_nodes WHERE id = fg_pick())
        || ' by iteration: its mean and ± 1 sd as the messages go round')::TITLE
FROM fg_cycle_marginals WHERE variable = fg_pick() ORDER BY iteration;

-- Which cycles drove the clicked variable: how far it moves without each
-- one (leave-one-out, from the library).
SELECT 'What drove it' AS fg_more ::VIEW;
SELECT 12::SPAN;
SELECT 220::HEIGHT;
SELECT replace(replace(n.label, '_{', ''), '}', '')::XAXIS,
       round(i.mean - i.loo_mean, 4)::BARCHART,
       ((SELECT replace(replace(label, '\tau', 'τ'), '\', '') FROM fg_cycle_nodes WHERE id = fg_pick())
        || ' with each cycle minus without it: which cycles moved it most')::TITLE,
       'Each bar: the posterior mean with all six cycles minus the mean without that one. A long bar is a cycle the estimate leans on.'::INFO
FROM fg_cycle_influence i JOIN fg_cycle_nodes n ON n.id = i.observation
WHERE i.target = fg_pick() ORDER BY 2 DESC;

-- Whether the iterations settled: the free energy per iteration (lower is
-- a better fit; flat means converged).
SELECT 'Converged?' AS fg_more ::VIEW;
SELECT 12::SPAN;
SELECT 220::HEIGHT;
SELECT iteration::XAXIS, round(free_energy, 4)::LINECHART,
       'Free energy per iteration: the mean-field updates settle within three rounds'::TITLE,
       'The Bethe free energy the message passing minimises. When it stops falling, more iterations change nothing.'::INFO
FROM fg_cycle_free_energy ORDER BY iteration;
SELECT 1::ENDVIEW;

SELECT 'A machine''s hidden state over five shifts'::SECTION;

SELECT 12::SPAN;
SELECT '**The problem.** A machine wears without telling us: per shift we only see a coarse reading (one of three levels), not its condition. Maintenance wants to know in which shift the condition changed, to service the machine before it fails.

**The model.** A hidden condition $s_t$ (one of three) per shift, the reading $x_t$ (here 0, 0, 1, 2, 2) drawn from it; $A$ says how conditions follow one another, $B$ what each condition reads:

$$s_0 \sim \mathrm{Cat}\big(\tfrac13, \tfrac13, \tfrac13\big), \qquad s_t \mid s_{t-1} \sim \mathrm{Cat}(A\,s_{t-1}), \qquad x_t \mid s_t \sim \mathrm{Cat}(B\,s_t), \quad t = 1, \dots, 5$$

$$A \sim \mathrm{MatrixDirichlet}\begin{pmatrix} 1 & 1 & 1 \\ 1 & 1 & 1 \\ 1 & 1 & 1 \end{pmatrix}, \qquad B \sim \mathrm{MatrixDirichlet}\begin{pmatrix} 10 & 1 & 1 \\ 1 & 10 & 1 \\ 1 & 1 & 10 \end{pmatrix}$$

**Why these priors.** The start $s_0$ is uniform: we do not know the condition before the first shift. $A$ is flat (all ones): no opinion on how conditions move — the data learns it. $B$ is the strong one: each condition mostly shows "its" reading level (10 against 1, about 83 %). That gives the states their meaning — without it the three states are interchangeable, and five readings cannot pin down $A$ and $B$ at once. Dirichlet priors are conjugate to the categorical, so counts add; the updates run mean-field, $q(s)\,q(A)\,q(B)$, three rounds.

**What comes out.** The condition per shift: the model is at least 94 % sure of state 0 in shifts 1–2, state 1 in shift 3 and state 2 in shifts 4–5 — a change step by step, the cue to plan the service. Read it with the prior in mind: with five readings, $B$''s prior largely decides that the states follow the readings. The model only numbers the states; calling them good, worn and faulty is our reading.'::MARKDOWN,
       'What this graph solves'::TITLE;

SELECT 12::SPAN;

SELECT anofox_plot_factor_graph(n,
         edges := (SELECT list(e) FROM fg_hmm_edges e),
         schedule := (SELECT list(s) FROM fg_hmm_schedule s),
         width := 1400, height := 380)
       AS "Which state was the machine in?"
FROM fg_hmm_nodes n;
