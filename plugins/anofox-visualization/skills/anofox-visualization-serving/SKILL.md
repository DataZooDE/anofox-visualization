---
name: anofox-visualization-serving
description: >
  Serve and deploy anofox-visualization dashboards from DuckDB:
  anofox_serve(port) (the authoring builder with Explore on the live
  session), anofox_serve_dashboards(dir, port[, options]) (locked,
  read-only serving of a folder of ::ROLE .sql dashboards to consumers),
  anofox_serve_refresh(port), anofox_serve_stop(port), and the standalone
  `serve` binary (authoring on a database file, or `serve --dashboards`
  static server-side rendering with whitelisted -- @param values). Covers
  which mode to pick, the trust model (server-owned SQL, typed variables,
  read-only snapshot, Host allowlist), every option (max_rows, timeout_ms,
  threads, max_queries, max_inflight_mb, max_response_mb, cache_mb,
  memory_limit, duckdb_threads, bind, hosts, search_path, app, home, attach, load,
  refresh_s, data, path_vars, theme), the result cache, single-flight and
  cancelled requests, snapshot refresh, a live data folder, /healthz,
  header comments (-- @title, -- @group, -- @tags, -- @description, -- @owner, -- @certified,
  -- @load, -- @param, -- @vars, -- @requires)
  and deployment (deploy/run-locked.sh, systemd, Docker, reverse proxy with
  TLS, replicas behind a load balancer, sizing). Use when the user wants to
  "share / publish / host / deploy a dashboard", "serve it to my team",
  "put it behind nginx / Caddy", "run it in Docker / Kubernetes", "open
  the builder on my data", asks whether serving is safe, how to refresh
  served data, or gets "nothing is serving on port", "already serving",
  421 / 503 / 413 errors. Login, SSO, row-level security and embedding are
  in the access-control skill. Serving needs the C-API build (release or
  from source; the community build is render-only).
version: 2026.10.18
user-invocable: false
---

# Anofox Visualization — Serving and Deployment

**Extension:** `anofox_visualization` C-API build (a release file, or `duckext/scripts/build-native.sh --release`; the community build has no serving functions) | **Binaries:** `serve`, `dashboard` | **Docs:** `docs/secure-serving.md`, `docs/deployment.md` | **Login / policies:** the access-control skill

## Which mode

| | Authoring | Locked, full UI (A) | Static render (B) |
|---|---|---|---|
| Start | `SELECT anofox_serve(port);` or `serve <db>` | `SELECT anofox_serve_dashboards(dir, port[, options]);` | `serve --dashboards <dir> [--init setup.sql]` |
| For | you, on this machine | consumers, interactive | consumers, static |
| UI | full builder, SQL editor, **Explore data** | the real client, no editor, no Explore | server-side SVG pages |
| Client sends | SQL (`POST /query`) | `{dashboard, panel, vars, page}` (`POST /api/panel`) — **no SQL** | dashboard id + whitelisted `?param=` values |
| Data | the live read-write session / file | a private read-only snapshot (refreshable); optionally one read-only data folder | `duckdb -readonly` |
| Login, policies | loopback + per-server token only | `auth` + row / column `policy` (access-control skill) | `--auth` (no policies) |

Rule of thumb: explore your own data → authoring; hand a dashboard to others → A (interactive) or B (no data API at all). Never expose authoring beyond loopback.

## Functions (extension)

| Function | Behaviour |
|---|---|
| `anofox_serve(port[, options])` | the builder on `http://127.0.0.1:<port>/?token=…` over the **current session** (its tables, attached databases, loaded extensions); opens the browser. `options`: the app bar's JSON — `name`, `tagline` (`""` hides it), `logo`, `link`, `favicon`, `name_visible` (`false` = wordmark logo alone; needs a logo); images as `https://` or `data:image/…` (a file: `'data:image/svg+xml;base64,' \|\| to_base64(content) FROM read_blob('logo.svg')`) |
| `anofox_serve_dashboards(dir, port[, options])` | every `.sql` file of `dir`: `/` lists them, `/d/<id>` opens one; snapshots the session, checks every statement, then binds |
| `anofox_serve_refresh(port)` | rebuilds the snapshot from the session's committed data and swaps it in atomically; returns `… now serves snapshot epoch N` |
| `anofox_serve_stop(port)` | stops that server, deletes its snapshot; `nothing is serving on port N` if none |

