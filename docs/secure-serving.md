# Secure serving — trust model

**Goal.** Serve a fixed, live dashboard to *untrusted* consumers: they see
current data, but they cannot run arbitrary SQL or reach anything beyond the
dashboard they were given — while the author keeps a convenient authoring UI
that nobody else can drive.

There are three serving modes. Two run client SQL only for the author; the
other two never accept SQL from the client at all.

| | **Authoring** | **Locked, full UI (A)** | **Static render (B)** |
|---|---|---|---|
| Entry | `anofox_serve(port)` / `serve <db>` | `anofox_serve_dashboards(dir, port[, options])` | `serve --dashboards <dir>` |
| Audience | the author, on this machine | untrusted consumers (interactive) | untrusted consumers (static) |
| UI | full builder + editor | real client, editor removed | server-side SVG only |
| What the client sends | SQL (`POST /query`) | `{dashboard, panel, vars, page}` (`POST /api/panel`) — **no SQL** | dashboard id + whitelisted `?param=` values |
| Database | the live read-write session / file | private read-only snapshot (refreshable), locked config; optionally one data folder | `duckdb -readonly`, locked config |
| Bind | loopback only (enforced) | `127.0.0.1` | `--bind` (default `127.0.0.1`) |
| Access control | loopback `Host`, same-origin, per-server token | same-origin `POST`; put auth/TLS in a proxy | put auth/TLS in a proxy |

Available in the **from-source build** only (`duckext/`, see
[`BUILD.md`](../BUILD.md)); the community extension is render-only.

---

## Authoring — `anofox_serve(port)` and `serve <db>`

Runs whatever SQL the builder sends, against your live data. That is the point,
so the protection is about making sure *only you* can send it:

- **Loopback only.** The extension always binds `127.0.0.1`; the `serve` bin
  refuses a non-loopback `--bind` in authoring mode.
- **Per-server token.** A random 128-bit token is generated at start. The URL
  that is printed (and opened) carries it once — `http://127.0.0.1:8080/?token=…`;
  the server answers with a redirect that stores it in an `HttpOnly;
  SameSite=Strict` cookie (`anofox_token_<port>`). Every request needs the
  cookie or an `X-Anofox-Token` header; otherwise `401`.
- **DNS rebinding.** The `Host` header must be a loopback name for this port
  (`127.0.0.1`, `localhost`, `[::1]`); anything else is `403`.
- **CSRF.** A request whose `Origin` differs from the `Host` it was sent to is
  `403` — including pages on *other* localhost ports (cookies are shared across
  ports, origins are not). A `text/plain` form/fetch POST from a web page
  therefore never reaches the SQL endpoint.
- **No CLI meta commands** (`serve` bin). It executes SQL through the `duckdb`
  CLI; a body (or `--init` script) containing a line that starts with `.` is
  refused, so `.shell`, `.read`, `.output`, … never reach the CLI.
- Body limit 4 MiB; authoring results are capped at 1,000,000 rows.

`SELECT anofox_serve_stop(port)` stops an extension server.

## A. Locked, full UI — `anofox_serve_dashboards(dir, port[, options])`

Serves the real browser client for every `.sql` file in `dir`
(`/` lists them, `/d/<id>` opens one; ids are restricted to `[A-Za-z0-9_.-]`).

**The server owns the SQL.** The dashboards are planned on the server at
startup; the client never sends SQL. To run statement *n* of dashboard *id* it
POSTs

```json
{"dashboard": "sales", "panel": 3, "vars": {"region": "EU"},
 "page": {"limit": 10, "offset": 20, "sort": "Region", "desc": false, "filter": "app"}}
```

- `vars` values must be JSON strings, numbers, booleans, `null` or flat lists of
  those; they become **typed SQL literals** (`'EU'`, `5`, `TRUE`, `['a','b']`) in
  `SET VARIABLE <name> = <literal>`; names must be identifiers. Objects, nested
  lists, NUL bytes and oversize values are rejected — there is no way to turn a
  value into an expression or a subquery.
