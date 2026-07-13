-- One-time, read-write setup for `serve --dashboards … --init this-file`.
-- In production this is where you ATTACH your live source and expose read-only
-- VIEWS the dashboards read from — e.g.:
--   ATTACH 'md:my_db' AS live;                     -- MotherDuck
--   ATTACH '' AS pg (TYPE postgres, ...);          -- PostgreSQL
--   CREATE VIEW sales AS SELECT region, channel, n FROM live.sales;
-- Here we just seed a demo table.
CREATE OR REPLACE TABLE sales AS SELECT * FROM (VALUES
  ('EU','app',30),('EU','web',22),('EU','api',12),
  ('US','app',41),('US','web',28),('US','api', 9),
  ('APAC','app',18),('APAC','web',25),('APAC','api',14)
) t(region, channel, n);

-- A monthly series for the live-forecast dashboard.
CREATE OR REPLACE TABLE ts AS
  SELECT 'a' AS series, DATE '2022-01-01' + INTERVAL (i) MONTH AS ds,
         round(50 + 10*sin(i/2.0) + i, 1) AS y
  FROM range(0, 24) t(i);
