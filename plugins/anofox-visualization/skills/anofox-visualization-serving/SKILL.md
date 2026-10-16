---
name: anofox-visualization-serving
description: >
  Serve anofox-visualization dashboards from DuckDB: anofox_serve(port) (the
  authoring builder wired to the live session), anofox_serve_dashboards(dir,
  port[, options]) (locked, read-only serving of a folder of ::ROLE .sql
  dashboards to consumers), anofox_serve_refresh(port),
  anofox_serve_stop(port), and the standalone
  `serve` binary (authoring on a database file, or `serve --dashboards`
  static server-side rendering with whitelisted -- @param values) plus the
  `dashboard` CLI renderer. Covers the trust model (loopback bind,
  per-server token cookie, Host / Origin checks against DNS rebinding and
  CSRF, server-owned SQL with typed variable binding, read-only snapshot with
  external access, extension loading and configuration locked), the options
  and limits (max_rows, timeout_ms, max_body_bytes, threads, attach, load,
  refresh_s, data, path_vars;
  --max-rows, --timeout, --threads, --cache, --init, --bind), header
  comments (-- @title, -- @load, -- @param), refreshing data, and which mode
  to pick. Use when the user wants to "share / publish / host a dashboard",
  "serve it to my team", "open the builder on my data", asks whether serving
  is safe, or gets "nothing is serving on port" / "already serving" errors.
  Serving needs the from-source C-API build (the community build is
  render-only).
version: 2026.10.15
user-invocable: false
---

# Anofox Visualization — Serving Cheat Sheet

**Extension:** `anofox_visualization` from-source C-API build (`duckext/scripts/build-native.sh --release`; the community build has no serving functions) | **Binaries:** `serve`, `dashboard` (core crate) | **Docs:** `docs/secure-serving.md`

## Which mode

| | Authoring | Locked, full UI (A) | Static render (B) |
|---|---|---|---|
| Start | `SELECT anofox_serve(port);` or `serve <db>` | `SELECT anofox_serve_dashboards(dir, port[, options]);` | `serve --dashboards <dir> [--init setup.sql]` |
| For | you, on this machine | untrusted consumers, interactive | untrusted consumers, static |
| UI | full builder + SQL editor | the real client, editor removed | server-side SVG pages |
| Client sends | SQL (`POST /query`) | `{dashboard, panel, vars, page}` (`POST /api/panel`) — **no SQL** | dashboard id + whitelisted `?param=` values |
| Data | the live read-write session / file | a private read-only snapshot, refreshable while serving; optionally one read-only data folder | `duckdb -readonly` |
| Bind | `127.0.0.1` (enforced) | `127.0.0.1` | `--bind` (default `127.0.0.1`) |
| Access control | loopback `Host`, same-origin, per-server token | same-origin `POST`; auth / TLS in a reverse proxy | auth / TLS in a reverse proxy |

Rule of thumb: explore your own data → authoring; hand a fixed dashboard to others → A (interactive) or B (most locked down, no data API at all). Never expose authoring mode beyond loopback.

## Functions (extension)