- `page` (for `::PAGED` tables) is structural: the server builds the
  `COUNT(*)` / `ORDER BY` / `LIMIT` / `OFFSET` query itself (and the statistics,
  column-profile and distinct-values queries); column names are quoted as
  identifiers and the full-text filter and column-filter values are bound as
  **prepared-statement parameters**.
- Unknown fields, dashboards or panels are rejected (`400`/`404`). There is no
  `/query` endpoint (`410`).
- Every statement of every dashboard is checked with DuckDB's own parser at
  startup (exactly one statement each); panels execute as single prepared
  statements.

**Read-only, locked-down snapshot.** At startup the server copies the session's
databases (`COPY FROM DATABASE`) into a **private, randomly named `0700`
directory** and opens them in a **separate DuckDB instance**:

- every attached DuckDB database (not just the current one) is re-attached
  `READ_ONLY` under its original name; non-DuckDB catalogs (Postgres, SQLite, …)
  are *not* snapshotted and are reported as warnings — expose what you need
  through tables in a DuckDB database (pass `'{"attach": false}'` to snapshot only
  the current database);
- extensions loaded in the session, `-- @load <ext>` header lines and
  `options.load` are loaded first;
- then `enable_external_access = false` (no file or network access — `COPY … TO`,
  `read_text`/`read_csv`, `ATTACH`, `INSTALL`, `LOAD` are all refused), extension
  auto-install/auto-load and community extensions are off, and
  `lock_configuration = true`, so no statement can undo any of it;
- on unix the snapshot files are unlinked as soon as they are attached (the open
  handles keep working); the directory is removed when the server stops
  (`anofox_serve_stop`) or a start fails. On Windows the files remain until stop.

**Refreshing the snapshot.** The snapshot is taken at startup and can be
rebuilt while the server runs — no restart, no failed requests:

- `options.refresh_s` rebuilds it every *n* seconds in the background;
  `SELECT anofox_serve_refresh(port)` rebuilds it now (from the session that
  started the server, so it sees that session's *committed* data) and returns
  once the new one serves. Rebuilds never overlap: an on-demand refresh waits
  for a running one, the interval refresher skips its turn.
- The swap is atomic: a request takes the current snapshot when it starts and
  finishes on it; later requests get the new one. The old snapshot (its
  DuckDB instance and its directory) is closed and deleted when its last
  request ends.
- Each snapshot has its own result cache, single-flight and supersession
  table, so nothing is cached, shared or cancelled across snapshots; the
  ETags carry the snapshot's epoch.
- A failed rebuild leaves the current snapshot in service; the error goes to
  the log (and to the caller of `anofox_serve_refresh`).
- `GET /healthz` reports `epoch` (1 at start, +1 per refresh),
  `snapshot_age_s`, `refresh_s`, `refreshing` and `refresh_failed` (no error
  text — `/healthz` is public).
- While a rebuild runs, the old and the new snapshot both exist: plan for twice
  the snapshot's disk and memory.

There is no HTTP endpoint for refreshing: it is a SQL call in the serving
session (or the interval), so no viewer can trigger one.

**A live data folder (`options.data`).** Data that is too large or changes
too often to copy at every refresh — e.g. a folder with one parquet directory
per forecast run — can be read in place:

```sql
SELECT anofox_serve_dashboards('dashboards', 8095,
  '{"data": "/srv/runs", "path_vars": ["run"]}');
```

```sql
-- a panel: the run is picked by a variable (a dropdown, a ?v.run= link, …)
SELECT ds::XAXIS, yhat::LINECHART
FROM read_parquet(getvariable('anofox_data') || '/' || getvariable('run') || '/forecast.parquet');
```

- The folder (canonicalised: no `..` or symlink in the configured path) is
  the only path the snapshot instance may open — DuckDB's
  `allowed_directories` with `enable_external_access = false` and
  `lock_configuration = true`. DuckDB refuses any path outside it, including
  `…/run/../../etc`.
- **Keep it narrow.** Every viewer can read every file in it. The server
  refuses `/`, top-level system directories (`/etc`, `/home`, `/tmp`, `/usr`,
  `/var`, …) and any folder that contains the home directory or the temp
  directory (where the snapshots live); point it at the folder that holds the
  served data and nothing else.
- Each request gets `SET VARIABLE anofox_data = '<folder>'` after the viewer's
  variables; a viewer can't set `anofox_data` (`400`).
- `path_vars` lists the variables whose values become path segments: each
  value must be a plain name — `[A-Za-z0-9_.-]`, not starting with `.` (so not
  `.`, `..` or a hidden file), no `/` — a non-negative integer, or `null`;
  anything else is `400` before any SQL runs.
- **Read-only — make it so in the OS.** DuckDB has no read-only form of
  `allowed_directories`: a statement that writes (`COPY … TO`, `EXPORT`,
  `ATTACH`, …, or anything that runs one: `EXPLAIN ANALYZE`,
  `PREPARE`/`EXECUTE`) could write into the folder. The **hard guarantee** is
  the operating system's: mount the folder read-only into the server
  (`-v /srv/runs:/srv/runs:ro`, a read-only bind mount) or give the server's OS
  user read permission only.
- On top of that, with `data` set the server only starts if every dashboard
  statement is classified as harmless by DuckDB's own parser
  (`json_serialize_sql`, which parses without running and accepts only
  `SELECT` statements): panels must be queries (`SELECT`, `WITH`, `FROM`,
  `VALUES`, set operations; a `PIVOT` only as a subquery with its `IN (…)`
  values listed), and setup statements must be `SET VARIABLE <name> =
  <expression>` or `CREATE [OR REPLACE] TEMP TABLE|VIEW <name> AS <query>`
  with a query that passes the same check. Everything else — `EXPLAIN`,
  `PREPARE`, `EXECUTE`, `CALL`, `PRAGMA`, `COPY`, `INSERT`, macros, … — fails
  the start. The check needs DuckDB's `json` extension (loaded into the
  snapshot automatically; without it the server refuses to start).
