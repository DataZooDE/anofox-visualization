-- @title How many returns will reach our warehouse in the next four weeks?
-- @group Logistics & production
-- @tags  returns, forecast, survival, convolution, story, anofox_statistics
-- @description Returns arrive weeks after the sale: a return rate misses sales and storms, a chain of delays does not.
-- @layout story
--
-- A story (#1369, -- @layout story): an online shop's warehouse planner
-- needs the returns of the next four weeks. The usual shortcut (sales × a
-- return rate, shifted by a typical lag) against a chain: sales, then the
-- customer's request, then the parcel's journey, each delay a curve.
-- DRAFT (step 1): the world is synthetic and the chain uses the true delay
-- curves; step 2 fits them with anofox_statistics (Weibull AFT, censored
-- orders) and adds a time-series baseline.
--
CREATE OR REPLACE TEMP TABLE rw_param AS
SELECT DATE '2026-06-02' AS today, 28 AS horizon,
       0.20 AS p_regular, 0.32 AS p_promo,          -- share of units that come back
       1.3 AS ask_shape, 10.0 AS ask_scale,         -- days until the customer asks (Weibull)
       2.2 AS travel_shape, 6.0 AS travel_scale,    -- days the parcel travels (Weibull)
       0.15 AS storm_share;                         -- share of due parcels received on a storm day
CREATE OR REPLACE TEMP TABLE rw_promo AS
FROM (VALUES ('Black Friday', DATE '2025-11-28', DATE '2025-12-01', 3.0, false),
             ('May sale',     DATE '2026-05-21', DATE '2026-05-27', 2.6, false),
             ('June sale',    DATE '2026-06-11', DATE '2026-06-14', 2.2, true)) t(promo, start_day, end_day, lift, planned);
CREATE OR REPLACE TEMP TABLE rw_storm AS
FROM (VALUES ('January storm', DATE '2026-01-19', DATE '2026-01-22', false),
             ('Storm next week', DATE '2026-06-08', DATE '2026-06-10', true)) t(storm, start_day, end_day, what_if);
-- discrete Weibull delays: probability of 1..60 (ask) and 1..30 (travel) days
CREATE OR REPLACE TEMP TABLE rw_ask AS
SELECT k, exp(-pow((k - 1) / ask_scale, ask_shape)) - exp(-pow(k / ask_scale, ask_shape)) AS f
FROM rw_param, range(1, 61) t(k);
CREATE OR REPLACE TEMP TABLE rw_travel AS
SELECT k AS j, exp(-pow((k - 1) / travel_scale, travel_shape)) - exp(-pow(k / travel_scale, travel_shape)) AS f
FROM rw_param, range(1, 31) t(k);
-- noise: a standard normal per (day, stream), from two hashes (Box–Muller)
CREATE OR REPLACE TEMP MACRO rw_z(d, s) AS
  sqrt(-2 * ln((hash(d::VARCHAR || s || 'u') % 1000003 + 1) / 1000004.0))
  * cos(2 * pi() * (hash(d::VARCHAR || s || 'v') % 1000003) / 1000003.0);
-- sales: the plan (expected) and what was sold; after today only the plan is known
CREATE OR REPLACE TEMP TABLE rw_sales AS
WITH d AS (SELECT (DATE '2025-06-01' + INTERVAL (i) DAY)::DATE AS day FROM range(0, 460) t(i))
SELECT d.day,
       800 * CASE dayofweek(d.day) WHEN 6 THEN 1.35 WHEN 0 THEN 1.25 WHEN 1 THEN 0.95 ELSE 1.0 END
           * (1 + 0.15 * sin(2 * pi() * (dayofyear(d.day) - 60) / 365.0))
           * coalesce(max(p.lift), 1) AS planned,
       max(p.promo) AS promo,
       max(p.promo) IS NOT NULL AS on_promo
FROM d LEFT JOIN rw_promo p ON d.day BETWEEN p.start_day AND p.end_day
GROUP BY d.day;
CREATE OR REPLACE TEMP TABLE rw_sales2 AS
SELECT s.*, greatest(0, round(planned * (1 + 0.08 * rw_z(day, 's'))))::INT AS sold,
       CASE WHEN on_promo THEN p_promo ELSE p_regular END AS p
FROM rw_sales s, rw_param;
-- What feeds the chain, per scenario: before today the units sold, from
-- today the sales plan. 'actual' is what really happened (the held-out
-- truth); 'no June sale' takes the planned sale out of the plan; 'storm'
-- adds next week's storm at the warehouse.
CREATE OR REPLACE TEMP TABLE rw_feed AS
SELECT sc.scenario, s.day,
       CASE WHEN sc.scenario = 'actual' OR s.day < r.today THEN s.sold
            WHEN sc.scenario = 'no June sale' AND s.promo = 'June sale' THEN s.planned / pr.lift
            ELSE s.planned END AS units,
       CASE WHEN sc.scenario = 'no June sale' AND s.promo = 'June sale' THEN r.p_regular ELSE s.p END AS p
FROM rw_sales2 s, rw_param r,
     (VALUES ('actual'), ('forecast'), ('storm'), ('no June sale')) sc(scenario)
LEFT JOIN rw_promo pr ON pr.promo = 'June sale';
-- requests: each day's returned units spread over the following days
CREATE OR REPLACE TEMP TABLE rw_asked AS
SELECT f.scenario, (f.day + INTERVAL (a.k) DAY)::DATE AS day, sum(f.units * f.p * a.f) AS asked
FROM rw_feed f, rw_ask a GROUP BY ALL;
-- parcels due at the warehouse: each request spread over its travel days
CREATE OR REPLACE TEMP TABLE rw_due AS
SELECT q.scenario, (q.day + INTERVAL (t.j) DAY)::DATE AS day, sum(q.asked * t.f) AS due
FROM rw_asked q, rw_travel t GROUP BY ALL;
-- the warehouse: on a storm day only storm_share of what is due (and
-- waiting) comes in; the rest arrives over the next days, half a day at a time
CREATE OR REPLACE TEMP TABLE rw_received AS
WITH RECURSIVE base AS (
  SELECT d.scenario, row_number() OVER (PARTITION BY d.scenario ORDER BY d.day) AS n, d.day, d.due,
         EXISTS (SELECT 1 FROM rw_storm s WHERE d.day BETWEEN s.start_day AND s.end_day
                 AND (NOT s.what_if OR d.scenario = 'storm')) AS storm
  FROM rw_due d WHERE d.day BETWEEN DATE '2025-06-02' AND DATE '2026-08-31'),
q AS (
  SELECT scenario, n, day, due, storm,
         CASE WHEN storm THEN due * 0.15 ELSE due END AS received,
         CASE WHEN storm THEN due * 0.85 ELSE 0 END AS waiting
  FROM base WHERE n = 1
  UNION ALL
  SELECT b.scenario, b.n, b.day, b.due, b.storm,
         CASE WHEN b.storm THEN (b.due + q.waiting) * 0.15 ELSE b.due + 0.5 * q.waiting END,
         CASE WHEN b.storm THEN (b.due + q.waiting) * 0.85 ELSE 0.5 * q.waiting END
  FROM base b JOIN q ON b.scenario = q.scenario AND b.n = q.n + 1)
SELECT scenario, day, storm, received AS expected, waiting,
       CASE WHEN scenario = 'actual'
            THEN greatest(0, round(received + sqrt(received) * 1.3 * rw_z(day, 'r')))::INT END AS received
FROM q;
-- the shortcut: last 8 weeks' receipts over the sales two weeks before them,
-- times the sales two weeks earlier
CREATE OR REPLACE TEMP TABLE rw_shortcut AS
WITH rate AS (
  SELECT (SELECT sum(received) FROM rw_received WHERE scenario = 'actual'
            AND day >= today - 56 AND day < today)
       / (SELECT sum(sold) FROM rw_sales2 WHERE day >= today - 70 AND day < today - 14) AS rate
  FROM rw_param)
SELECT (f.day + INTERVAL 14 DAY)::DATE AS day, rate.rate * f.units AS shortcut, rate.rate
FROM rw_feed f, rate WHERE f.scenario = 'forecast';
-- the two delays together: of the units sold on one day, the share asked
-- for and the share arrived k days later (ask, then travel)
CREATE OR REPLACE TEMP TABLE rw_curve AS
WITH arrive AS (
  SELECT a.k + t.j AS k, sum(a.f * t.f) AS f FROM rw_ask a, rw_travel t GROUP BY 1)
SELECT k, coalesce(a.f, 0) AS ask, coalesce(v.f, 0) AS arrive,
       sum(coalesce(v.f, 0)) OVER (ORDER BY k) AS arrived
FROM range(1, 61) r(k) LEFT JOIN rw_ask a USING (k) LEFT JOIN arrive v USING (k);
-- the forecast days: actual (held out), chain, shortcut, and the chain's
-- scenarios; a draft 50 % / 90 % range (count noise) around the chain
CREATE OR REPLACE TEMP TABLE rw_fc AS
SELECT r.day, r.received AS actual, f.expected AS chain, s.shortcut,
       st.expected AS storm, ns.expected AS no_sale,
       greatest(0, f.expected - 1.645 * 1.3 * sqrt(f.expected)) AS lo90, f.expected + 1.645 * 1.3 * sqrt(f.expected) AS hi90,
       greatest(0, f.expected - 0.674 * 1.3 * sqrt(f.expected)) AS lo50, f.expected + 0.674 * 1.3 * sqrt(f.expected) AS hi50
FROM rw_received r
JOIN rw_received f ON f.day = r.day AND f.scenario = 'forecast'
JOIN rw_received st ON st.day = r.day AND st.scenario = 'storm'
JOIN rw_received ns ON ns.day = r.day AND ns.scenario = 'no June sale'
JOIN rw_shortcut s ON s.day = r.day, rw_param p
WHERE r.scenario = 'actual' AND r.day >= p.today AND r.day < p.today + p.horizon;
-- every number the text quotes, in one row
CREATE OR REPLACE TEMP TABLE rw_n AS
SELECT p.today, (p.today + p.horizon - 1)::DATE AS last_day,
       round(sum(fc.chain), -1) AS chain, round(sum(fc.chain) - 1.645 * 1.3 * sqrt(sum(fc.chain)), -1) AS chain_lo,
       round(sum(fc.chain) + 1.645 * 1.3 * sqrt(sum(fc.chain)), -1) AS chain_hi,
       round(sum(fc.shortcut), -1) AS shortcut, round(100 * (sum(fc.chain) / sum(fc.shortcut) - 1)) AS gap_pct,
       sum(fc.actual) AS actual,
       round(avg(abs(fc.chain - fc.actual))) AS mae_chain, round(avg(abs(fc.shortcut - fc.actual))) AS mae_short,
       round(100 * abs(sum(fc.chain) / sum(fc.actual) - 1), 1) AS err_chain,
       round(100 * abs(sum(fc.shortcut) / sum(fc.actual) - 1), 1) AS err_short,
       CASE WHEN sum(fc.chain) > sum(fc.actual) THEN 'over' ELSE 'under' END AS dir_chain,
       CASE WHEN sum(fc.shortcut) > sum(fc.actual) THEN 'over' ELSE 'under' END AS dir_short,
       round(max(abs(fc.shortcut - fc.actual))) AS worst_short,
       round(any_value(s.rate) * 100, 1) AS rate_pct, round(100 * p.p_regular) AS p_regular, round(100 * p.p_promo) AS p_promo,
       (SELECT min(k) FROM rw_curve WHERE arrived >= 0.5 * (SELECT max(arrived) FROM rw_curve)) AS half_days,
       (SELECT min(k) FROM rw_curve WHERE arrived >= 0.9 * (SELECT max(arrived) FROM rw_curve)) AS p90_days,
       (SELECT round(sum(ask * k) / sum(ask)) FROM rw_curve) AS ask_mean,
       (SELECT round(sum(j * f) / sum(f)) FROM rw_travel) AS travel_mean,
       (SELECT round(sum(units * p), -2) FROM rw_feed f JOIN rw_promo pr ON f.day BETWEEN pr.start_day AND pr.end_day
         WHERE f.scenario = 'actual' AND pr.promo = 'May sale') AS may_returns,
       (SELECT round(100 * sum(f.units * f.p * c.arrived) / sum(f.units * f.p))
          FROM rw_feed f JOIN rw_promo pr ON f.day BETWEEN pr.start_day AND pr.end_day
          JOIN rw_curve c ON c.k = p.today - f.day - 1
         WHERE f.scenario = 'actual' AND pr.promo = 'May sale') AS may_in_pct,
       (SELECT round(sum(x.chain) - sum(x.storm), -1) FROM rw_fc x, rw_storm s WHERE s.what_if AND x.day BETWEEN s.start_day AND s.end_day) AS storm_dip,
       round(sum(fc.chain) - sum(fc.no_sale), -1) AS no_sale_total
FROM rw_fc fc, rw_param p, rw_shortcut s WHERE s.day = p.today
GROUP BY p.today, p.horizon, p.p_regular, p.p_promo;

-- ── opening ──────────────────────────────────────────────────────────────
SELECT 'Returns reach the warehouse weeks after the sale. A planner who multiplies sales by a return rate gets the usual weeks right and the unusual ones wrong: after a sale, during a storm, when the mix of buyers changes. This page forecasts returns the way they happen — a sale, then the customer''s request, then the parcel''s journey — and shows where the difference is worth it.

*Draft: a synthetic shop with known truth. The delay curves are still the true ones; fitting them from the data is the next step.*'::MARKDOWN;

SELECT 'The question'::TAB;
SELECT 'It is **{{today:date}}**. The returns desk of an online shop has to plan staff and space for the next four weeks, until **{{last_day:date}}**. How many parcels will come back?

Twelve days ago the shop ended its May sale: seven days at more than twice the usual sales, and deal buyers send more back — about **{{p_promo}} %** of the units against **{{p_regular}} %** in a normal week. In two weeks the June sale follows. Both shape the next four weeks, and the usual forecast sees neither properly.'::MARKDOWN, *
FROM rw_n;
SELECT 'Parcels expected at the returns desk in the next four weeks'::GROUP;
SELECT chain::METRIC, 'chain forecast'::LABEL FROM rw_n;
SELECT shortcut::METRIC, 'shortcut: sales × return rate'::LABEL FROM rw_n;
SELECT gap_pct::PERCENT, 'chain against shortcut'::LABEL FROM rw_n;
SELECT 'end'::ENDGROUP;
SELECT 'The chain expects **{{chain:metric}}** parcels (a 90 % range of {{chain_lo:metric}} to {{chain_hi:metric}}), the shortcut **{{shortcut:metric}}**. In total the two are **{{gap_pct:pct}}** apart. The bigger difference is *when* the parcels arrive — and the days are what a planner staffs for. The rest of this page explains why, and checks both against what really came in.'::MARKDOWN, * FROM rw_n;

SELECT 'Why the shortcut breaks'::TAB;
SELECT 'The shortcut is what most planning sheets do: take the receipts of the last eight weeks, divide by the sales two weeks before them — here **{{rate_pct:pct}}** — and multiply that rate by the sales two weeks before each day to come.

In a regular week that is close enough. Sales are steady, so the returns are steady, and one lag fits them all. It breaks when the sales are not steady. A return is not one fixed lag after its sale: some customers send the parcel back the next day, others after a month, and the parcel spends a few more days on the road. A promotion therefore does not come back as one peak two weeks later, but as a long, low wave.'::MARKDOWN, * FROM rw_n;
SELECT day::XAXIS, sold AS "Units sold" ::BARCHART, 'Units sold per day' ::TITLE,
       p.start_day::XBAND_FROM, p.end_day::XBAND_TO, p.promo::XBAND_LABEL
FROM rw_sales2 s LEFT JOIN rw_promo p ON p.start_day = s.day AND NOT p.planned, rw_param r
WHERE s.day BETWEEN r.today - 112 AND r.today - 1 ORDER BY day;
SELECT r.day::XAXIS, r.received AS "Parcels received" ::LINECHART, 'Parcels received at the warehouse per day' ::TITLE,
       p.start_day::XBAND_FROM, p.end_day::XBAND_TO, p.promo::XBAND_LABEL
FROM rw_received r LEFT JOIN rw_promo p ON p.start_day = r.day AND NOT p.planned, rw_param q
WHERE r.scenario = 'actual' AND r.day BETWEEN q.today - 112 AND q.today - 1 ORDER BY r.day;
SELECT 'The May sale (shaded) more than doubled the units sold for a week. Its returns have only started to arrive: by today about **{{may_in_pct:pct}}** of the roughly {{may_returns:metric}} parcels it will bring back are in. The rest arrives over the next weeks — in exactly the window being planned.'::MARKDOWN, 'caption'::MARKDOWN_STYLE, * FROM rw_n;

SELECT 'One day of sales'::TAB;
SELECT 'To see where the wave comes from, follow one day: **1,000 units** sold on a regular day. About {{p_regular}} % of them — 200 units — will come back, but not at once. Two clocks run one after the other.

The **customer''s clock**: how long until they ask to return. Most ask within the first two weeks, some much later; on average after about **{{ask_mean}} days**. The **parcel''s clock**: how long the parcel travels once they ask, on average about **{{travel_mean}} days**. Put together, half of the 200 parcels are at the warehouse after **{{half_days}} days**, nine in ten after **{{p90_days}} days**.'::MARKDOWN, * FROM rw_n;
SELECT k::XAXIS, 1000 * 0.2 * ask AS "Customer asks" ::LINECHART, 1000 * 0.2 * arrive AS "Parcel arrives" ::LINECHART,
       'Of 1,000 units sold on one day: returns per day after the sale' ::TITLE, 'days after the sale' ::XTITLE
FROM rw_curve ORDER BY k;
SELECT 'The parcel curve is the customer curve pushed right and smoothed by the journey. Every day of sales sends such a wave; the warehouse receives all of them added up.'::MARKDOWN, 'caption'::MARKDOWN_STYLE;
SELECT 'The two clocks matter because they react to different things. A new returns policy or a different kind of buyer changes the customer''s clock. A storm, a strike or a full warehouse changes the parcel''s clock — and it does so on a calendar day, for every parcel on the road that day, whenever it was sold. In January a four-day storm cut receipts to a few dozen a day; the parcels did not disappear, they arrived in the days after.'::MARKDOWN;
SELECT r.day::XAXIS, r.received AS "Parcels received" ::LINECHART, 'Parcels received around the January storm' ::TITLE,
       s.start_day::XBAND_FROM, s.end_day::XBAND_TO, s.storm::XBAND_LABEL
FROM rw_received r LEFT JOIN rw_storm s ON s.start_day = r.day AND NOT s.what_if
WHERE r.scenario = 'actual' AND r.day BETWEEN DATE '2026-01-05' AND DATE '2026-02-08' ORDER BY r.day;

SELECT 'The chain'::TAB;
SELECT 'The forecast follows the same path. Each day''s sales — the units sold so far, the sales plan from today on — is multiplied by its return share and spread forward along the two clocks, and the warehouse receives the sum:'::MARKDOWN;
SELECT '$$\text{receipts}_t \;=\; \sum_{s} \text{sales}_s \cdot p_s \sum_{k} f_{\text{ask}}(k)\, f_{\text{travel}}(t - s - k)$$

$p_s$ is the share returned (higher for promotion days), $f_{\text{ask}}$ and $f_{\text{travel}}$ are the two delay curves. A storm enters as a cap on the receipts of its days, with the rest carried forward. In statistics this is a *survival convolution*: survival models describe each delay, the convolution turns them into a forecast per calendar day.'::MARKDOWN, 'note'::CALLOUT;
SELECT 'Two properties follow directly. Returns of sales made before today are already on their way and are counted with their own clocks — nothing is forgotten because it was sold last month. And any sales forecast can be pushed through the chain, so a planned promotion, or a cancelled one, shows up in the returns without refitting anything.

In this draft the curves are the true ones of the synthetic shop. The next step fits them from the order data with a Weibull model in anofox_statistics, counting the orders that have not come back *yet* as censored instead of as kept — the mistake that makes naive return rates too low.'::MARKDOWN;
SELECT 'Customer asks' AS "Delay", ask_shape AS "Weibull shape", ask_scale AS "Weibull scale (days)" FROM rw_param
UNION ALL SELECT 'Parcel travels', travel_shape, travel_scale FROM rw_param;
SELECT '*The delay curves of the synthetic shop — the truth a fitted model must recover.*'::MARKDOWN;

SELECT 'Against the truth'::TAB;
SELECT 'Because the shop is synthetic, the next four weeks have already happened. Both forecasts were made on {{today:date}} with the same sales plan; the blue line with dots is what really arrived.'::MARKDOWN, * FROM rw_n;
SELECT day::XAXIS, actual AS "Actual" ::LINECHART, chain AS "Chain" ::LINECHART, shortcut AS "Shortcut" ::LINECHART,
       lo90::BAND_LOWER, hi90::BAND_UPPER, lo50::BAND_LOWER, hi50::BAND_UPPER,
       'Parcels per day, next four weeks: forecasts and what arrived' ::TITLE,
       p.start_day::XBAND_FROM, p.end_day::XBAND_TO, p.promo::XBAND_LABEL
FROM rw_fc LEFT JOIN rw_promo p ON p.start_day = rw_fc.day ORDER BY day;
SELECT 'Shaded around the chain: its 50 % and 90 % ranges (draft: day-to-day noise only).'::MARKDOWN, 'caption'::MARKDOWN_STYLE;
SELECT 'Over the four weeks the chain is **{{err_chain:pct}} {{dir_chain}}** the parcels that arrived, the shortcut **{{err_short:pct}} {{dir_short}}**. Day by day the gap is wider: the chain misses by **{{mae_chain:metric}}** parcels on an average day, the shortcut by **{{mae_short:metric}}**, and on its worst day by {{worst_short:metric}}. The shortcut puts the May sale''s returns into one peak two weeks after the sale and then runs short while the long tail is still coming in; the chain spreads them as they really arrive.'::MARKDOWN, * FROM rw_n;
SELECT 'Chain' AS "Method" ::TABLE, chain AS "Parcels, 4 weeks", err_chain AS "Off in total, %", mae_chain AS "Average miss per day" FROM rw_n
UNION ALL SELECT 'Shortcut', shortcut, err_short, mae_short FROM rw_n
UNION ALL SELECT 'What arrived', actual, NULL, NULL FROM rw_n;
SELECT 'A fair comparison needs a third method too: a time-series model on the receipts themselves, given the same sales plan. It comes with the fitted version.'::MARKDOWN;

SELECT 'What if'::TAB;
SELECT 'The chain answers planning questions without refitting: change the input, keep the curves. Two questions the planner gets every week: what does a storm next week do to the desk, and what happens if the June sale is cancelled?'::MARKDOWN;
SELECT scenario::BUTTONS FROM (VALUES ('Storm next week'), ('No June sale')) t(scenario);
SELECT day::XAXIS, chain AS "Plan as it is" ::LINECHART,
       CASE WHEN coalesce(getvariable('scenario'), 'Storm next week') = 'Storm next week' THEN rw_fc.storm ELSE rw_fc.no_sale END AS "With the change" ::LINECHART,
       'Parcels per day: the plan against the change' ::TITLE,
       s.start_day::XBAND_FROM, s.end_day::XBAND_TO, s.storm::XBAND_LABEL
FROM rw_fc LEFT JOIN rw_storm s ON s.start_day = rw_fc.day AND s.what_if
       AND coalesce(getvariable('scenario'), 'Storm next week') = 'Storm next week'
ORDER BY day;
SELECT CASE WHEN coalesce(getvariable('scenario'), 'Storm next week') = 'Storm next week'
  THEN 'During the three storm days the desk receives about **{{storm_dip:metric}}** parcels fewer than planned — and then they come at once. Over the four weeks the total stays the same; the work only moves into the days after the storm. Plan the extra shifts there, not during it.'
  ELSE 'Without the June sale the desk receives about **{{no_sale_total:metric}}** parcels fewer over the four weeks, nearly all of them in the last two — the sale''s returns would only start to arrive in the planning window. The first two weeks stay as they are: they are the May sale''s returns, already on their way.' END ::MARKDOWN, *
FROM rw_n;

SELECT 'What to do'::TAB;
SELECT 'For the planner the answer is short:

- **Staff for the wave, not the rate.** The four-week total is similar either way; the days are not. After a promotion the returns arrive late and long — the chain shows when.
- **Keep two clocks apart.** Customer behaviour and logistics change for different reasons. Fit them separately and each change has its place.
- **Push every plan through it.** A sales forecast, a cancelled sale, a storm warning: each is a new input, not a new model.

**Limits.** The shop here is synthetic, so the model gets the world it was built for. Real returns need the share returned per product group, and the delay curves drift with the season and with policies; refit them monthly and check them against what arrived, as above. When sales are steady all year, the shortcut is good enough — the chain earns its keep where sales are not.'::MARKDOWN;