A second server on a used port: `already serving on port N`; another process on it: `could not bind …`.

```sql skip
LOAD '/opt/anofox/anofox_visualization.duckdb_extension';      -- duckdb -unsigned
ATTACH '/data/warehouse.duckdb' AS warehouse (READ_ONLY);
SELECT anofox_serve_dashboards('/etc/anofox/dashboards', 8095, '{
  "search_path": "warehouse", "hosts": ["dash.example.com"],
  "threads": 16, "duckdb_threads": 4, "memory_limit": "2GB",
  "cache_mb": 256, "timeout_ms": 10000, "refresh_s": 300}');
SELECT anofox_serve_refresh(8095);   -- fresh data now, no restart
SELECT anofox_serve_stop(8095);
```

## Locked-server options (JSON, third argument)

Unknown keys and bad values fail the start, naming the valid ones.

| Key | Default | Meaning |
|---|---|---|
| `max_rows` | 100,000 | rows per panel; exceeding it is an **error**, not truncation |
| `timeout_ms` | 30,000 | per request (query interrupted); keep the proxy's timeout above it |
| `max_body_bytes` | 64 KiB | request body |
| `threads` | 4 | HTTP workers = requests at once (16–32 is fine: cache hits are cheap) |
| `max_queries` | 4 | panel queries running at once; others wait up to `timeout_ms`, then `503` + `Retry-After` |
| `max_inflight_mb` / `max_response_mb` | 256 / 32 | results held outside DuckDB at once (`503` over it) / one response (`413` over it) |
| `cache_mb` | 64 | per-snapshot result cache; `0` = off (single-flight stays) |
| `memory_limit`, `duckdb_threads` | 80 % RAM, cores | DuckDB settings of the snapshot instance, shared by all queries |
| `bind` | `127.0.0.1` | `"0.0.0.0"` in a container / behind a load balancer on another host (warns without `auth`) |
| `hosts` | loopback names | public names the proxy forwards in `Host` (else `421`); `"*"` disables the check |
| `search_path` | serving default database | where unqualified names resolve: `"warehouse"`, `"warehouse.sales"`, comma-separated. A `USE` before starting does **not** reach the server |
| `app` | none | the landing page's header: `{"name", "tagline", "logo", "favicon"}` (the app-bar settings; logo an https:// or data:image/ URL) |
| `home` | none | a dashboard id: `/` redirects to it instead of the landing page (refuses to start if it doesn't exist) |
| `attach` | `true` | snapshot every attached DuckDB database; `false` = only the current one |
| `load` | `[]` | extensions for the snapshot (`["spatial"]`) |
| `refresh_s` | — | rebuild the snapshot every *n* s |
| `data` | — | one folder the panels may read in place, as `getvariable('anofox_data')`; statements must then be queries, `SET VARIABLE` or `CREATE TEMP TABLE\|VIEW … AS <query>`; mount it read-only |
| `path_vars` | `[]` | variables used as path segments under `data` (plain names `[A-Za-z0-9_.-]`, no leading `.`, else `400`) |
| `theme` | — | default theme for dashboards without `::THEME` |
| `auth`, `policy`, `policy_key_env`, `log`, `log_vars`, `audit` | — | login, access policy and logs: access-control skill |

```sql skip
-- a data folder: one parquet directory per forecast run, picked by a ::DROPDOWN or ?v.run=
SELECT anofox_serve_dashboards('dashboards', 8095, '{"data": "/srv/runs", "path_vars": ["run"]}');
-- in a panel:
SELECT ds::XAXIS, yhat::LINECHART, 'Forecast of the picked run'::TITLE
FROM read_parquet(getvariable('anofox_data') || '/' || getvariable('run') || '/forecast.parquet');
```

## How a locked server behaves

- **Server-owned SQL.** Dashboards are planned at start; each statement is checked with DuckDB's parser. `vars` become typed literals (`SET VARIABLE name = 'EU'`); objects, nested lists and oversize values are `400`. `::PAGED` requests are structural (page, sort, search, column filters); the server builds the SQL with bound parameters. No `/query` (`410`).
- **Read-only snapshot.** The session's DuckDB databases are copied into a private `0700` temp directory and opened `READ_ONLY` in a separate instance with external access, extension loading and configuration locked. Foreign catalogs (Postgres, SQLite, MotherDuck) are not snapshotted: copy what the dashboards need into DuckDB tables first.
- **Per request**: a fresh connection (setup statements re-run; only `TEMP` objects), row cap, timeout.
- **Cache and single-flight**: results are cached per snapshot, keyed by dashboard, panel, every variable and the page (and the viewer's claims with `auth`); identical concurrent requests run once. `now()` / `random()` panels are cached too — `cache_mb: 0` for those.
- **Cancelled requests**: paging or scrolling a `::PAGED` table quickly cancels the superseded query (`409`, `X-Anofox-Supersede`), unless another viewer waits for it.
- **Refresh** (`refresh_s` / `anofox_serve_refresh`): atomic swap, running requests finish on the old snapshot, a new empty cache; a failed rebuild keeps the old one. Needs the snapshot's disk and memory twice while it runs. No HTTP endpoint can trigger it.
- **`GET /healthz`**: `{"status":"ok","dashboards":3,"epoch":1,"snapshot_age_s":4,…, "limits":{…}}` once the snapshot is ready (with `auth`: only `status` without a login).
- **URL state**: `/d/<id>?tab=<name>&v.region=EU` opens a tab with inputs / selections pre-set (values validated like input changes), so other apps and dashboards can link in.

## Deployment (`docs/deployment.md`)

- **Process**: put the start SQL in a file and run `DUCKDB=/opt/anofox/duckdb deploy/run-locked.sh /etc/anofox/serve.sql` — it keeps the DuckDB CLI's stdin open, passes `-unsigned`, exits 1 on a SERVE_SQL error; `ANOFOX_CONTROL=/run/anofox/control` makes a private FIFO for SQL such as `SELECT anofox_serve_refresh(8080);`. The CLI must be the extension's DuckDB version (1.5.6).
- **systemd**: `deploy/anofox-dashboards.service` (DynamicUser, PrivateTmp with `TMPDIR=/var/tmp` — the snapshot belongs on disk, `MemoryMax`, sandboxing).
- **Docker**: `docker build -f deploy/Dockerfile -t anofox-dashboards .`; run with `-p 127.0.0.1:8080:8080 --read-only -v /tmp`, data and dashboards mounted `:ro`, `"bind": "0.0.0.0"`; keep host and container port equal or add the host port to `hosts`.
- **Reverse proxy**: TLS and login at Caddy / nginx in front of a loopback-bound server; forward `Host` and list it in `hosts`.
- **Replicas**: requests are stateless, so N identical servers work behind a load balancer; prefer sticky routing (cancellation, cache and snapshot are per replica); health-check `/healthz` with an accepted `Host`.
- **Checklist**: attach only what the dashboards need; `memory_limit`, `duckdb_threads`, `cache_mb`, `threads`, `max_queries` set and the container / unit memory above them; `$TMPDIR` on disk; every locked dashboard has an `::ASOF` chip.

## Binaries

```sh
serve mydata.duckdb [--port 8080] [--no-open] [--theme dark]          # authoring on a database file
      [--app-name "Acme Insights"] [--app-tagline TEXT] [--app-logo logo.svg|URL] [--app-link URL] [--app-favicon FILE|URL] [--app-hide-name]
serve --dashboards dash/ --init init.sql [--db data.duckdb] [--bind 127.0.0.1] [--port 8080] \
      [--max-rows 100000] [--timeout 30] [--threads 4] [--cache 60] [--theme T] \
      [--allow-host dash.example.com] [--auth @auth.json] [--log stderr] [--audit audit.jsonl]   # static render (B)
dashboard file.sql [out.html] [--theme T] [--dark]                    # one static HTML file, no server
dashboard --svg file.sql [out.svg]                                    # the whole dashboard as one SVG
```

`--init` runs once, read-write, before serving; `--cache <seconds>` is the shared render cache and the freshness knob. Try: `serve --dashboards examples/serve-dashboards/dash --init examples/serve-dashboards/init.sql`, or `examples/served/` for a locked server with cache, refresh and a data folder.

## Dashboard header comments

```sql skip
-- @title Which regions are behind plan?
-- @group Sales / EMEA
-- @tags  plan, regions
-- @description Revenue against plan per region, month by month.
-- @owner Sales controlling
-- @certified 2026-10-15
-- @load spatial
-- @param region [EU, US, APAC] = EU
-- @vars region, selected
-- @refresh 300
SELECT month::XAXIS, sum(revenue)::LINECHART, 'Revenue'::TITLE
FROM sales WHERE region = getvariable('region') GROUP BY ALL ORDER BY month;
```

`@title` page title (a plain business question; provenance goes in other `--` comments, not on the page) · `@group` where it sits (one group, ` / ` for one sub-level; parsed by the servers, shown by the builder's library) · `@tags` labels (lower-cased, ≤10; `--check` warns about an empty or >80-character group and more tags) · `@description` one line under the title on the landing page · `@owner` who looks after it (nav tooltip, `/` list, builder sidebar) · `@certified YYYY-MM-DD` the last check: a ✓ certified badge (a bad date counts as not certified; `--check` warns) · `@load` an extension (identifiers only) · `@param` a static-mode whitelist with its default · `@vars` the variables a locked-server viewer may set · `@refresh` the page refresh interval · `-- @requires <path>` directly above one statement: a missing optional file shows "Not available" instead of an error (above a `::TAB` / `::SUBTAB`: the tab is hidden and the server never runs its panels; above a `::TRACK`: the strip is left out; `a | b` = either; `${name}` = a variable's value; with a data folder the locked server checks it before running: `${name}` only from server-side values — `anofox_data`, `path_vars`, the dashboard's own `SET VARIABLE` — and the expanded path must stay inside the folder: no `..`, `~`, URL or outside absolute path; patterns only in the literal `@requires` text, never from a variable's value; matches through a symlink out of the folder don't count; never guard a setup statement that narrows what a viewer sees with `@requires` — a viewer can skip it).

A served folder groups itself from these headers: `window.__served.nav` items are `{id, title, group, tags, owner, certified}` (only what the viewer's scope allows); the nav shows the ungrouped first, then the groups, and becomes a searchable menu from 9 dashboards on; the `/` landing page (locked server and `serve --dashboards`) shows them as cards grouped the same way (title, description, owner, ✓ certified, updated, tags; search, tag and certified filters from 9 on), headed by `options.app` / `--app-name …` (name, tagline, logo); `options.home` / `--home <id>` makes `/` redirect to one dashboard instead. Lint the whole folder first with `dashboard --check <folder>` (`folder/duplicate-title`, `folder/bad-id`).

## Gotchas

1. **Community build = no serving.** Use the release / from-source C-API build (`duckdb -unsigned`).
2. **The snapshot is taken at start** (committed data only) and changes only on refresh. Heavy setup belongs in tables before serving (setup statements re-run per request). Files under `data` are read live, cached results are not: refresh or `cache_mb: 0` if files change in place.
3. **`421 Misdirected`** behind a proxy: add its public name to `hosts` (`--allow-host` for `serve`). **`503`**: `max_queries` / `max_inflight_mb` reached (the browser retries twice). **`413`**: raise `max_response_mb` or aggregate.
4. **Unqualified names don't resolve** after `USE warehouse`: pass `search_path`, qualify the tables, or create views in the main database.
5. **Inputs, tabs, cross-filter, `::PAGED` and downloads** work in authoring and A; the static `dashboard` HTML and mode B render panels only.
6. **Themes**: `--theme` / `options.theme` is only a default; a dashboard's `::THEME` wins. A `read_text` theme file must be readable when the snapshot is taken. The "Powered by DataZoo GmbH" credit cannot be hidden.
   **App bar** (the builder's name / tagline / logo per installation, not a dashboard's `::BRAND`): `--app-*` flags, `anofox_serve(port, options)`, or `app-config.json` next to `index.html` for static hosting (`web/app-config.example.json`). `--app-logo` files must be PNG/JPEG/GIF/WebP/SVG ≤ 512 KB; `--app-hide-name` needs `--app-logo`. Binaries never serve a `web/app-config.json` (flags / options only). Locked servers have no app bar, so `serve --dashboards` refuses `--app-*`.
7. **Lint before serving**: `dashboard --check dir/file.sql` (dashboards skill), plus `--policy policy.json` with an access policy.
