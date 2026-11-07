---
title: The locked server
description: Serve dashboards to others — a read-only snapshot, no SQL from the client, login and row policies, refresh, replicas.
---

The locked server is a thread pool inside a DuckDB process. A short SQL script starts it:

```sql
LOAD '/opt/anofox/anofox_visualization.duckdb_extension';
ATTACH '/data/warehouse.duckdb' AS warehouse (READ_ONLY);
CREATE VIEW sales AS FROM warehouse.sales;
SELECT anofox_serve_dashboards('/etc/anofox/dashboards', 8080, '{
  "bind": "127.0.0.1", "hosts": ["dash.example.com"],
  "threads": 16, "duckdb_threads": 4, "memory_limit": "2GB", "cache_mb": 256,
  "timeout_ms": 10000
}');
```

## What makes it safe to share

- Readers get the real client but **send no SQL**: a request names a dashboard, a panel and its variables (`POST /api/panel`); the server runs the panel's own SQL.
- Queries run on a **private read-only snapshot** of the session, in a locked-down DuckDB: no file or network access, configuration locked.
- **[Login and row / column policies](../security/)** decide who sees which rows.
- Limits on threads, memory, rows, time and request size, a `Host` allow-list, a strict CSP.

## Running it

| | How | Details |
|---|---|---|
| One process | systemd unit, `run-locked.sh` | [deployment.md § 2](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#2-one-process) |
| Container | `deploy/Dockerfile` (debian-slim, tini, unprivileged) | [§ Container](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#container) |
| Reverse proxy | TLS, authentication headers, `Host` (nginx, Caddy, Traefik) | [§ 3](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#3-reverse-proxy-tls-authentication-host) |
| Several replicas | behind a load balancer; one writer for state and write-back | [§ 4](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#4-several-replicas-behind-a-load-balancer) |
| Fresh data | `refresh` on a schedule, `anofox_serve_refresh(port)`, or a Quack source (prototype) | [§ 5](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#5-refreshing-the-data) |
| New dashboards | swap the folder, `anofox_serve_reload(port)` | [§ 5a](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#5a-deploying-dashboard-changes-772) |
| Limits and sizing | memory, threads, measured throughput | [§ 6–7](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#6-resource-limits) |
| Monitoring | `/healthz`, Prometheus `/metrics`, structured logs | [§ 8](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#8-monitoring) |

Also on the locked server: per-viewer state (`::SUBMIT`), shared write-back (`::SAVE`), saved views, notes, scheduled reports and alerts by mail / Slack / Teams / webhook, and *Ask* (off by default).

## Server-rendered pages only

`serve --dashboards <dir>` renders SVG pages on the server with no data API at all — for readers who need no interaction. [Details](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#b-static-render--serve---dashboards-dir).
