-- SERVE_SQL of the compose demo: the load-test reference data and dashboard.
LOAD '/opt/anofox/anofox_visualization.duckdb_extension';
.read /etc/anofox/example/setup.sql
SELECT anofox_serve_dashboards('/etc/anofox/example/dash', 8080, '{
  "bind": "0.0.0.0", "hosts": ["localhost"],
  "threads": 16, "duckdb_threads": 4, "memory_limit": "1GB", "cache_mb": 128
}');
