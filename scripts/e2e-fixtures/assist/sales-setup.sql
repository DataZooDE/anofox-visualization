-- #244 fixture: the data part of examples/dashboards/sales-overview.sql
-- (month, region, channel, price, units) — 36 months x 3 regions x 3
-- channels = 324 rows. Shared by tests/web/assist_preview_test.mjs and
-- scripts/assist-e2e.mjs so the SQL and the e2e drive the same data.
CREATE OR REPLACE TABLE sales AS
SELECT (DATE '2023-01-01' + to_months(m::INTEGER)) AS month,
       r.region, c.channel, c.price,
       GREATEST(0, (400 + 15*m + 50*sin(2*pi()*(m%12)/12.0)
                    + ((hash(r.region||c.channel||m::VARCHAR)%80)::INTEGER - 40)))::INTEGER AS units
FROM (VALUES ('EU'),('US'),('APAC')) r(region),
     (VALUES ('web',39.0),('app',29.0),('store',49.0)) c(channel,price),
     range(0,36) t(m);
