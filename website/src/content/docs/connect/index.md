---
title: Where the data comes from
description: A dashboard reads whatever DuckDB reads — local files, files over HTTPS or S3, DuckDB databases, Postgres, MySQL, SQLite, MotherDuck.
---

A dashboard's setup statements are plain DuckDB SQL, so it reads anything DuckDB can read. What changes between the ways to run it is **where that SQL runs**, and therefore which files and networks it can reach.

| Source | In the browser (DuckDB-Wasm) | CLI, authoring server, locked server |
|---|---|---|
| [CSV, Parquet, JSON files](files/) | files you open (*Explore data*), or URLs | local paths, a data folder |
| [Files over HTTPS / S3 / GCS](remote/) | HTTPS URLs whose server allows CORS | yes (`httpfs`) |
| [A DuckDB database](databases/) | — (open its tables as Parquet or CSV) | `serve --db`, `ATTACH … (READ_ONLY)` |
| [Postgres, MySQL, SQLite](databases/#other-databases) | — | DuckDB's database extensions |
| [MotherDuck](databases/#motherduck) | — | `ATTACH 'md:…'` with a token |

On a **locked server** the dashboards read a read-only snapshot of the sources you give it, refreshed on a schedule, never the live database per viewer. See [The locked server](../deploy/locked-server/).
