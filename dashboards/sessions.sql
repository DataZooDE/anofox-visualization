-- A duckplot dashboard: annotate result columns with ::ROLE casts (Shaper-style).
-- Statements without a role (this CREATE) are setup; the annotated SELECTs below
-- each become a panel. Run:  cargo run --bin dashboard -- dashboards/sessions.sql

CREATE TABLE sessions AS SELECT * FROM (VALUES
  ('W1','app',30),('W1','web',22),('W1','api',12),
  ('W2','app',41),('W2','web',28),('W2','api',15),
  ('W3','app',26),('W3','web',33),('W3','api', 9),
  ('W4','app',48),('W4','web',30),('W4','api',18)
) t(week, channel, n);

-- a heading panel
SELECT 'Weekly sessions'::LABEL;

-- stacked bar: sessions per week, split by channel
SELECT week::XAXIS, channel::CATEGORY, sum(n)::BARCHART_STACKED
FROM sessions GROUP BY ALL ORDER BY week, channel;

-- line: total sessions per week
SELECT week::XAXIS, sum(n)::LINECHART
FROM sessions GROUP BY ALL ORDER BY week;

-- bar: total sessions per channel
SELECT channel::XAXIS, sum(n)::BARCHART
FROM sessions GROUP BY ALL ORDER BY sum(n) DESC;