| Function | Returns / behaviour |
|---|---|
| `anofox_serve(port INTEGER)` | starts the builder on `http://127.0.0.1:<port>/?token=…`, opens the browser, returns a message with the URL. Panels query the **current session** (its tables, attached databases, loaded extensions) |
| `anofox_serve_dashboards(dir VARCHAR, port INTEGER[, options VARCHAR])` | serves every `.sql` file of `dir`: `/` lists them, `/d/<id>` opens one (ids `[A-Za-z0-9_.-]`); returns `anofox-visualization serving N locked, read-only dashboard(s) at http://127.0.0.1:<port>/ — server owns the SQL` |
| `anofox_serve_refresh(port INTEGER)` | rebuilds a locked server's snapshot from the session's committed data and swaps it in atomically (running requests finish on the old one); returns `… now serves snapshot epoch N` |
| `anofox_serve_stop(port INTEGER)` | stops that server (and deletes a locked server's snapshot); `nothing is serving on port N` if there is none |

Starting a second server on a used port fails with `anofox-visualization is already serving on port N`; another process on the port gives `could not bind 127.0.0.1:N: …`.

```sql skip
LOAD '/tmp/anofox_visualization.duckdb_extension';      -- duckdb -unsigned
CREATE TABLE sales AS SELECT * FROM read_parquet('sales.parquet');

SELECT anofox_serve(8080);           -- authoring builder on the live session
SELECT anofox_serve_stop(8080);

SELECT anofox_serve_dashboards('dashboards', 8095,
  '{"max_rows": 50000, "timeout_ms": 10000, "threads": 8}');
SELECT anofox_serve_refresh(8095);   -- fresh data now, no restart
SELECT anofox_serve_stop(8095);      -- stop + delete the snapshot
```

### Locked-server options (JSON, third argument)

| Key | Default | Meaning |
|---|---|---|
| `max_rows` | 100,000 | rows per panel; exceeding it is an **error**, not a silent truncation |
| `timeout_ms` | 30,000 | per request, enforced with `duckdb_interrupt` |
| `max_body_bytes` | 64 KiB | request body limit |
| `threads` | 4 | worker pool |
| `attach` | `true` | snapshot every attached DuckDB database; `false` = only the current one |
| `load` | `[]` | extensions to load in the snapshot instance (`["spatial"]`) |
| `refresh_s` | — | rebuild the snapshot every *n* seconds and swap it in atomically (new epoch, empty result cache); `/healthz` shows `epoch` + `snapshot_age_s` |
| `data` | — | a folder the panels may read in place (DuckDB `allowed_directories`; nothing else on disk); panels get its path as `getvariable('anofox_data')`; every statement must then be a query, `SET VARIABLE x = <expr>` or `CREATE TEMP TABLE\|VIEW x AS <query>` (checked with DuckDB's parser, needs the json extension); not `/`, system dirs, or a folder containing `$HOME` / the temp dir; mount it read-only for a hard guarantee |
| `path_vars` | `[]` | variables used as path segments under `data` (`getvariable('run')`): values must be plain names `[A-Za-z0-9_.-]`, no leading `.`, else `400` |
| `theme` | — | default theme for dashboards without their own `::THEME`: a name (`"dark"`, `"acme+dark"`) or a theme JSON object; validated and frozen at start |

Unknown keys are an error naming the valid ones.

## Trust model

**Authoring** (`anofox_serve`, `serve <db>`): runs whatever SQL the builder sends, so it is locked to you.

- Loopback only; a random 128-bit token in the printed URL is moved into an `HttpOnly; SameSite=Strict` cookie (`anofox_token_<port>`); every request needs it (or `X-Anofox-Token`), else `401`.
- `Host` must be a loopback name for this port (DNS rebinding → `403`); an `Origin` that differs from the `Host` is `403` (CSRF, including other localhost ports).
- `serve` bin: lines starting with `.` (CLI meta commands such as `.shell`) are refused. Body limit 4 MiB, results capped at 1,000,000 rows.

**Locked (A)**:

- **The server owns the SQL.** Dashboards are planned at startup, each statement checked with DuckDB's parser; the client only names panel *n* of dashboard *id*.
- **Variables are typed literals**: `vars` values must be strings, numbers, booleans, `null` or flat lists of those and become `SET VARIABLE name = <literal>`; names must be identifiers. Objects, nested lists, NUL bytes and oversize values are rejected. `::PAGED` requests are structural: page, sort, search, column filters (`eq`, `contains`, `gt`, `ge`, `lt`, `le`), a statistics or distinct-values request; the server builds the SQL itself (`serving::page_sql`, the same builder the browser uses) with every value a bound parameter and every column a quoted identifier. An empty multi-select list is sent as `[]::VARCHAR[]`.
- **Read-only snapshot**: at start the session's DuckDB databases are copied (`COPY FROM DATABASE`) into a private `0700` temp directory and re-attached `READ_ONLY` in a separate DuckDB instance; then `enable_external_access = false` (no files, network, `ATTACH`, `INSTALL`, `LOAD`), extension autoload off and `lock_configuration = true`. Non-DuckDB catalogs (Postgres, SQLite, …) are not snapshotted (reported as warnings): materialise what the dashboards need into DuckDB tables first.
- **Per request**: a fresh connection (no state leaks between viewers), setup statements re-run (only `TEMP` objects can be created), row cap and timeout. No `/query` endpoint (`410`); unknown fields / dashboards / panels are `400` / `404`.

**Static (B)** (`serve --dashboards`): panels run in `duckdb -readonly` with external access off and configuration locked after the declared `-- @load`s; only whitelisted `-- @param` values are accepted (`400` otherwise).

**Still the operator's job**: authentication and TLS (reverse proxy such as nginx / Caddy in front of a loopback-bound server), least privilege on the data (the snapshot contains everything in the session's DuckDB databases), treating variables as untrusted *values* in panel SQL (mode A does not whitelist them), running as an unprivileged OS user.

## Binaries

```sh
cargo build --release --features serve --bin serve      # needs web/pkg (wasm-pack) for the UI
serve mydata.duckdb [--port 8080] [--no-open] [--theme dark]   # authoring on a database file
serve --dashboards dash/ --init init.sql \
      [--db data.duckdb] [--bind 127.0.0.1] [--port 8080] \
      [--max-rows 100000] [--timeout 30] [--threads 4] [--cache 60] [--theme T]   # static render (B)
cargo run --bin dashboard -- file.sql [out.html] [--theme T] [--dark]   # one static HTML file, no server
cargo run --bin dashboard -- --svg file.sql [out.svg] [--theme T] [--dark]   # the whole dashboard as one SVG
```

- `--init setup.sql` runs once, read-write, before serving (attach sources, create views).
- `--cache <seconds>` is a shared render cache and the freshness knob.
- Try it: `serve --dashboards examples/serve-dashboards/dash --init examples/serve-dashboards/init.sql`.

## Dashboard header comments

```sql skip
-- @title Sales by region
-- @load spatial
-- @param region [EU, US, APAC] = EU
-- @refresh 300
SELECT month::XAXIS, sum(revenue)::LINECHART, 'Revenue'::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY ALL ORDER BY month;
```

`@param` whitelists a static-mode parameter (default after `=`); `@load` names an extension to load before serving (identifiers only; also honoured by the locked server); `@title` and `@refresh` set the page title and refresh interval.

## Gotchas

1. **Community build = no serving.** `anofox_serve*` exist only in the C-API build (`duckdb -unsigned`, `LOAD '/path/anofox_visualization.duckdb_extension'`).
2. **The locked snapshot is taken at start** and rebuilt only by `refresh_s` / `anofox_serve_refresh(port)` (uncommitted rows are not seen). A rebuild briefly needs the snapshot's space twice. Heavy setup belongs in tables before serving (setup statements re-run per request). Files under `data` are read live, but cached results are not: refresh (or `cache_mb: 0`) if files change in place.
3. **Attach DuckDB, not foreign catalogs**, for locked serving: copy Postgres / SQLite / MotherDuck data into local DuckDB tables first.
4. **Inputs, tabs, cross-filter and downloads** need the browser UI (authoring or A); the static `dashboard` HTML and mode B render panels only.
5. **Themes in served pages.** `--theme T` / `options.theme` is only a default — a dashboard's own `::THEME` wins. Viewers can switch light/dark (`?theme=dark`, the 🌙 toggle) only when the theme follows the viewer; a URL never injects a theme, and `?locale=` is ignored in served/locked mode. A `read_text` theme file must be readable when the locked snapshot is taken (or pass the JSON in `options.theme`). The "Powered by DataZoo GmbH" credit is shown on every page and cannot be hidden.
6. **Lint before serving**: `dashboard --check dir/file.sql` (dashboards skill) catches panels that would be blank or fail for every viewer.
