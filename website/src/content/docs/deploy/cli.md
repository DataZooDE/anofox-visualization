---
title: CLI, HTML and CI
description: Render dashboards to SVG or standalone HTML, and check them on every pull request.
---

The `dashboard` CLI ships with every release.

```sh
dashboard sales.sql out.html            # a standalone, interactive HTML page
dashboard --svg sales.sql sales.svg      # one SVG of the whole dashboard (reports, e-mail)
dashboard --check sales.sql              # structure, SQL and design checks (--strict: warnings fail)
dashboard --describe 'orders.parquet'    # columns, types, ranges: what to chart
dashboard --roles                        # every ::ROLE
```

Queries run through the `duckdb` CLI on your `PATH` (`--data <dir>` points relative paths at a folder).

## In CI

The `check-dashboards` action runs the released `dashboard --check` on a folder of dashboards for every pull request:

```yaml
- uses: DataZooDE/anofox-visualization/.github/actions/check-dashboards@v2026.11.10
  with:
    folder: dashboards
    version: v2026.11.10     # the release the server runs
```

Details: [deployment.md](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md), [DOCS § 2](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/DOCS.md#2-three-ways-to-run-it).
