# Deploying a locked dashboard server

How to run `anofox_serve_dashboards` (mode A of
[`secure-serving.md`](secure-serving.md)) for real viewers: as one process,
under systemd or in a container, behind a TLS proxy, and as several replicas
behind a load balancer. The sizing numbers at the end come from
`scripts/load-test.mjs`.

The files referenced here are in [`deploy/`](../deploy):

| File | What it is |
|---|---|
| `run-locked.sh` | entry point: runs a SERVE_SQL with the DuckDB CLI and keeps it in the foreground |
| `serve.example.sql` | a SERVE_SQL: load the extension, attach the data, start the server |
| `anofox-dashboards.service` | systemd unit with resource limits and sandboxing |
| `Dockerfile` | container image (DuckDB CLI + the native extension) |
| `Caddyfile`, `nginx.conf` | reverse proxy: TLS, basic auth, `Host`, sticky replicas |
| `compose.yaml`, `compose-serve.sql` | two replicas behind Caddy, to try locally |

## 1. What runs

A locked server is a thread pool inside a DuckDB process. SERVE_SQL is a short
SQL script run by the DuckDB CLI:

```sql
LOAD '/opt/anofox/anofox_visualization.duckdb_extension';
ATTACH '/data/warehouse.duckdb' AS warehouse (READ_ONLY);
CREATE VIEW sales AS FROM warehouse.sales;     -- or write warehouse.sales in the dashboards
SELECT anofox_serve_dashboards('/etc/anofox/dashboards', 8080, '{
  "bind": "127.0.0.1", "hosts": ["dash.example.com"],
  "threads": 16, "duckdb_threads": 4, "memory_limit": "2GB", "cache_mb": 256,
  "timeout_ms": 10000
}');
```

Attach the data `READ_ONLY`. Two things don't work here:

- `USE warehouse`: the server takes its snapshot through the extension's own
  connection, which keeps the CLI's main database as its current database. So
  either qualify the tables in the dashboards (`warehouse.sales`) or expose
  them as views in the main database, as above.
- Opening the file with `duckdb -readonly`: the snapshot has to be written
  through that session, which a read-only session refuses.

At startup the server copies the session's DuckDB databases into a private
snapshot and checks every dashboard statement. Only then does it bind the
port. So once `GET /healthz` answers `{"status":"ok",...}`, the server is ready.
For the 2-million-row load-test data that took 2–5 s. The snapshot changes
only when it is refreshed (§5).

The process must stay in the foreground. The DuckDB CLI exits when its stdin
ends, and the servers stop with it. [`deploy/run-locked.sh`](../deploy/run-locked.sh)
gives the CLI a stdin that never ends: a FIFO it holds open for both reading and
writing. It then `exec`s the CLI, so signals reach DuckDB directly: `SIGTERM`
stops it. An error in SERVE_SQL ends the CLI with status 1, so a process that
serves nothing doesn't stay up.

With `ANOFOX_CONTROL=/run/anofox/control`, that FIFO lives at a known path
(mode `0600`). SQL written to it runs in the serving session, e.g. a refresh
(§5). An error there is printed and the server keeps running. Whoever can
write to the FIFO can run any SQL in the read-write session, so keep it
private to the server's user.

```sh
DUCKDB=/opt/anofox/duckdb deploy/run-locked.sh /etc/anofox/serve.sql
curl -s http://127.0.0.1:8080/healthz
# {"status":"ok","dashboards":3,"epoch":1,"snapshot_age_s":4,"refresh_s":null,"refreshing":false,"refresh_failed":false}
```

The DuckDB CLI must be the version the extension was built for (1.5.6 here).
The extension is unsigned, so `run-locked.sh` passes `-unsigned`.

### Options that matter for deployment

