-- #845 (M3-3): data-quality results (web/data/tabular/): a daily feed with its
-- anomaly statuses, and the checks run over the tables.
CREATE TABLE tq_daily AS FROM read_csv('data/tabular/tq_daily.csv');
CREATE TABLE tq_check_results AS FROM read_csv('data/tabular/tq_check_results.csv');
