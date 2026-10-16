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
For the 2-million-row load-test data that took 2–5 s. The snapshot never
changes while the server runs. To serve new data, start a new server (§5).

The process must stay in the foreground. The DuckDB CLI exits when its stdin
ends, and the servers stop with it. [`deploy/run-locked.sh`](../deploy/run-locked.sh)
gives the CLI a stdin that never ends: a FIFO it holds open for both reading and
writing. It then `exec`s the CLI, so signals reach DuckDB directly: `SIGTERM`
stops it. With `-bail`, an error in SERVE_SQL exits with status 1 rather than
leaving a process that serves nothing.

```sh
DUCKDB=/opt/anofox/duckdb deploy/run-locked.sh /etc/anofox/serve.sql
curl -s http://127.0.0.1:8080/healthz      # {"status":"ok","dashboards":3}
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
- restarts on failure, including a SERVE_SQL error (`-bail`);
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
  and let the proxy reach it there.
- **`Host`**: a request through the published port carries
  `Host: 127.0.0.1:8080`, which counts as loopback on the server's port, so it
  passes. That only works if the host port equals the server's port: with
  `-p 127.0.0.1:9000:8080` the `Host` is `127.0.0.1:9000` and gets `421`, so
  either keep the ports equal or add `"127.0.0.1:9000"` to `hosts`. A proxy on
  another container network sends its own name, so list that name in `hosts`.

The image has no `curl`. A health check can use bash's `/dev/tcp` (as
`compose.yaml` does) or come from the orchestrator.

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

For single sign-on, put an authenticating proxy (oauth2-proxy, Authelia,
Cloudflare Access) in the same place. The server doesn't see who the viewer
is: every authenticated viewer can open every dashboard the server serves. To
give groups different dashboards, run separate servers (directories, ports) and
route them at the proxy.

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
- *Snapshots may differ* while replicas restart one after another (§5). A sticky
  viewer sees one snapshot for all panels of a dashboard.

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
`httpfs` loaded in SERVE_SQL. The snapshot is copied at startup, so the source
may change or disappear afterwards. To refresh, start replicas on the new file
and drain the old ones (§5). A replica's memory holds its own snapshot (§6),
whatever the replica count.

**Try it locally** ([`deploy/compose.yaml`](../deploy/compose.yaml)): two
replicas of the load-test dashboard behind Caddy with TLS (Caddy's local CA),
basic auth, a sticky cookie, and active and passive health checks. The image
is 216 MB, and each replica used about 190 MB with the 2M-row demo data:

```sh
docker compose -f deploy/compose.yaml up --build -d
curl -k -u viewer:demo https://localhost:8443/healthz      # {"status":"ok","dashboards":1}
# open https://localhost:8443/d/ops (user viewer, password demo)
docker compose -f deploy/compose.yaml stop dash1           # its viewers move to dash2 at once
docker compose -f deploy/compose.yaml down
```

## 5. Refreshing the data

Until the server can refresh its snapshot in place (#21), new data needs a
restart. To avoid downtime:

- **One process:** start a second server on another port with the new data,
  switch the proxy's upstream, then stop the old one. With Caddy that means
  editing the `reverse_proxy` target and `caddy reload`; with nginx, the
  `server` lines and `nginx -s reload`.
- **Replicas:** restart them one at a time (`docker compose restart dash1`,
  wait until it is healthy, then `dash2`). The health check keeps traffic off a
  replica until its new snapshot is ready.

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

In the load test, ordinary viewers stayed under a few hundred MB on 2 million
rows. The abusive clients pushed the server to 1.2 GB with 16 workers,
whether or not `memory_limit` was 512 MB. With 4 workers it was 550 MB (§7).
A safe `MemoryMax`:

    memory_limit + cache_mb + threads × 50 MB + 100 MB

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
| scroll | 100 | page the table only: 10 superseding page requests 25 ms apart, random sort column |
| abuse | 20 + 10 | 20 clients sending the largest pages, statistics and distributions of every column, oversized and unknown requests, next to 10 ordinary viewers |

Viewers think for 1 s between actions, and each scenario runs for 30 s. So one
virtual viewer is much busier than a person reading a dashboard: count it as
several real viewers.

### Measured (2026-10-16)

Setup: Intel i9-13900H (6 P-cores + 8 E-cores, 20 hardware threads), 62 GB
RAM, Linux 6.18, DuckDB CLI 1.5.6, release build, Node 24. The client ran on
the same machine. It was a shared workstation: other jobs kept the load
average at 11–46 during the runs, so read the numbers as conservative. Latency
is per request as the client sees it, in ms. "Dashboard load" is the full
sequence of 16 requests for one dashboard.

**Whole machine**, `{"threads": 16}` (DuckDB's defaults: 20 threads, 80 % of
RAM):

| Scenario | Viewers | Req/s | Panel p50 / p95 / p99 | Page p50 / p95 / p99 | Dashboard load p50 / p95 | Executions | Cache / shared | 409 | Errors | Peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|---|
| burst | 100 | 381 | 167 / 289 / 379 | 260 / 509 / 668 | 3,138 / 4,089 | 1,201 | 25 % | 0 | 0 | 350 |
| cold | 20 | 154 | 7.7 / 47 / 88 | 14 / 43 / 72 | 218 / 689 | 4,261 | 15 % | 0 | 0 | 330 |
| hot | 200 | 1,670 | 1.5 / 15 / 39 | 0.6 / 6.2 / 25 | 28 / 442 | 120 | 99.8 % | 0 | 0 | 210 |
| scroll | 100 | 80 | – | 3,159 / 6,250 / 6,954 | – | 6,166 | 0.1 % | 3,439 | 0 | 609 |
| abuse | 20 + 10 | 53 | 62 / 170 / 272 | 72 / 208 / 752 | 1,162 / 2,462 | 2,343 | – | 0 | 0 | 1,187 |

**Two P-cores** (`--cpus 0-3`: 4 hardware threads),
`{"threads": 16, "duckdb_threads": 4, "memory_limit": "2GB"}`:

| Scenario | Viewers | Req/s | Panel p50 / p95 / p99 | Page p50 / p95 / p99 | Dashboard load p50 / p95 | Cache / shared | 409 | Errors | Peak RSS MB |
|---|---|---|---|---|---|---|---|---|---|
| burst | 100 | 125 | 411 / 1,108 / 1,479 | 946 / 2,302 / 3,228 | 9,420 / 12,646 | 25 % | 0 | 0 | 267 |
| cold | 5 | 39 | 5.0 / 47 / 84 | 13 / 64 / 192 | 278 / 569 | 14 % | 0 | 0 | 205 |
| cold | 20 | 104 | 13 / 228 / 337 | 41 / 205 / 808 | 1,250 / 2,365 | 15 % | 2 | 0 | 258 |
| cold | 50 | 144 | 201 / 444 / 596 | 239 / 570 / 1,284 | 4,096 / 5,165 | 15 % | 2 | 0 | 316 |
| hot | 50 | 422 | 0.7 / 19 / 53 | 0.4 / 5.1 / 113 | 11 / 709 | 99.1 % | 0 | 0 | 168 |
| hot | 200 | 1,649 | 0.7 / 27 / 71 | 0.4 / 6.4 / 124 | 12 / 1,152 | 99.8 % | 0 | 0 | 183 |
| hot | 500 | 4,089 | 1.3 / 17 / 72 | 0.4 / 14 / 202 | 22 / 1,144 | 99.9 % | 0 | 0 | 186 |
| scroll | 100 | 38 | – | 6,447 / 17,676 / 18,787 | – | 0.2 % | 2,312 | 0 | 552 |
| abuse | 20 + 10 | 37 | 132 / 429 / 547 | 160 / 694 / 2,557 | 2,425 / 5,457 | – | 2 | 0 | 846 |

"Cache / shared" is the share of successful requests answered without running
a query: from the cache, or by waiting for an identical running request. Even
"cold" viewers share the dashboard's inputs and layout statements. A data
server started in 2.2–5.1 s, including copying the 2M-row snapshot.

### What the numbers say

- **Popular dashboards are cheap.** When viewers share filter values (presets,
  defaults), the cache answers 99.8 % of the requests. Two P-cores served 500
  busy viewers at 4,000 requests/s with a panel p95 of 17 ms. The first load of
  each combination is the slow tail: dashboard-load p95 is about 1 s, p50 is 22 ms.
- **Distinct filters need cores.** When every viewer picks its own values,
  each request is a DuckDB query over the data (here 2M rows).
  - On two P-cores, 5 busy viewers loaded a dashboard in 0.28 s (p95 0.57 s);
    20 took 1.25 s (p95 2.4 s); 50 took 4.1 s.
  - The whole machine handled 20 in 0.22 s (p95 0.69 s).
  - As a rule of thumb for data of this size: one physical core for every 3–4
    busy viewers who all pick different filters keeps a dashboard load under
    a second. Real viewers who read before clicking need far fewer cores.
- **A dashboard load is 16 requests in sequence.** The client fetches the
  panels one after another, so a load takes the sum of their latencies, not the
  slowest one. More panels mean a slower first load, even on a fast server.
- **Cancellation works.** In "scroll", every superseded page request was
  answered `409` and its query interrupted. No query ran to completion for a
  request nobody wanted any more. 100 viewers dragging the Scroll view
  non-stop, each sorting up to 1M rows by a random column, still saturate any
  box (p50 3 s on the whole machine). Sorting a large table by an arbitrary
  column is the most expensive request a viewer can make.
- **Abuse is contained but costs memory.**
  - 20 clients sent the largest pages (10,000 rows), statistics and value
    distributions of every column, and oversized and unknown requests, without
    pause. Ordinary viewers next to them got no errors. Their panels slowed to
    a p50 of 62 ms (1.5 ms in the "hot" run, which has the same viewers without
    the abusers).
  - Rejected requests were answered `400`, `404` and `413`.
  - Peak RSS reached 1.2 GB. `memory_limit` didn't change that much: with
    `"memory_limit": "512MB"` the peak was still 1.1 GB. Each request in
    flight holds its result outside DuckDB's buffer pool, and the number of
    requests in flight is `threads`: with `"threads": 4` the peak dropped to
    550 MB.
  - **Bound memory with `threads`** (HTTP workers) as well as `memory_limit`,
    and keep heavy requests out with proxy rate limits.
- **Idle footprint:** about 90 MB RSS with the 2M-row snapshot (the snapshot
  stays on disk until queries touch it).

**Before the tiny_http fixes** (same machine, the same day): in a 20-viewer
"cold" run, 10 requests were never answered within the 2-minute timeout. The
HTTP server's thread pool had left their connections waiting
(`vendor/tiny_http/README.anofox.md`). Every response bigger than about one
TCP segment also waited ~40 ms for a delayed ACK: "hot" panel p95 was 47 ms
(now 15), and a dashboard load took 209 ms (now 28).

### Against the targets of the serving plan

| Target ([plan §5](plans/plan-duckdb2-server.md#5-load-test-plan)) | Goal | Measured |
|---|---|---|
| panel p50 / p95, hot, 200 viewers | ≤ 50 / 300 ms | 1.5 / 15 ms (2 P-cores: 0.7 / 27 ms) |
| dashboard load p95, cold, 20 viewers | ≤ 2 s | 0.69 s (2 P-cores: 2.4 s) |
| cache hit rate, hot | ≥ 90 % | 99.8 % |
| wasted executions, scroll | ≤ 10 % | 0 %: every superseded query was interrupted |
| errors (not 4xx by design) | 0 | 0 in every scenario |
| RSS ≤ snapshot + `memory_limit` + cache | | missed under abuse: per-request results sit outside `memory_limit`, so bound `threads` |
| L4 refresh under load | | not run: needs the snapshot refresh (#21) |

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