- Error messages sent to viewers show the folder as `<data>`; the log keeps
  the full path.
- Files appear live: a new run is readable as soon as it is written, without a
  refresh. The result cache does *not* see file changes — runs that are
  rewritten in place need a refresh (which starts a new cache) or
  `cache_mb: 0`.
- Views in the session over files in the folder (`CREATE VIEW v AS SELECT …
  FROM read_parquet('/srv/runs/…')`) are copied into the snapshot as views and
  read the files live too.
- Every viewer can read every file in the folder (by any `path_vars` value):
  serve one folder per audience. (DuckDB 1.5 resolves symlinks, so a link
  inside the folder that points outside it is refused too.)

**Per-request isolation and limits.**

- A fresh connection per request: one viewer's variables or temp objects never
  leak into another's. Setup statements (`CREATE TEMP …`) of the dashboard are
  re-run per request; only TEMP objects can be created (the data is read-only),
  so materialise anything heavy before serving.
- Options (third argument, JSON): `max_rows` (default 100,000 — exceeding it is
  an error, not a silent truncation), `timeout_ms` (default 30,000 — enforced
  with `duckdb_interrupt` from a watchdog), `max_body_bytes` (default 64 KiB),
  `threads` (worker pool, default 4), `cache_mb` (see below), `attach`, `load`,
  `hosts` (see below), `refresh_s`, `data`, `path_vars` (see above).
- **DNS rebinding.** Only requests whose `Host` is a loopback name on the
  server's port (`127.0.0.1`, `localhost`, `[::1]`) are served; anything else
  gets `421`. Behind a reverse proxy that forwards the original `Host` (Caddy
  does by default), list the public name: `{"hosts": ["dash.example.com"]}`
  (a name without a port matches any port; `"*"` disables the check).
- Cross-origin `POST`s are refused (CSRF).

**Result cache and single-flight.** A snapshot never changes, so a panel's
result is a pure function of its request. Results are cached per snapshot (a
refresh starts an empty cache), keyed by the parsed request — dashboard, panel, every variable value
(in a canonical order) and the `page` object — so a different value is a
different entry and nothing is shared across dashboards or servers. Identical
requests that arrive while one is running wait for it instead of running again
(errors are shared with them, but never cached).

