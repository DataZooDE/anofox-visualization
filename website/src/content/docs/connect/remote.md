---
title: Files over HTTPS, S3 and GCS
description: Read Parquet and CSV straight from object storage or a web server.
---

DuckDB reads remote files directly; Parquet is read selectively (only the columns and row groups a query needs).

```sql
CREATE TABLE trips AS FROM read_parquet('https://example.com/data/trips.parquet');
CREATE TABLE sales AS FROM read_parquet('s3://my-bucket/sales/*.parquet');   -- httpfs + credentials
```

- **Native (CLI, servers):** the `httpfs` extension handles `https://`, `s3://` and `gs://`. Give S3 credentials with a DuckDB `CREATE SECRET` in a setup statement, or let the server's environment provide them.
- **In the browser:** HTTPS works when the file's server sends CORS headers (`Access-Control-Allow-Origin`). Object storage needs a CORS rule for the site's origin.
- **Read once, use often:** a remote file read by several panels is downloaded again by each one in the browser. Read it into a table once — `CREATE TEMP TABLE … AS FROM read_parquet(…)` — rather than a view; `dashboard --check` flags the view (`design/remote-view`).
