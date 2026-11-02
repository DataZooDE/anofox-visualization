-- #845 (M3-3): a slice of the M5 monthly sales (web/m5_monthly.parquet): the
-- first FOODS_1 items in stores CA_1, TX_1 and WI_1 (24 series, monthly).
CREATE TABLE m5 AS
SELECT split_part(series, '_', 1) || '_' || split_part(series, '_', 2) || '_' || split_part(series, '_', 3) AS item,
       split_part(series, '_', 4) || '_' || split_part(series, '_', 5) AS store,
       ds AS month, y AS units
FROM read_parquet('m5_monthly.parquet')
WHERE series LIKE 'FOODS_1_00%' AND (series LIKE '%_CA_1' OR series LIKE '%_TX_1' OR series LIKE '%_WI_1');