| Option | Default | |
|---|---|---|
| `bind` | `127.0.0.1` | Listen address. Use `0.0.0.0` in a container or when a load balancer on another host connects. A specific address is also accepted as a `Host`. |
| `hosts` | loopback names | The public name(s) the proxy forwards in `Host` (§3). Without it, proxied requests get `421`. |
| `threads` | 4 | HTTP worker threads = requests handled at once. Cache hits are cheap, so 16–32 is fine. A request that waits for an identical one also occupies a worker. |
| `duckdb_threads` | number of cores | DuckDB `threads` of the snapshot instance, shared by all queries. |
| `memory_limit` | 80 % of RAM | DuckDB `memory_limit` of the snapshot instance, shared by all queries. Set it on a shared host. Above it, queries spill to the snapshot's temp directory or fail. |
| `cache_mb` | 64 | Result cache. 0 turns it off; single-flight stays on. |
| `timeout_ms` | 30 000 | Per request; the query is interrupted. Keep the proxy's timeout above it. |
| `max_rows`, `max_body_bytes` | 100 000, 64 KiB | Per result / request body. |

The DuckDB settings apply before the configuration is locked. A request cannot
change them.

## 2. One process

### systemd

```sh
install -D -m 755 deploy/run-locked.sh /opt/anofox/run-locked.sh
install -m 755 duckdb anofox_visualization.duckdb_extension /opt/anofox/
install -D -m 644 serve.sql /etc/anofox/serve.sql      # + /etc/anofox/dashboards/*.sql
cp deploy/anofox-dashboards.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now anofox-dashboards
journalctl -u anofox-dashboards -f                     # one line per request
```

The unit ([`anofox-dashboards.service`](../deploy/anofox-dashboards.service)):

- runs as a throw-away user (`DynamicUser=yes`) that can read only `/data` and
  `/etc/anofox`. If the data isn't world-readable, use `User=anofox` instead;
- puts the snapshot in a private `/var/tmp` (`PrivateTmp=yes`,
  `TMPDIR=/var/tmp`). The snapshot is a full copy of the attached databases, so
  it belongs on disk; `/tmp` is a RAM-backed tmpfs on many systems. On unix the
  snapshot files are unlinked as soon as they are attached. A server stopped by
  a signal leaves an empty `anofox-snap-*` directory, which goes away with the
  private temp directory;
- restarts on failure, including a SERVE_SQL error;
- keeps the control FIFO at `/run/anofox-dashboards/control`
  (`echo "SELECT anofox_serve_refresh(8080);" | sudo tee /run/anofox-dashboards/control`);
- limits resources: `MemoryMax`, `CPUQuota`, `TasksMax`, `LimitNOFILE` (see §6);
- sandboxes the process: `ProtectSystem=strict`, `NoNewPrivileges`, etc.

`systemd-analyze verify` accepts the unit once the paths exist.

### Container

```sh
docker build -f deploy/Dockerfile -t anofox-dashboards .
docker run -d --name dash -p 127.0.0.1:8080:8080 --read-only -v /tmp \
  --memory 2g --cpus 4 \
  -v $PWD/serve.sql:/etc/anofox/serve.sql:ro \
  -v $PWD/dashboards:/etc/anofox/dashboards:ro \
  -v $PWD/data:/data:ro anofox-dashboards
```

The build stage compiles the extension (Rust, wasm-pack for the embedded web
client, the DuckDB CLI for packaging and the smoke test). The runtime image is
`debian:bookworm-slim` with the CLI, the extension and `run-locked.sh`, running
as an unprivileged user under `tini`. As PID 1 without its own signal handler,
DuckDB would ignore `SIGTERM` and `docker stop` would take 10 s. With `tini` it
takes about half a second. `-v /tmp` gives the snapshot an anonymous volume on
disk. `--tmpfs /tmp` also works, but then the snapshot takes RAM. Two things
differ in a container:

- **`"bind": "0.0.0.0"`**: the port is reached from outside the container's
  loopback. Publish it on the host's loopback only (`-p 127.0.0.1:8080:8080`)
  and let the proxy reach it there. Without `auth`, the start message warns
  that the server is bound beyond loopback. That is expected here: the
  published port is what limits who can reach it.
- **`Host`**: a request through the published port carries
  `Host: 127.0.0.1:8080`, which counts as loopback on the server's port, so it
  passes. That only works if the host port equals the server's port: with
  `-p 127.0.0.1:9000:8080` the `Host` is `127.0.0.1:9000` and gets `421`, so
  either keep the ports equal or add `"127.0.0.1:9000"` to `hosts`. A proxy on
  another container network sends its own name, so list that name in `hosts`.

