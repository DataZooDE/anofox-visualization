-- #244 eval seed: the supply-chain backlog data (web/data/supply-chain/weeks.csv).
CREATE TABLE sc_weeks AS FROM read_csv('data/supply-chain/weeks.csv');
