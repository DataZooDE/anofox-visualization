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
| Bind | loopback only (enforced) | `bind` option (default `127.0.0.1`) | `--bind` (default `127.0.0.1`) |
| Access control | loopback `Host`, same-origin, per-server token | same-origin `POST`; login via an auth proxy (`options.auth`); row and column policies (`options.policy`) | login via an auth proxy (`--auth`); no policies |

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
  text — `/healthz` is public). With `auth`, a caller that is not signed in
  through the proxy gets only `{"status":"ok"}`.
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
  values listed), and setup statements must be queries, `SET VARIABLE <name> =
  <expression>` or `CREATE [OR REPLACE] TEMP TABLE|VIEW <name> AS <query>`
  with a query that passes the same check (the same classifier the
  [access policy](#row-and-column-policies-access-control-p2) uses). Everything else — `EXPLAIN`,
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
  `threads` (HTTP worker pool = concurrent requests, default 4), `cache_mb`
  (see below), `attach`, `load`, `hosts` (see below), `refresh_s`, `data`,
  `path_vars` (see above), `auth`, `log`, `log_vars` (see
  [Login via an auth proxy](#login-via-an-auth-proxy-access-control-p1)),
  `policy` (see [Row and column policies](#row-and-column-policies-access-control-p2)),
  and for [deployment](deployment.md):
  - `bind` — listen address, default `127.0.0.1`. `"0.0.0.0"` (or `"::"`) is
    for containers and replicas behind a load balancer. A specific address is
    also accepted as a `Host`; other names still need `hosts`.
  - `memory_limit` — DuckDB `memory_limit` of the snapshot instance (`"2GB"`,
    `"50%"`). The default is DuckDB's 80 % of RAM, shared by all queries.
  - `duckdb_threads` — DuckDB `threads` of the snapshot instance, shared by all
    queries. The default is the number of cores.
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
- `--auth '<json>'` (or `--auth @auth.json`) — login via an auth proxy, the
  same object as `options.auth` below; `--log <path|stderr>` and
  `--log-vars none|names|values` — the request log. With `--auth` the render
  cache (`--cache`) is kept per viewer; it holds at most 1,024 pages / 64 MiB
  (oldest first out) and drops expired pages. A non-loopback `--bind` without
  `--auth` prints a `WARNING`. A request target with a control character is
  `400`. The render server has no access policies (only the locked server
  does), so with `--auth` every panel has to filter on the claims itself.

Try it: `serve --dashboards examples/serve-dashboards/dash --init
examples/serve-dashboards/init.sql` → `http://127.0.0.1:8080/`.

![serve mode — sales dashboard with a whitelisted region param](img/serve-sales.png)

---

- **DNS rebinding.** The `Host` must be a loopback name on the server's port,
  the `--bind` address, or a name given with `--allow-host NAME` (repeatable;
  `*` disables the check); anything else gets `421`. Behind a proxy that
  forwards the original `Host`, pass its public name with `--allow-host`.

## Login via an auth proxy (access control P1)

DuckDB has no users, roles, `GRANT` or row-level security, so who sees what is
enforced by the server. Phase 1 takes the viewer's identity from an **auth
proxy** that already does the login (oauth2-proxy, Caddy `forward_auth`, nginx
`auth_request`, Cloudflare Access, …) and hands it to the dashboard SQL.

```sql
SELECT anofox_serve_dashboards('dashboards', 8095, '{
  "hosts": ["dash.example.com"],
  "auth": {
    "mode": "proxy",
    "shared_secret_env": "ANOFOX_PROXY_SECRET",
    "trusted_proxies": ["127.0.0.1"],
    "header_user": "X-Forwarded-User",
    "header_groups": "X-Forwarded-Groups",
    "claims": {"tenant": {"header": "X-Forwarded-Tenant", "required": true}}
  },
  "policy": "/etc/anofox/policy.json",
  "log": "/var/log/anofox/requests.jsonl"
}');
```

With `auth`, the server needs an access policy (`policy`, or `policy.json`
in the dashboard folder — see
[Row and column policies](#row-and-column-policies-access-control-p2)), or an
explicit `"policy": "none"` to rely on claim filters in the panels as below;
otherwise it refuses to start.

| `auth` key | Default | Meaning |
|---|---|---|
| `mode` | (required) | `"proxy"` — the only mode in P1 |
| `shared_secret` / `shared_secret_env` | (one is required) | The secret the proxy sends on every request; ≥ 32 printable characters (`openssl rand -hex 32`). Prefer `shared_secret_env` (the name of an environment variable of the DuckDB process): a literal in the SQL ends up in shell history and query logs |
| `secret_header` | `X-Anofox-Proxy-Secret` | Header carrying the secret |
| `trusted_proxies` | key absent: any peer | Addresses / CIDR ranges (`10.0.0.0/8`, `::1`) the proxy connects from; any other peer is refused even with the right secret. An empty list is an error. Write IPv4 ranges as IPv4 (`::ffff:…` forms are rejected; IPv4-mapped peers are matched as IPv4) |
| `header_user` | `X-Forwarded-User` | Header with the user name / e-mail (required, non-empty) |
| `header_groups` | off (`null`) | **Opt-in.** Groups header, split on `groups_separator` (default `,`) |
| `claims` | — | Extra claims → `__claim_<name>` (up to 16): `{name: "Header"}` or `{name: {"header": "Header", "required": true}}` |

`header_groups` takes the same `{"header": …, "required": true}` form. A
`required` claim that is absent or empty is `401` (`deny: missing_claim`);
otherwise an absent claim is `NULL` (groups: `[]`). The start message lists the
identity headers the server trusts — compare it with the proxy configuration.

**How a request is checked.** With `auth` set, every request except `GET
/health` (`/healthz`; it answers `{"status":"ok"}` without a login and its full
body only with one) must (1) come from a `trusted_proxies` address (when given), (2) carry
the shared secret exactly once (constant-time comparison), and (3) carry a
non-empty user header exactly once. Otherwise the answer is `401` with the same
body for every reason (the reason goes to the request log only). Identity
headers are read **only after** the secret checks out, so identity headers a
client sends directly are never believed. A repeated identity header (a proxy
that appends instead of replacing) is refused rather than guessed at. A broken
`auth` object fails the server start: the server never falls back to serving
without login.

**The secret proves that a request came through the proxy — not that the proxy
wrote every header in it.** A proxy forwards the client's other headers, so
every configured identity header (user, groups, each claim) must be **set or
removed by the proxy on every request**; one it leaves alone reaches the server
with the client's value. That is why groups and extra claims are opt-in:
configure only headers your proxy controls (the recipes below do this for each
header they name).

**Claims are reserved variables.** After the viewer's own variables, the
server sets, on the request's private connection:

| Variable | Type | Value |
|---|---|---|
| `__claim_user` | `VARCHAR` | the user header |
| `__claim_groups` | `VARCHAR[]` | the groups, trimmed, sorted, deduplicated (`[]` if none) |
| `__claim_<name>` | `VARCHAR` | each `claims` header (`NULL` if absent) |

All three are typed literals like any variable value. A viewer can **never**
set a `__claim_*` variable (any case): a panel request naming one is `400`, a
`-- @param` or `-- @vars` entry naming one is ignored, and the browser client
ignores `?v.__claim_*` in the URL. The same holds for `anofox_data` (the data
folder, see above), whether or not a data folder is configured. Without `auth`
the claims are simply unset
(`NULL`), so a panel that filters on them returns no rows instead of every row.

```sql
-- rows of the viewer's regions; the 'admins' group sees all
SELECT region::XAXIS, sum(revenue)::BARCHART FROM orders
WHERE region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))
   OR list_contains(getvariable('__claim_groups'), 'admins')
GROUP BY region;
```

**Only filter on claims, never on viewer variables.** Any variable that is not
a claim is set by the viewer and can hold any value. Optionally restrict the
viewer-settable variables of a dashboard with a header line
`-- @vars region, channel, selected`; variables not listed are dropped (never
set) on a locked server. List the dashboard's inputs, named selections and
`selected` (the client sends the shared click selection).

**Per-viewer caching.** The result cache and the single-flight key include the
viewer's claims (user, groups, every extra claim), so a result computed for one
viewer is never served to — and a running query never shared with — a viewer
with different claims. The static server's `--cache` key includes the claims
too. Supersession tickets are scoped to the user as well.

### Proxy recipes

The proxy must **authenticate every request, set or remove every configured
identity header (never append, never pass the client's through), add the
secret, and be the only way to reach the server**. Keep the server on loopback
(or on a private network with `trusted_proxies`). The nginx, Caddy and
oauth2-proxy configs below were checked with `nginx -t`, `caddy validate` and
oauth2-proxy's strict config loader. The nginx and Caddy recipes were also run
in front of a server, with a stand-in for oauth2-proxy's `/oauth2/auth`, while
a client sent forged `X-Forwarded-User`, `X-Forwarded-Groups` and
`X-Forwarded-Tenant`: the server saw only the proxy's values (and no groups or
tenant when the login had none).

nginx + oauth2-proxy (`auth_request`). `proxy_set_header` replaces the client's
header, and with an empty value nginx sends no header at all. So every identity
header gets a `proxy_set_header` line, even one the IdP may not return:

```nginx
location = /oauth2/auth {
  internal;
  proxy_pass http://127.0.0.1:4180;
  proxy_pass_request_body off;
  proxy_set_header Content-Length "";
}
location / {
  auth_request /oauth2/auth;
  auth_request_set $user   $upstream_http_x_auth_request_email;
  auth_request_set $groups $upstream_http_x_auth_request_groups;
  auth_request_set $tenant $upstream_http_x_auth_request_tenant;
  proxy_set_header X-Forwarded-User   $user;    # set or removed, never the client's
  proxy_set_header X-Forwarded-Groups $groups;  # (only if header_groups is set)
  proxy_set_header X-Forwarded-Tenant $tenant;  # one line per claims header
  proxy_set_header X-Anofox-Proxy-Secret "<secret>";
  proxy_set_header Host $http_host;             # with its port; list it in options.hosts
  proxy_pass http://127.0.0.1:8095;
}
```

(Plus oauth2-proxy's own `location /oauth2/ { proxy_pass http://127.0.0.1:4180; }`
and `error_page 401 = /oauth2/sign_in;` for the login redirect.)

Caddy (`forward_auth`). Delete every identity header first, inside a `route`
block so the directives run in the written order (outside `route`, Caddy sorts
them). `copy_headers` then sets the ones the auth response carries:

```caddy
dash.example.com {
  route {
    request_header -X-Forwarded-User
    request_header -X-Forwarded-Groups
    request_header -X-Forwarded-Tenant
    forward_auth 127.0.0.1:4180 {
      uri /oauth2/auth
      copy_headers X-Auth-Request-Email>X-Forwarded-User X-Auth-Request-Groups>X-Forwarded-Groups X-Auth-Request-Tenant>X-Forwarded-Tenant
    }
    request_header X-Anofox-Proxy-Secret {env.ANOFOX_PROXY_SECRET}
    reverse_proxy 127.0.0.1:8095
  }
}
```

oauth2-proxy on its own (alpha config). oauth2-proxy removes each
`injectRequestHeaders` header from the client's request before setting it
(`preserveRequestValue: false` is the default), and joins multi-valued claims
such as groups with `,`:

```yaml
upstreamConfig:
  upstreams:
  - id: anofox
    path: /
    uri: http://127.0.0.1:8095
injectRequestHeaders:
- name: X-Forwarded-User
  values:
  - claimSource:
      claim: email
- name: X-Forwarded-Groups        # only with "header_groups": "X-Forwarded-Groups"
  values:
  - claimSource:
      claim: groups
- name: X-Anofox-Proxy-Secret
  values:
  - secretSource:
      fromFile: /etc/anofox/proxy-secret
```

Cloudflare Access (not tested here): Access sets
`Cf-Access-Authenticated-User-Email` on requests it has authenticated. Use it as
the only identity header: `"header_user": "Cf-Access-Authenticated-User-Email"`,
with **no `header_groups` and no `claims`**, because Cloudflare passes other
client headers such as `X-Forwarded-Groups` through to the origin. Add the
secret with a Transform Rule (*Modify Request Header → Set static*, which
replaces a client's value), and let only `cloudflared` reach the server. Groups
or other claims from Access require verifying the signed
`Cf-Access-Jwt-Assertion`, which is P3 (#50).

**Folded headers (obs-fold).** An HTTP/1.1 header line that starts with a space
or tab continues the previous header (obsolete line folding, RFC 9112 §5.2).
The server's HTTP parser does **not** unfold such a line; it reads it as a
header of its own. A proxy that forwarded folded lines unchanged would
therefore let a client add identity headers the proxy never saw. The proxy must
reject folded headers or unfold them before forwarding, as RFC 9112 requires.
Both were checked: nginx answers `400`, and Caddy (Go) unfolds the line into
the previous header's value.

### Request log

`"log": "/var/log/anofox/requests.jsonl"` (or `"stderr"`; `serve`: `--log`)
appends one JSON object per line for page views, `/api/panel` requests and
every refused request (static assets are not logged):

```json
{"ts":"2026-10-16T20:47:19.487Z","user":"alice@example.com","method":"POST","path":"/api/panel","status":200,"ms":25,"dashboard":"rls","panel":1,"cache":"run","bytes":68,"peer":"127.0.0.1","vars":["channel"]}
{"ts":"2026-10-16T20:47:19.762Z","user":null,"method":"POST","path":"/api/panel","status":401,"ms":0,"peer":"127.0.0.1","deny":"bad_secret","vars":[]}
```

- `ts` UTC, `ms` latency, `cache` = `hit` / `shared` (joined a running
  identical query) / `run`, `bytes` = response size, `deny` = `untrusted_peer` /
  `bad_secret` / `no_user` / `missing_claim` / `malformed_identity`.
- `log_vars` (`--log-vars`): `"names"` (default) logs variable names only;
  `"values"` logs their SQL literals (may be personal data — opt in);
  `"none"` omits them. The path is logged without its query string. Headers,
  cookies, the secret and claims other than the user are never logged.
- The file is created `0600` (an existing file readable by others is set to
  `0600`), never through a symlink (`O_NOFOLLOW`), and reopened for every line,
  so a rename-based rotation needs no signal (logrotate: `daily`, `rotate 14`,
  `compress`, `delaycompress`, no `copytruncate` needed). Each line is one
  `write` with `O_APPEND`.
- With `"stderr"`, the server's plain diagnostics share the stream; they start
  with `[anofox-serve]` and escape every control character in request data
  (a newline is written as `\n`), so only the request log writes lines that
  start with `{`. A request target containing a control character is `400`.

### Threat model (P1)

| Threat | Mitigation |
|---|---|
| A client sends `X-Forwarded-User: admin` directly to the server | No secret → `401`; identity headers are only read after the secret matches. Keep the server reachable only by the proxy (loopback / private network + `trusted_proxies`) |
| A client sends identity headers *through* the proxy | The proxy must overwrite them (recipes above); a duplicated header is `401` |
| A client sends identity headers the proxy **doesn't set** (groups, extra claims) — the secret still matches | Groups and claims are opt-in (only `header_user` by default); the proxy must set or remove every configured header on every request (each recipe does; checked with forged headers through nginx and Caddy); `required` claims make a missing value `401`; the start message lists the trusted headers |
| Folded (obs-fold) header lines smuggling an identity header past the proxy | The proxy must reject or unfold them (nginx: `400`, Caddy: unfolds); the server can't tell a folded line from a header |
| The secret leaks (config file, `ps`, SQL history) | `shared_secret_env`, `trusted_proxies` as a second factor; the secret never appears in errors, `Debug` output or the log. Rotate by restarting with a new value |
| Brute-forcing the secret | ≥ 32 characters, constant-time comparison (no early return on a length mismatch either) |
| A viewer sets `__claim_user` / `__claim_groups` / `anofox_data` (`vars`, `?v.*`, a `@param`) | Rejected (`400`) / ignored, in any case and in every configuration; claims and `anofox_data` are set by the server after the viewer's variables (a unit test pins this order) |
| Snapshot refresh (`refresh_s`) mixing users or snapshots | Each snapshot generation has its own cache; keys include the claims, supersession tickets the user (tested with two users under constant refreshes) |
| `/healthz` leaks snapshot state | With `auth`, only `status` without a login |
| A panel filters on a viewer-settable variable | Documented rule: filter only on `__claim_*`; `-- @vars` limits what viewers can set |
| One viewer's cached result served to another | Claims are part of the cache and single-flight key (both servers) |
| One viewer cancels another's query | Supersession tickets are scoped to user + 128-bit page id |
| A misconfigured `auth` serves without login | A bad `auth` fails the start; there is no fallback to anonymous |
| Server reachable from the network without login | `serve --bind` beyond loopback without `--auth` prints a `WARNING`; so does the extension's start message for `bind` beyond loopback without `auth` (the default is loopback) |
| Log injection / personal data in logs | JSON-escaped lines; diagnostics on the same stderr escape control characters, and a URL with one is `400`; variable values only with `log_vars: "values"`; the log file is `0600` and never opened through a symlink |
| Many viewers fill the static server's `--cache` | Capped at 1,024 pages / 64 MiB, expired pages purged |
| DNS rebinding, CSRF, SQL from the client | Unchanged: Host allowlist (`421`), same-origin `POST`s, no SQL endpoint |

**Out of scope for P1** (see #50): verifying JWTs/OIDC tokens itself; signed
embed links; API keys. On its own, P1 relies on the dashboard author writing
the claim filter into every panel; [P2 policies](#row-and-column-policies-access-control-p2)
enforce it regardless of the panel SQL.

**What can never enforce access:** a static HTML export or the browser /
DuckDB-Wasm demo (the data is in the client, whoever opens it can read all of
it), and a MotherDuck token (it grants the whole database). Only the locked
server (A) and the static render server (B) keep data on the server.

## Row and column policies (access control P2)

With P1 every panel has to filter on the claims itself, and one forgotten
`WHERE` shows every row. A **policy** moves the rule out of the panels: the
locked server (A) rewrites every dashboard statement so that it can only read
tables through per-viewer views, and refuses to start when a statement could
get around them.

```json
{
  "tables": {
    "sales": {
      "rows": "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user')) OR list_contains(getvariable('__claim_groups'), 'admins')"
    },
    "employees": {
      "rows": "getvariable('__claim_tenant') = tenant",
      "columns": {
        "salary": {"allow_groups": ["hr"], "else": "null"},
        "email":  {"allow_groups": ["hr", "support"], "else": "mask"},
        "name":   {"allow_groups": ["hr"], "else": "hash"},
        "ssn":    {"allow_users": ["payroll@example.com"], "else": "deny"}
      }
    },
    "ref.regions": {"rows": true}
  }
}
```

Pass the path of the policy, or put it in `policy.json` next to the dashboards:
`anofox_serve_dashboards('dashboards', 8095, '{"auth": {…}, "policy": "/etc/anofox/policy.json"}')`.
A `policy.json` in the dashboard folder is enforced even without the option, so
forgetting the option can't serve every row. **Keep the policy outside the
dashboard folder** when the people who write dashboards should not decide who
sees what: with `options.policy` a missing or broken file stops the start.
With `auth` and no policy found (e.g. a `policy.json` deleted from the
folder) the server refuses to start unless `"policy": "none"` says that the
panels filter on the claims themselves. The start message says `access policy
on N table(s)`; with a policy but no `auth` it warns that every viewer is
anonymous (claims unset, so claim-based row filters return nothing).

Under a policy, a viewer whose panel fails gets a generic message ("this
panel could not be shown"); the DuckDB error, which might quote a value, goes
to the server log only.

### Policy file reference

| Key | Meaning |
|---|---|
| `tables` | `{"<table>" or "<schema>.<table>": rule}` — tables and views of the snapshot's default database (the one the session `USE`s). **Every table a dashboard reads needs a rule**; any other table is refused. |
| `tables.*.rows` | **Required.** A SQL condition over the table's columns that a row must meet, or `true` for every row. It may read the viewer only through `getvariable('__claim_…')` with a literal name (a viewer variable is refused). It may use other tables (e.g. an entitlement table): those are read directly, without their own policy, and the panels still can't read them unless they have a rule too. It must be one condition: text that ends the `WHERE` (`… ) UNION SELECT …`) is refused. |
| `tables.*.columns` | `{"<column>": "deny" \| "null" \| "mask" \| "hash"}` for everyone, or `{"allow_groups": [...], "allow_users": [...], "else": "deny" \| "null" \| "mask" \| "hash"}` (default `else`: `null`). Viewers in an allowed group (`__claim_groups`) or with an allowed user name (`__claim_user`) see the value. Columns not listed are visible. |
| `table_functions` | File readers the dashboards may call (`read_parquet`, `read_csv`, `read_json`, … `glob`). Off by default. Files have no row or column policy (DuckDB still confines them to `options.data`). Nothing else can be allowed. |
| `functions` | Extra scalar / aggregate functions the dashboards may call — an extension's (`st_area`, …), after you reviewed that they only compute on their arguments. A function the reviewed list denies can't be added. |

The `hash` mask's key is random per server start; set
`"policy_key_env": "ANOFOX_POLICY_KEY"` (the name of an environment variable
holding ≥ 32 characters, e.g. `openssl rand -hex 32`) in the server options
to keep hashes stable across restarts.

Unknown keys are errors. The policy is checked against the snapshot at start
and at every refresh: a table or column it names that doesn't exist, a row
filter that doesn't parse or bind, or a relation named `__anofox…` stops the
start (a failing refresh keeps the previous snapshot in service,
`refresh_failed: true` in `/healthz`).

| Column rule | The viewer gets |
|---|---|
| `deny` | no column: it is not in the view (`SELECT *` leaves it out; a panel naming it fails for this viewer — lint below) |
| `null` | `NULL` of the column's type: the panel keeps working |
| `mask` | `'****'` (`NULL` stays `NULL`) |
| `hash` | 16 hex digits of HMAC-SHA256(key, column ‖ value): equal values in one column stay equal, so grouping and `count(DISTINCT …)` work, but the value can't be looked up, and the same value hashes differently in another column. The server computes it in its own function (`__anofox_policy_hash`), so the key is never in SQL, plans, errors or logs. Random per server start unless `policy_key_env` is set |

### How it is enforced

1. **At start (and per refresh), once per statement.** DuckDB parses the
   statement (`json_serialize_sql`). Every table reference becomes
   `temp.main.__anofox_rls_<i>` (with the original name as alias, so `t.col`
   still works); CTE names are renamed to `__anofox_cte_<n>`, so a name the
   rewrite takes for a CTE can only ever bind to that CTE. DuckDB prints the
   result (`json_deserialize_sql`), which is parsed **again and verified**: it
   may reference only those views, the dashboard's own temp objects and the
   renamed CTEs. The browser still shows the dashboard's original SQL; the
   server runs only the rewritten one.
2. **Per request**, on the request's private connection, after the claims are
   set: `CREATE TEMP VIEW __anofox_rls_<i> AS SELECT * EXCLUDE (denied)
   REPLACE (masked) FROM (SELECT * FROM <db>.<schema>.<table> WHERE <rows>)`.
   The column decision uses the viewer's claims; the row filter reads them
   from the session variables. A claim value is never spliced into SQL text.
3. `::PAGED` requests (rows, count, stats, profile, distinct, filters) wrap
   the rewritten panel, so they see only the viewer's rows and masked values.
   The result cache and single-flight key include the claims (P1).

**Fail closed.** A dashboard statement is refused, and the server does not
start (the error names the dashboard and statement), when it:

- reads a table or view without a rule, including `information_schema.*`,
  `pg_catalog.*`, a file path (`FROM 'x.csv'`) and a view whose body reads
  other tables;
- names a database (`snap.main.sales`, `temp.main.__anofox_rls_0`);
- calls a table function other than `range`, `generate_series`, `unnest` and
  the policy's `table_functions` — so `query()`, `query_table()`,
  `json_execute_serialized_sql()`, the `duckdb_*()` / `pragma_*()` catalog
  functions, `sql_auto_complete()`, table macros and file readers;
- calls a user macro (its body is not checked);
- calls a function that is not on the **allowlist**
  (`src/host/policy_functions.txt`: every function of the pinned DuckDB
  1.5.6, classified `allow` or `deny`) or in the policy's `functions`. Denied:
  the functions that read the catalog, statistics, settings or a SQL text of
  their own, outside the views — `json_serialize_plan` (it binds and
  optimizes any SQL string against the raw tables; its plans would leak min/max
  statistics and the view definitions), `stats`, `json_serialize_sql`,
  `current_setting`, `current_query`, `currval`, `nextval`, `getenv`, the
  `pg_*` / `has_*_privilege` catalog macros, … A function the list doesn't know
  (a newer DuckDB, a loaded extension) is refused; a test fails when the
  pinned DuckDB has an unclassified function;
- uses time travel (`AT (VERSION => …)`), `SHOW TABLES` or `DESCRIBE` /
  `SUMMARIZE` of a table without a rule;
- is not a query, `CREATE [OR REPLACE] TEMP TABLE|VIEW <name> AS <query>` or
  `SET VARIABLE <name> = <expr>` (whose query / expression is rewritten too):
  `CREATE MACRO`, `ATTACH`, `COPY`, `SET`, `PIVOT …` statements (use `FROM t
  PIVOT (…)`), … are refused, as is a setup statement that sets a `__claim_*`
  variable or creates an object named `__anofox…`.

CTEs, subqueries (in `FROM`, `SELECT`, `WHERE`, `IN`, `EXISTS`), joins
(lateral, positional, ASOF), set operations, recursive CTEs, `PIVOT` in `FROM`,
`unnest` of a subquery, `DESCRIBE SELECT …` and the dashboard's own temp tables
all work, and all read the viewer's views. A viewer request that sets a
`__claim_*` or `__anofox…` variable is `400`.

**Lint before serving.** `dashboard --check dash.sql --db snapshot.duckdb
--policy policy.json` reports what the server would refuse
(`policy/refused`), panels that use a column some viewers can't see
(`policy/denied-column`: a warning, an error when nobody may see it — the
panel then always fails) and masked columns (`policy/masked-column`, a note).
It uses the server's classifier and function list; it runs the `duckdb` CLI
on PATH to parse, and warns (`policy/duckdb-version`) when that is not the
pinned 1.5.6. The server checks again at start and fails closed either way.

`duckext/scripts/policy-key-e2e.sh <extension>` runs a locked server with a
known `hash` key and checks that the key is on no reachable surface (panel
results and errors, `::PAGED` profiles, the page, `/healthz`, stderr, the
request log), that the served hash is the specified HMAC, and that SQL which
could show it (`duckdb_views()`, `current_query()`, `json_serialize_plan`,
`current_setting`) and `auth` without a policy refuse to start.

### Threat model (P2)

| Threat | Mitigation |
|---|---|
| A panel forgets the claim filter | The panel reads the policy's view, not the table |
| A dashboard reads a table without a rule (directly, in a subquery, a CTE, a `UNION` arm, a lateral join, a `SET VARIABLE`, a temp table) | Every table reference in every statement is rewritten or refused; refused = no start |
| A CTE named like a table (`WITH payroll AS (…)`), or a table named like a CTE | CTEs are renamed; a reference the rewrite takes for a CTE can only bind to that CTE, everything else goes through a view or is refused. A non-recursive CTE's own name inside its body is the table, as in DuckDB |
| `snapshot.main.t`, `temp.main.__anofox_rls_0`, `information_schema`, `duckdb_tables()`, `pragma_table_info()` | Database-qualified names and catalog functions are refused |
| String-built SQL: `query('SELECT … ' \|\| getvariable('x'))`, `query_table(…)`, `json_execute_serialized_sql(…)` | Refused (table functions outside the allowlist) |
| A macro (or a built-in macro) whose body reads a table | Macros are refused; `CREATE MACRO` in setup is refused |
| A view in the snapshot whose body reads a protected table | A view needs its own rule; the view's rule is what applies to it |
| Setup overwrites the claims (`SET VARIABLE __claim_groups = ['admins']`) or the views | Refused at start; views are created after the claims and before any dashboard SQL, and their names are reserved |
| A viewer sets `__claim_*` / `__anofox…` (`vars`, `?v.*`, `@param`) | `400` / ignored (P1) |
| A row filter reads a viewer-settable variable | Refused at start: a row filter may read only `getvariable('__claim_…')` |
| Row filter text that ends the `WHERE` clause | Refused: the filter must stay one condition |
| A masked or denied column via `::PAGED` filter / sort / stats / distinct / profile | Paging wraps the rewritten panel; it sees `NULL` / `****` / a hash, a denied column doesn't exist |
| Guessing a hashed value (dictionary attack on `md5(email)`) | HMAC-SHA256 with a server key and the column's name, computed in Rust: the key never appears in SQL, plans, errors or logs |
| Probing tables, statistics or the views' SQL through `json_serialize_plan('…')`, `stats(col)`, `current_setting`, `current_query` (#105) | Functions are allowlisted: those are `deny`, and anything not classified is refused |
| An error message quotes a value the viewer may not see | Under a policy viewers get a generic error; the details go to the log |
| A dashboard author deletes `policy.json` from the dashboard folder | With `auth`, no policy refuses the start unless `"policy": "none"`; keep the policy outside the folder (`options.policy`: a missing file stops the start) |
| One viewer's cached result served to another | Claims are part of the cache and single-flight key (P1) |
| A refresh brings a renamed table or column | The policy is re-checked; the refresh fails and the previous snapshot stays |
| A rewrite bug lets a table through | The rewritten SQL is parsed again and verified against an allowlist of names; a mismatch stops the start |

**Limits.** Policies cover the locked server (A) only: the static render
server `serve --dashboards` (B) has no policies (filter in the panels, P1
style), and a static HTML export or the browser / DuckDB-Wasm demo can never
enforce rows or columns (the data is in the client). Files in `options.data`
read through allowed `table_functions` have no row or column policy. A rule
on a view applies to the view's output; the tables it reads are protected only
where panels read them directly. Column rules decide by user and group;
other claims (`__claim_tenant`, …) can drive row filters only. `hash` is
pseudonymisation, not encryption: a viewer who knows a value and sees it next
to its hash elsewhere can match them.

## Deployment

[`deployment.md`](deployment.md) covers deploying a locked server: one process
under systemd or in a container, TLS and auth at a reverse proxy (Caddy, nginx)
with the `Host` allowlist, resource limits, replicas behind a load balancer,
and sizing from the load test (`scripts/load-test.mjs`).

## What remains the operator's job

- **Authentication + TLS** for consumer-facing modes: terminate both at a reverse
  proxy (nginx / Caddy / Cloudflare) in front of the server; bind the server to
  loopback and let only the proxy reach it. Pass the identity on with `auth`
  (above) when dashboards show different rows to different viewers.
- **Least privilege on the data**: the snapshot contains everything in the
  session's DuckDB databases, and every file in `options.data` is readable. Serve from a session (or `--db`) that only holds
  the tables/views the dashboards need.
- **Variable values are not whitelisted in mode A**: a viewer can set any
  identifier-named variable (except `__claim_*`; `-- @vars` limits the names)
  to any literal. Panels see them only through `getvariable()`, so write panel
  SQL that treats variables as untrusted *values* (they cannot change the
  query's structure), and never use one as an access filter.
- **Process isolation**: run the server as an OS user without access to more
  than it needs.