The image has no `curl`. A health check can use bash's `/dev/tcp` (as
`compose.yaml` does) or come from the orchestrator. For the control FIFO, pass
`-e ANOFOX_CONTROL=/tmp/control` and write to it with `docker exec`:

```sh
docker exec dash sh -c 'echo "SELECT anofox_serve_refresh(8080);" > /tmp/control'
```

## 3. Reverse proxy: TLS, authentication, `Host`

The server speaks plain HTTP and has no authentication, so put a proxy in front
of it that terminates TLS and authenticates viewers. The server checks two
headers on every request:

- **`Host`** must be a loopback name on its port, the `bind` address, or a
  name in `hosts`. Otherwise it answers `421`. Proxies forward the client's
  `Host` (Caddy by default, nginx with `proxy_set_header Host $http_host`), so
  list the public name: `"hosts": ["dash.example.com"]`. A name without a port
  matches any port.
- **`Origin`**, which browsers send on every `POST`, must match the `Host`.
  Otherwise the server answers `403`. Forward the `Host` exactly as the browser
  sent it, including a non-default port. nginx's `$host` drops the port and
  breaks this; use `$http_host`.

**Caddy** gets and renews certificates by itself:

```caddyfile
dash.example.com {
	basic_auth {
		viewer $2a$14$...   # caddy hash-password
	}
	reverse_proxy 127.0.0.1:8080
}
```

**nginx**: see [`deploy/nginx.conf`](../deploy/nginx.conf). Keep
`client_max_body_size` at or below the server's `max_body_bytes` and
`proxy_read_timeout` above its `timeout_ms`.

