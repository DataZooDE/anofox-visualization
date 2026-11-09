---
title: Files
description: CSV, Parquet and JSON in the browser, the CLI and on a server — and how a dashboard finds its data folder.
---

## In the browser

Open the [builder](../../app/) and use **Explore data**: pick CSV, Parquet or JSON files from your disk, or paste URLs. They are registered with the in-browser DuckDB under their file names, and Explore writes a first dashboard over them. Nothing leaves your machine.

```sql
CREATE TABLE sales AS FROM read_csv('sales.csv');      -- a file you opened
SELECT region ::XAXIS, sum(revenue) ::BARCHART FROM sales GROUP BY ALL ORDER BY 2 DESC;
```

## With the CLI and the servers

Paths are relative to where the process runs. A dashboard that should work in several places reads its folder from a variable instead of a hard-coded path:

```sql
CREATE TABLE sales AS FROM read_parquet(getvariable('anofox_data') || '/sales/*.parquet');
```

`dashboard --data <dir>`, `dashboard --site … --data <dir|url>` and the locked server's data folder set `anofox_data`. A published static site can list its files so the browser downloads each one once.

[Static sites and data folders →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#c-publish-as-a-static-site--dashboard---site-417)
