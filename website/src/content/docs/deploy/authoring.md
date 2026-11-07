---
title: Authoring on your data
description: The builder wired to a live DuckDB — for you, on your machine.
---

```sql
-- in DuckDB, with the full toolkit loaded
SELECT anofox_serve(8080);      -- the builder at http://127.0.0.1:8080, on this session
```

or from the shell: `serve warehouse.duckdb`.

The builder then runs its SQL against your live, read-write session: every table you can see, the SQL editor, *Explore data*. It binds to loopback and protects itself with a per-server token. **Never put it behind a public proxy** — anyone who reaches it can run any SQL. To hand dashboards to others, use the [locked server](../locked-server/).

Details: [Authoring](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#authoring--anofox_serveport-and-serve-db).