Basic auth only keeps strangers out: every viewer who logs in sees every
dashboard. For per-viewer data, the server can take the viewer's identity from
an authenticating proxy (oauth2-proxy, Caddy `forward_auth`, nginx
`auth_request`). It uses the `auth` option with a shared secret, and the
claims reach the SQL as `__claim_*` variables. See
[Login via an auth proxy](secure-serving.md#login-via-an-auth-proxy-access-control-p1)
for the options and tested proxy recipes. Things that matter for deployment:

- With `auth`, the server needs an access policy (`"policy": "/etc/anofox/policy.json"`,
  kept outside the dashboard folder; see
  [Row and column policies](secure-serving.md#row-and-column-policies-access-control-p2)),
  or `"policy": "none"` when the panels filter on the claims themselves —
  otherwise it refuses to start.
- `/healthz` stays reachable without a login (it then answers only
  `{"status":"ok"}`), so health checks need no secret.
- With `"bind": "0.0.0.0"`, set `trusted_proxies` to the proxy's address or
  network.
- The cache and the single-flight key include each viewer's claims. Panels
  that depend on the user are therefore cached per user, so the hit rates in
  §7 apply per user and per group of identical claims.

To give groups different dashboards without `auth`, run separate servers
(directories, ports) and route them at the proxy.

## 4. Several replicas behind a load balancer

Every request is self-contained: dashboard, panel, variable values, page. The
server keeps no session. So any number of identical servers can answer the same
viewers.

```
             ┌──────────── TLS + auth ────────────┐
 browsers ──▶│ proxy / load balancer (Caddy, nginx)│──▶ replica 1 (own snapshot, own cache)
             └─────────────────────────────────────┘──▶ replica 2 (own snapshot, own cache)
```

**Sticky or not.** Stickiness isn't needed for correct results, but it helps.
Route a viewer to the same replica (Caddy `lb_policy cookie`, nginx
`hash $remote_addr consistent`) because:

- *Cancellation is per replica.* When a viewer pages a `::PAGED` table quickly,
  each request supersedes the previous one (`X-Anofox-Supersede`). Only the
  replica that runs the older query can cancel it. Without stickiness the
  superseded queries run to the end on other replicas. The browser has already
  given up on them, so it's wasted work, not a wrong result.
- *The cache is per replica.* With N replicas and no stickiness, a combination
  of filters is computed up to N times before every replica has it cached.
  Popular dashboards still reach a high hit rate. For the load test's five
  shared combinations that's a few extra queries in total.
- *Snapshots may differ* while replicas refresh or restart one after another
  (§5). A sticky viewer sees one snapshot for all panels of a dashboard.

Without stickiness (plain round robin), everything still works. Expect more
executions and no cancellation across replicas.

**Health checks.** Use `GET /healthz`. It answers only after the snapshot is
built, so a starting replica gets no traffic before it is ready. The check must
send a `Host` the replica accepts: the public name (Caddy:
`health_headers { Host dash.example.com }`), or `127.0.0.1:<port>` from inside
the container. A check that sends the replica's address as `Host` gets `421`
and marks every replica down. Add passive checks too (Caddy `fail_duration` +
`lb_try_duration`, nginx `max_fails` + `proxy_next_upstream`). A replica that
stops is then skipped on the next request, not only after the next active
check. Panel requests are read-only `POST`s, so retrying one on another
replica is safe.

**Snapshot distribution.** Each replica snapshots what its SERVE_SQL attaches.
To give all replicas the same data, publish one immutable DuckDB file per data
version and point every replica at it. Options: a read-only volume, a file
synced from object storage before start, or `ATTACH 's3://…' (READ_ONLY)` with
`httpfs` loaded in SERVE_SQL. The snapshot is a copy, so the source may change
or disappear afterwards. To refresh, see §5. A replica's memory holds its own snapshot (§6),
whatever the replica count.

**Try it locally** ([`deploy/compose.yaml`](../deploy/compose.yaml)): two
replicas of the load-test dashboard behind Caddy with TLS (Caddy's local CA),
basic auth, a sticky cookie, and active and passive health checks. The image
is 216 MB, and each replica used about 190 MB with the 2M-row demo data:

```sh
docker compose -f deploy/compose.yaml up --build -d
curl -k -u viewer:demo https://localhost:8443/healthz      # {"status":"ok","dashboards":1,"epoch":1,...}
# open https://localhost:8443/d/ops (user viewer, password demo)
docker compose -f deploy/compose.yaml stop dash1           # its viewers move to dash2 at once
docker compose -f deploy/compose.yaml down
```

## 5. Refreshing the data

A server can rebuild its snapshot while it serves, without failed requests
([details](secure-serving.md)):

- **On a schedule:** `"refresh_s": 300` rebuilds it every 5 minutes from what
  the serving session sees.
- **On demand:** run `SELECT anofox_serve_refresh(8080);` in the serving
  session. Under `run-locked.sh` that means through the control FIFO:

  ```sh
  echo "SELECT anofox_serve_refresh(8080);" > /run/anofox/control
  ```

An attached file stays open, so replacing it on disk (a new file moved over
the old one) is not seen. Re-attach it in the same statement batch:

```sh
echo "DETACH warehouse; ATTACH '/data/warehouse.duckdb' AS warehouse (READ_ONLY);
SELECT anofox_serve_refresh(8080);" > /run/anofox/control
```

Views defined on `warehouse.*` (as in §1) keep working across the re-attach.
While a rebuild runs, the old and the new snapshot both exist: plan for twice
the snapshot's disk and memory. `/healthz` reports `epoch` (+1 per refresh)
and `refresh_failed`.

**Replicas** refresh independently, so for a short time they serve different
snapshots. Sticky routing (§4) keeps one viewer on one replica. To switch all
replicas at once instead, start new ones on the new file and drain the old
ones. Without an in-place refresh, the alternative is a rolling restart
(`docker compose restart dash1`, wait until it is healthy, then `dash2`). The
health check keeps traffic off a replica until its snapshot is ready.

## 6. Resource limits

**Memory.** Peak RSS of a server is about:

- the DuckDB CLI process: ~50 MB;
- the snapshot itself is a file in `$TMPDIR`. It doesn't count towards RSS
  unless `$TMPDIR` is a tmpfs, in which case it takes RAM equal to the size
  of the attached databases;
- the snapshot's data that queries touch: DuckDB's buffer pool, which pages it
  in from the snapshot file and holds it up to `memory_limit`;
- query working memory: sorts, hash tables, large pages, also under
  `memory_limit`;
- the result cache: up to `cache_mb`;
- results in flight, outside `memory_limit`: each of up to `threads` requests
  holds its query result and its JSON. That's tens of MB for the largest
  pages, statistics and distributions;
- one thread stack per worker and per DuckDB thread.

In the load test, ordinary viewers stayed under 450 MB on 2 million rows,
including a snapshot refresh. The abusive clients pushed the server to
1.1–1.4 GB with 16 workers, whether or not `memory_limit` was 512 MB, and to
0.8 GB with 4 workers (§7). A safe `MemoryMax`:

    memory_limit + cache_mb + threads × 60 MB + 200 MB

If SERVE_SQL also creates tables in memory, add their size: they live in the
CLI's own DuckDB instance, besides the snapshot. Attach a file instead.

**CPU.** Queries use up to `duckdb_threads` cores between them. The HTTP
workers (`threads`) mostly wait on the cache or on DuckDB. Give the process as
many cores as `duckdb_threads` (`--cpus`, `CPUQuota`).

**File descriptors and tasks.** Each viewer keeps a few keep-alive connections,
and each one holds a server thread. Allow `LimitNOFILE` and `TasksMax` above
the number of open connections. The proxy pools connections to the server, so
behind a proxy that is the proxy's pool size, not the number of viewers.

## 7. Sizing

`scripts/load-test.mjs` drives a locked server running the reference dashboard
[`examples/load-test/dash/ops.sql`](../examples/load-test/dash/ops.sql). The
dashboard has a period picker, a region switch, three KPIs, a line chart, a
bar chart and a `::PAGED` table over 2,000,000 order lines. The virtual viewers
send what the browser client sends: every panel of the dashboard one after the
other, then the table's first page and its column statistics, then pages,
sorting and a filter. Paging requests carry `X-Anofox-Supersede`. A run starts
a fresh server per scenario, with its data in a temporary database file, and
reads the server's log for executions and interruptions.

```sh
duckext/scripts/build-native.sh --release           # → /tmp/anofox_visualization.duckdb_extension
node scripts/load-test.mjs --ext /tmp/anofox_visualization.duckdb_extension \
    --duckdb ~/.cache/duckdb-1.5.6/duckdb --scenario all --json results.json
node scripts/load-test.mjs --ext … --scenario smoke    # ~1 min, exits 1 on a failure
```

`--cpus 0-3` pins the server to those cores (`taskset`), `--server-options`
passes the options JSON, and `--url` targets a running server instead (with
`--pid` for its RSS). The script's header lists all flags.

| Scenario | Viewers | What they do |
|---|---|---|
| burst | 100 | arrive at once on a fresh server and open the dashboard |
| cold | 20 | each picks its own period, so the cache can't help |
| hot | 200 | five filter combinations shared by everyone (presets) |
| refresh | 100 | the hot viewers, while the server rebuilds its snapshot every 5 s |
| scroll | 100 | page the table only: 10 superseding page requests 25 ms apart, random sort column |
| abuse | 20 + 10 | 20 clients sending the largest pages, statistics and distributions of every column, oversized and unknown requests, next to 10 ordinary viewers |

Viewers think for 1 s between actions, and each scenario runs for 30 s. So one
virtual viewer is much busier than a person reading a dashboard: count it as
several real viewers.

### Measured (2026-10-17)

Setup: Intel i9-13900H (6 P-cores + 8 E-cores, 20 hardware threads), 62 GB
RAM, Linux 6.18, DuckDB CLI 1.5.6, release build of this branch, Node 24. The
client ran on the same machine. It was a shared workstation: other jobs kept
the load average at 9–30 during the runs, so read the numbers as conservative.
Latency is per request as the client sees it, in ms. "Dashboard load" is the
full sequence of 16 requests for one dashboard. "refresh" rebuilds the snapshot
every 5 s (`refresh_s`).

**Whole machine**, `{"threads": 16}` (DuckDB's defaults: 20 threads, 80 % of
RAM):

| Scenario | Viewers | Req/s | Panel p50 / p95 / p99 | Page p50 / p95 / p99 | Dashboard load p50 / p95 | Executions | Cache / shared | 409 | Errors | Peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|---|
| burst | 100 | 514 | 107 / 211 / 277 | 98 / 548 / 581 | 2,329 / 3,024 | 1,201 | 25 % | 0 | 0 | 347 |
| cold | 20 | 157 | 5.9 / 34 / 65 | 12 / 42 / 74 | 166 / 536 | 4,432 | 15 % | 0 | 0 | 344 |
| hot | 200 | 1,667 | 1.1 / 14 / 27 | 0.6 / 6.9 / 40 | 21 / 419 | 120 | 99.8 % | 0 | 0 | 205 |
| refresh | 100 | 838 | 2.7 / 18 / 29 | 0.8 / 14 / 90 | 67 / 288 | 720 | 97.3 % | 0 | 0 | 445 |
| scroll | 100 | 124 | – | 1,882 / 4,146 / 4,723 | – | 8,716 | 0.1 % | 4,700 | 0 | 722 |
| abuse | 20 + 10 | 58 | 48 / 149 / 222 | 58 / 149 / 432 | 1,095 / 1,715 | 2,952 | – | 0 | 0 | 1,373 |

**Two P-cores** (`--cpus 0-3`: 4 hardware threads),
`{"threads": 16, "duckdb_threads": 4, "memory_limit": "2GB"}`:

| Scenario | Viewers | Req/s | Panel p50 / p95 / p99 | Page p50 / p95 / p99 | Dashboard load p50 / p95 | Cache / shared | 409 | Errors | Peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|
| burst | 100 | 138 | 404 / 913 / 1,172 | 579 / 2,563 / 2,658 | 8,749 / 11,499 | 25 % | 0 | 0 | 285 |
| cold | 5 | 39 | 3.8 / 36 / 68 | 11 / 42 / 71 | 179 / 446 | 14 % | 0 | 0 | 191 |
| cold | 20 | 116 | 9.3 / 181 / 258 | 27 / 131 / 607 | 926 / 2,114 | 14 % | 0 | 0 | 283 |
| cold | 50 | 136 | 213 / 464 / 576 | 331 / 666 / 1,433 | 4,386 / 5,643 | 15 % | 7 | 0 | 299 |
| hot | 50 | 418 | 0.6 / 20 / 52 | 0.4 / 7.7 / 166 | 11 / 638 | 99.1 % | 0 | 0 | 171 |
| hot | 200 | 1,659 | 0.9 / 24 / 49 | 0.4 / 9.3 / 322 | 16 / 820 | 99.8 % | 0 | 0 | 177 |
| hot | 500 | 4,071 | 2.0 / 18 / 68 | 0.5 / 10 / 176 | 38 / 1,165 | 99.9 % | 0 | 0 | 179 |
| refresh | 100 | 755 | 1.6 / 53 / 102 | 0.5 / 66 / 649 | 32 / 724 | 97.2 % | 0 | 0 | 422 |
| scroll | 100 | 37 | – | 7,219 / 19,028 / 20,194 | – | 0.7 % | 2,201 | 0 | 561 |
| abuse | 20 + 10 | 35 | 141 / 420 / 530 | 169 / 532 / 2,351 | 2,526 / 5,491 | – | 0 | 0 | 1,068 |

"Cache / shared" is the share of successful requests answered without running
a query: from the cache, or by waiting for an identical running request. Even
"cold" viewers share the dashboard's inputs and layout statements. A server
started in 2.1–3.2 s, including copying the 2M-row snapshot.

### What the numbers say

- **Popular dashboards are cheap.** When viewers share filter values (presets,
  defaults), the cache answers 99.8 % of the requests. Two P-cores served 500
  busy viewers at 4,000 requests/s with a panel p95 of 18 ms. The first load of
  each combination is the slow tail: dashboard-load p95 is about 1 s, p50 is
  38 ms.
- **Distinct filters need cores.** When every viewer picks its own values,
  each request is a DuckDB query over the data (here 2M rows).
  - On two P-cores, 5 busy viewers loaded a dashboard in 0.18 s (p95 0.45 s);
    20 took 0.93 s (p95 2.1 s); 50 took 4.4 s.
  - The whole machine handled 20 in 0.17 s (p95 0.54 s).
  - As a rule of thumb for data of this size: one physical core for every 3–4
    busy viewers who all pick different filters keeps a dashboard load under
    a second. Real viewers who read before clicking need far fewer cores.
- **Refreshing under load is smooth.** Six rebuilds in 30 s under 100 viewers
  caused no errors and no latency cliff.
  - Each rebuild starts an empty cache, so the hit rate fell from 99.8 % to
    97 %. Panel p95 went from 14 to 18 ms on the whole machine, and from 24 to
    53 ms on two P-cores.
  - Peak RSS grew by the second snapshot that exists during a rebuild.
- **A dashboard load is 16 requests in sequence.** The client fetches the
  panels one after another, so a load takes the sum of their latencies, not the
  slowest one. More panels mean a slower first load, even on a fast server.
- **Cancellation works.** In "scroll", every superseded page request was
  answered `409` and its query interrupted. No query ran to completion for a
  request nobody wanted any more. 100 viewers dragging the Scroll view
  non-stop, each sorting up to 1M rows by a random column, still saturate any
  box (p50 1.9 s on the whole machine). Sorting a large table by an arbitrary
  column is the most expensive request a viewer can make.
- **Abuse is contained but costs memory.**
  - 20 clients sent the largest pages (10,000 rows), statistics and value
    distributions of every column, and oversized and unknown requests, without
    pause. Ordinary viewers next to them got no errors. Their panels slowed to
    a p50 of 48 ms (1.1 ms in the "hot" run, which has the same viewers without
    the abusers).
  - Rejected requests were answered `400`, `404` and `413`.
  - Peak RSS reached 1.4 GB. `memory_limit` didn't bound it: with
    `"memory_limit": "512MB"` the peak was still 1.1 GB, and with 4 workers
    instead of 16 it was 0.8 GB. Results in flight and the allocator's caches
    sit outside DuckDB's buffer pool.
  - **Size memory per worker** (§6), and keep heavy requests out with proxy
    rate limits.
- **Idle footprint:** about 90 MB RSS with the 2M-row snapshot (the snapshot
  stays on disk until queries touch it).

**Before the tiny_http fixes of #93** (same machine, the day before): in a
20-viewer "cold" run, 10 requests were never answered within the 2-minute
timeout. The HTTP server's thread pool had left their connections waiting.
Every response bigger than about one TCP segment also waited ~40 ms for a
delayed ACK: "hot" panel p95 was 47 ms (now 14), and a dashboard load took
209 ms (now 21). The load test found both bugs.

### Against the targets of the serving plan

| Target ([plan §5](plans/plan-duckdb2-server.md#5-load-test-plan)) | Goal | Measured |
|---|---|---|
| panel p50 / p95, hot, 200 viewers | ≤ 50 / 300 ms | 1.1 / 14 ms (2 P-cores: 0.9 / 24 ms) |
| dashboard load p95, cold, 20 viewers | ≤ 2 s | 0.54 s (2 P-cores: 2.1 s) |
| cache hit rate, hot | ≥ 90 % | 99.8 % |
| wasted executions, scroll | ≤ 10 % | 0 %: every superseded query was interrupted |
| refresh under load (L4) | no error spike or latency cliff | 0 errors; panel p95 14 → 18 ms |
| errors (not 4xx by design) | 0 | 0 in every scenario |
| RSS ≤ snapshot + `memory_limit` + cache | | missed under abuse: results in flight sit outside `memory_limit` (§6) |

## Checklist

- [ ] SERVE_SQL attaches only what the dashboards need (the snapshot contains
      every attached DuckDB database).
- [ ] `bind` is loopback, or the port is reachable only from the proxy.
- [ ] `hosts` lists the public name; the proxy forwards `Host` with its port.
- [ ] TLS and authentication at the proxy; proxy timeout above `timeout_ms`.
- [ ] `memory_limit`, `duckdb_threads`, `cache_mb` and `threads` set;
      `MemoryMax`/`--memory` above them (§6); `$TMPDIR` on disk.
- [ ] Health check on `/healthz` with an accepted `Host`.
- [ ] Replicas: sticky routing; every replica serves the same data file.
