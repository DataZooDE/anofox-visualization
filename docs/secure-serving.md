# Secure multi-user serving — design & plan

**Goal.** Serve a fixed, live dashboard to *untrusted* consumers: they see
current data, but they cannot run arbitrary SQL or reach anything beyond the
dashboard they were given.

This document is the plan to get from today's single-developer serving to that.

## Status (v0.1 — first cut shipped)

The **serve mode** exists: `serve --dashboards <dir> [--init setup.sql]`.

- ✅ **Server owns the SQL** — dashboards are `.sql` files loaded server-side; the
  client selects a dashboard by id and parameter *values* only. No `/query`, no
  client SQL. (steps 1, 2, 4)
- ✅ **Whitelisted params** — declared per dashboard (`-- @param region [EU, US]
  = EU`); a value outside the list is rejected with `400`. (step 3)
- ✅ **Read-only queries** — every panel query runs `duckdb --readonly`. (step 5,
  partial)
- ✅ **Result caching** — `--cache <seconds>` caches each rendered
  (dashboard + resolved params) view. Within the TTL, N viewers of the same view
  share one render (no extra DB load), and the TTL is the freshness knob (data
  changes appear after it). Off by default. (step 6, caching)
- ⬜ **Full capability lockdown** — a dedicated read-only role / disabling
  `ATTACH`/file reads at the session level is still the operator's job via
  `--init` + DuckDB config. (step 5, rest)
- ⬜ **Auth + TLS** — deployment; put a reverse proxy in front. (step 6, rest)

Try it: `serve --dashboards examples/serve-dashboards/dash --init
examples/serve-dashboards/init.sql` → open `http://127.0.0.1:8080/`.

The authoring mode (embedded builder + free-form `/query`) is unchanged and
localhost-only.

---

## Where we are today (v0 — authoring mode)

`SELECT anofox_serve(port)` starts an in-process HTTP server that:

- serves the **full builder** (with the SQL editor), and
- exposes `POST /query`, which runs **whatever SQL the client sends** against the
  live DuckDB session.

This is great for the author on `localhost`, and we keep it — but renamed in
intent to **admin / authoring mode**. It is *not* safe to expose to untrusted
consumers: the client controls the SQL. "View-only" (`?embed=1`) only hides the
editor in the UI; it is presentation, not an access boundary.

---

## Target (v1 — serve mode)

Three changes, in priority order.

### 1. The server owns the SQL (the architectural flip)

The single most important change. The client stops sending SQL; it selects a
**dashboard** and supplies **whitelisted parameters**.

- **Register dashboards server-side.** A dashboard is
  `{ id, title, panels: [ { id, sql_template, params } ], refresh }`. Each
  `sql_template` is the annotated (`::ROLE`) SQL, fixed at registration, with
  only typed `:param` placeholders.
  - Source: a directory of `.sql` files loaded at startup, and/or
    `CALL anofox_register_dashboard('sales', $$ … $$)`.
  - The server parses/plans each template **once**.
- **Endpoints (no SQL crosses the wire):**
  - `GET  /d/<id>` → the view-only dashboard shell (no editor).
  - `POST /d/<id>/panel/<n>` with a JSON body of **only** declared params →
    the server binds them and returns the rendered SVG (or rows).
  - `GET  /d` → list of dashboards the caller may see.
- **Parameters are typed + whitelisted** (enum of allowed values, or a numeric
  range/date window) and **bound as query parameters** — never string-
  concatenated — so there is no injection surface. A param that isn't declared
  is rejected.

Result: a consumer can pick `region=EU` from a dropdown, but cannot change the
query, add a column, `ATTACH`, read a file, or reach another table.

### 2. Read-only, least-privilege connection

Defence in depth, so even a bug in param handling is bounded.

- Serve from a DuckDB connection that can only **read the specific views** the
  dashboards expose (not base tables): `CREATE VIEW dash_sales AS SELECT … ;`
  and grant/scope to those.
- Lock the session down: no `ATTACH`/`INSTALL`/`COPY`/file reads
  (`SET enable_external_access = false;`, disable the relevant functions), and
  run the process as an OS user with no filesystem access beyond what it needs.
- For MotherDuck/Postgres, use a **read-only role** on the upstream too.

### 3. Auth + TLS (deployment)

- Bind address is configurable (default `127.0.0.1`; opt in to `0.0.0.0` only
  behind a proxy).
- Terminate **TLS + authentication at a reverse proxy** (nginx / Caddy /
  Cloudflare) in front of the serving process.
- Optional: signed dashboard links or per-consumer tokens for finer access.

---

## Two modes, one binary

| | **Admin / authoring** (today) | **Serve** (new) |
|---|---|---|
| UI | full builder + editor | view-only, fixed dashboards |
| SQL source | free-form `/query` from the client | server-owned templates + params |
| DB connection | read-write session | read-only, view-scoped |
| Bind / auth | localhost, dev token | proxy + TLS + auth |
| Audience | the author | untrusted consumers |

Admin mode stays for development; serve mode is what you expose.

---

## Freshness & concurrency

- Each panel request runs its stored query on the live read-only connection → the
  data is always current.
- Add **per-panel result caching with a TTL** so N consumers refreshing don't
  become N× load on the upstream DB; the TTL doubles as the freshness knob.
- Bake a default **auto-refresh interval** into the served dashboard.

---

## Where it should live

Today the server is embedded in the DuckDB extension. For v1, a **small
standalone service** (the core renderer + a DuckDB connection it owns + the HTTP
layer above) is cleaner than a server inside an extension, and it can `ATTACH`
MotherDuck/PostgreSQL itself. The extension keeps `anofox_serve` for local
authoring.

---

## Steps

1. Split `anofox_serve` into **admin** vs **serve** modes; move free-form
   `/query` behind admin + localhost + a token.
2. **Dashboard registry** — parse annotated SQL → stored plan + declared params
   (from `.sql` files and/or a `register` call).
3. **Typed param binding** — whitelist + bind as query params; reject unknowns.
4. `GET /d/<id>` view-only shell + `POST /d/<id>/panel/<n>` param endpoint.
5. **Read-only, view-scoped connection** + capability lockdown.
6. **Deployment guide** — reverse proxy (TLS/auth), bind address; optional
   result caching + default refresh interval.

Step 1 removes the immediate footgun (free-form SQL on a network-exposed port);
steps 2–4 are the server-owns-the-SQL flip; 5–6 harden and productionize.