- `cache_mb` (default 64; `0` turns the cache off — the single-flight stays):
  least-recently-used entries are evicted beyond it; one result may take at most
  a quarter of the cache.
- Responses carry an `ETag` (a random per-server id, the snapshot epoch and a
  digest of the key).
- Panels that read the clock or random numbers (`now()`, `random()`) are cached
  like any other: their first result is served until the next refresh.
  Use `cache_mb: 0` for those.
- A cache hit answers faster than a query, so a viewer can tell whether some
  other viewer requested exactly the same panel + values recently. All viewers
  can request every value anyway; if even that matters, use `cache_mb: 0`.

**Superseded requests.** A viewer who pages or scrolls a `::PAGED` table quickly
would otherwise leave a query running per step. The client sends
`X-Anofox-Supersede: <viewer>/<panel>/<kind>` (the viewer id is 128 random bits
generated by the page) and aborts its own previous request of that panel + kind;
the server cancels the older request with the same header value (`409`) and
interrupts its query (`duckdb_interrupt`) — unless another viewer is waiting for
the same query, which then keeps running for them. A newer identical request
(an impatient double click) takes the running query over instead of restarting
it. Other viewers can't cancel a request without its viewer id. A malformed
header is `400`.

- The injected page config is script-safe JSON (`<` is emitted as `<`),
  titles and ids are HTML-escaped.

**Try it:** [`examples/served/`](../examples/served/) serves two dashboards of
a made-up bakery chain with the result cache, interval and on-demand refresh,
and a data folder of forecast runs picked by a `run` variable, and has a smoke
test for all of it (`node examples/served/smoke.mjs`).

```sql
SELECT anofox_serve_dashboards('dashboards', 8095,
  '{"max_rows": 50000, "timeout_ms": 10000, "threads": 8, "cache_mb": 256,
    "refresh_s": 300}');
SELECT anofox_serve_refresh(8095);   -- fresh data now, without a restart
SELECT anofox_serve_stop(8095);
```

## B. Static render — `serve --dashboards <dir>`

The most locked-down option: **no client data API at all**, server-side SVG only.
Consumers pick a dashboard id and whitelisted params
(`-- @param region [EU, US] = EU`; a value outside the list is `400`).

- Panels run `duckdb -readonly`; after the server-declared `-- @load`s, each
  session sets `enable_external_access = false` and `lock_configuration = true`.
- `--max-rows` (default 100,000), `--timeout` seconds (default 30; the CLI is
  killed), `--threads` (default 4), `--cache <seconds>` (shared render cache,
  doubles as the freshness knob).
- `--init setup.sql` runs once, read-write, before serving (attach sources,
  create views/tables). CLI dot-commands are refused there too.

Try it: `serve --dashboards examples/serve-dashboards/dash --init
examples/serve-dashboards/init.sql` → `http://127.0.0.1:8080/`.

![serve mode — sales dashboard with a whitelisted region param](img/serve-sales.png)

---

- **DNS rebinding.** The `Host` must be a loopback name on the server's port,
  the `--bind` address, or a name given with `--allow-host NAME` (repeatable;
  `*` disables the check); anything else gets `421`. Behind a proxy that
  forwards the original `Host`, pass its public name with `--allow-host`.

## What remains the operator's job

- **Authentication + TLS** for consumer-facing modes: terminate both at a reverse
  proxy (nginx / Caddy / Cloudflare) in front of the server; bind the server to
  loopback and let only the proxy reach it.
- **Least privilege on the data**: the snapshot contains everything in the
  session's DuckDB databases, and every file in `options.data` is readable. Serve from a session (or `--db`) that only holds
  the tables/views the dashboards need.
- **Variable values are not whitelisted in mode A**: a viewer can set any
  identifier-named variable to any literal. Panels see them only through
  `getvariable()`, so write panel SQL that treats variables as untrusted
  *values* (they cannot change the query's structure).
- **Process isolation**: run the server as an OS user without access to more
  than it needs.
