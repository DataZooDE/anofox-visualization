-- Example SERVE_SQL for run-locked.sh / the container image / the systemd unit.
-- Runs once at startup; the locked server snapshots what this session sees.
LOAD '/opt/anofox/anofox_visualization.duckdb_extension';

-- Your data: attach the production database read-only (the snapshot copies it,
-- so the file can be replaced afterwards). The server snapshots through its own
-- connection, which doesn't see a USE here: qualify the tables in the
-- dashboards (warehouse.sales) or expose them as views in the main database.
ATTACH '/data/warehouse.duckdb' AS warehouse (READ_ONLY);
CREATE VIEW sales AS FROM warehouse.sales;

-- "bind": "127.0.0.1" behind a proxy on the same host; "0.0.0.0" in a container.
SELECT anofox_serve_dashboards('/etc/anofox/dashboards', 8080, '{
  "bind": "127.0.0.1",
  "hosts": ["dash.example.com"],
  "threads": 16,
  "duckdb_threads": 4,
  "memory_limit": "2GB",
  "cache_mb": 256,
  "timeout_ms": 10000
}');
