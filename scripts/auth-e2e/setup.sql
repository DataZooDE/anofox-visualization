CREATE OR REPLACE TABLE orders AS
SELECT r.region, c.channel, (abs(hash(r.region || c.channel)) % 60 + 20) AS revenue
FROM (VALUES ('EU'),('US'),('APAC')) r(region), (VALUES ('web'),('app')) c(channel);
-- who may see which region (the entitlement table the panels join on)
CREATE OR REPLACE TABLE entitlements AS SELECT * FROM (VALUES
  ('alice@example.com','EU'), ('bob@example.com','US'), ('bob@example.com','APAC')) t(viewer, region);
INSERT INTO entitlements VALUES ('acme-viewer', 'EU');
INSERT INTO entitlements VALUES ('svc-etl', 'US');
