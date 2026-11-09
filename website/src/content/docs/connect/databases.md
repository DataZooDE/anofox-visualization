---
title: Databases
description: A DuckDB file, Postgres, MySQL, SQLite and MotherDuck — attached read-only.
---

## A DuckDB database

The authoring server and the locked server take one with `--db`:

```sh
serve my.duckdb                                         # the builder on your database (loopback only)
serve --dashboards dashboards/ --db my.duckdb           # locked serving
```

From SQL, attach more: `ATTACH 'warehouse.duckdb' AS wh (READ_ONLY);`

## Other databases

DuckDB's database extensions attach Postgres, MySQL and SQLite; queries run against them like local tables:

```sql
INSTALL postgres; LOAD postgres;
ATTACH 'dbname=shop host=db.internal user=reader' AS shop (TYPE postgres, READ_ONLY);
SELECT country ::XAXIS, count(*) ::BARCHART FROM shop.public.orders GROUP BY ALL ORDER BY 2 DESC;
```

These run natively (CLI, servers), not in the browser. For a served dashboard prefer copying what it needs into the snapshot over querying the production database per viewer.

## MotherDuck

`ATTACH 'md:my_db';` with a `motherduck_token` in the environment. The token grants the whole database: give the locked server a read-only token, and keep it out of dashboards and URLs. The builder's AI assistant is told an attached remote database exists but is never sent its data.
