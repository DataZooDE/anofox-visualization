---
title: Operator checklist
description: What to set before a locked server goes live.
---

- [ ] TLS at the reverse proxy; the server bound to loopback (or a private network) and `hosts` set.
- [ ] A login (`auth`) and a row / column policy — or a deliberate `"policy": "none"`.
- [ ] Data attached `READ_ONLY`; the dashboards folder read-only.
- [ ] Limits: `memory_limit`, `threads`, `timeout_ms`, `max_rows` sized for your data ([sizing](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#7-sizing)).
- [ ] Refresh: a schedule or `anofox_serve_refresh`; an `::ASOF` chip on every dashboard.
- [ ] Monitoring: `/healthz` in the load balancer, `/metrics` scraped, logs kept.
- [ ] `dashboard --check --strict` in CI (the [check-dashboards action](../cli/#in-ci)).
- [ ] Extension telemetry off where it must be (`DATAZOO_DISABLE_TELEMETRY=1`, the default on load).

The full list: [deployment.md → Checklist](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#checklist).
