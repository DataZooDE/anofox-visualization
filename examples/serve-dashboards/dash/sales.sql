-- @title Sessions by channel
-- @param region [EU, US, APAC] = EU
-- @refresh 30
--
-- Panel-only dashboard for serve mode: fixed SQL, a whitelisted `region`
-- parameter bound as a DuckDB variable. Consumers pick the region; they never
-- send SQL. Runs read-only.
SELECT channel::XAXIS, sum(n)::BARCHART, 'Sessions by channel'::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY channel ORDER BY channel;
