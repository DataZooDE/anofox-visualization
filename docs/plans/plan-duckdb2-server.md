# Plan: DuckDB 2.0 as a multi-user serving backend

Status: investigation (2026-10-16), based on master `4f27257`, DuckDB pinned
to v1.5.6 (`.duckdb-version`). Resolves issue #16.
Related: [secure-serving.md](../secure-serving.md),
[plan-anofox-visualization.md](plan-anofox-visualization.md).

**Verdict.** **Go** for scaling the embedded locked server (option A, phases
S1–S4 below). **Not yet** for delegating panel queries to a shared DuckDB 2.0
/ Quack server (option B): 2.0.0 is not released, and the blockers listed in
§6 must be closed first. Re-evaluate at 2.0.1 (tentatively 2026-11-18).

---

## 1. Today (evidence)

| Piece | Where | What it does |
|---|---|---|
| Locked server | `duckext/src/serve.rs:477` `start_locked` | snapshot → validate every statement (exactly one, `serve.rs:500`) → bind `127.0.0.1:port` (`serve.rs:513`) → worker pool |
| Snapshot | `serve.rs:607` `build_snapshot` | `COPY FROM DATABASE` into a private 0700 dir, new in-memory DuckDB, re-`ATTACH … (READ_ONLY)`, load extensions, then `enable_external_access=false`, `lock_configuration=true` |
| Workers | `serve.rs:89` `spawn_workers` | `threads` (default 4) tiny_http threads → at most 4 queries in flight; the rest queue in the accept loop |
| Panel request | `serve.rs:802` `locked_panel` | fresh connection per request, `USE`, `SET VARIABLE` with typed literals, re-run setup stmts (TEMP only), panel as one prepared stmt |
| Limits | `serve.rs:344` `LockedOptions`, `duckext/src/db.rs:191` `with_deadline`, `db.rs:337` `json_rows` | row cap (`LIMIT max_rows+1` → 413), timeout via `duckdb_interrupt` watchdog, body cap |
| Request schema | `src/host/serving.rs:411` `parse_panel_request` | `{dashboard, panel, vars, page}`; unknown fields rejected; vars → literals only |
| `::PAGED` SQL | `serving.rs:578` `page_sql` / `:590` `page_sql_inline` | one builder for page / count / stats / distinct; user values bound as `?` (or escaped for Wasm / `/query`) |
| Client | `web/app.js:913` `servedRequest` | `POST /api/panel`; no `AbortController`, no dedupe; DuckDB-Wasm (`app.js:1143`) only when not served |
| Static `serve` bin | `src/bin/serve.rs:385,476` | `duckdb -readonly` per panel + a TTL render cache keyed by dashboard + resolved params |

Constraints today: all load lands on one process; the snapshot is frozen at
start (restart to refresh, `secure-serving.md` "The snapshot is taken at
startup"); setup statements re-run per request; there is no result cache in
the locked server; a superseded scroll page still runs to completion.

## 2. What DuckDB 2.0 / the ecosystem provides (verified 2026-10-16)

| Fact | Source |
|---|---|
| 2.0.0 ("Cyanoptera") is **not released**; release calendar says 2026-10-21, 2.0.1 2026-11-18, "dates are tentative". Alpha builds exist, "not production-ready"; feature freeze on `v2.0-cyanoptera`. | [release calendar](https://duckdb.org/release_calendar), [Try v2.0-dev (2026-09-02)](https://duckdb.org/2026/09/02/try-duckdb-20-alpha) |
| Client/server = the **Quack** protocol (`quack` extension, core, autoloaded). Experimental since v1.5.3 ("protocol, function names … subject to change"); goes 0.x → 1.0 with 2.0. | [Quack ext docs](https://duckdb.org/docs/current/core_extensions/quack), [v2.0 preview (2026-08-17)](https://duckdb.org/2026/08/17/duckdb-20-highlights) |
| Server: `quack_serve(uri, token :=, allow_other_hostname := false, disable_ssl :=)`, `quack_stop(uri)`. Client: `ATTACH 'quack:host' AS r (TOKEN …)` or stateless `quack_query(uri, query, token :=)`. | [Quack reference](https://duckdb.org/docs/current/quack/reference.html) |
| Over HTTP, one round trip per query, DuckDB-internal serialization (not Arrow). Multiple processes can write one database concurrently through the server. | [Quack blog (2026-05-12)](https://duckdb.org/2026/05/12/quack-remote-protocol) |
| Security: random token per server; hooks `quack_authentication_function(session_id, client_token, server_token)` and `quack_authorization_function(connection_id, query)` — the latter sees the **raw SQL text** on every PREPARE; the docs call regex filtering "illustrative, not robust" and recommend a `READ_ONLY` attach or statement-type checks. No TLS in the server; binds localhost; "do not expose Quack directly to the internet", use a reverse proxy. | [Quack security](https://duckdb.org/docs/current/quack/security.html) |
| DuckDB-Wasm can speak Quack (HTTP). | [Quack blog](https://duckdb.org/2026/05/12/quack-remote-protocol) |
| Concurrency (1.x): one read-write process **or** many read-only processes per file; in-process MVCC + optimistic concurrency. Multi-process access = Quack (or DuckLake with a Postgres catalog). | [Concurrency docs](https://duckdb.org/docs/current/connect/concurrency) |
| 2.0 also brings async I/O throughout the engine (remote Parquet/CSV reads parallelise better). | [v2.0 preview](https://duckdb.org/2026/08/17/duckdb-20-highlights) |
| MotherDuck: read-scaling tokens → `SELECT`-only access to a pool of replicas (default 4, up to 16), eventually consistent ("within minutes"; `CREATE SNAPSHOT` + `REFRESH DATABASES` to force), `session_name` pins a client to one replica for cache affinity. Wasm SDK with dual execution (browser + cloud). | [Read scaling](https://motherduck.com/docs/key-tasks/authenticating-and-connecting-to-motherduck/read-scaling/), [Wasm SDK blog](https://motherduck.com/blog/building-data-applications-with-motherduck/) |
| Arrow Flight: community `airport` extension (client side; server is your own). | [airport](https://duckdb.org/community_extensions/extensions/airport) |

**Not documented / unverified** (treat as unknown until tested on 2.0.0):
prepared-statement parameters over Quack; whether `duckdb_interrupt` on the
client cancels the remote query; whether `SET VARIABLE` / `getvariable()` and
TEMP objects live on the client or the server of an `ATTACH`ed Quack catalog;
the `CONNECT` statement named in the 2.0 preview (absent from the Quack docs);
server-side per-connection resource limits; C-API extension ABI compatibility
of our `duckext` with 2.0 (the alpha post only says community extensions can
build against the v2.0 branch).

## 3. Options

**A — Embedded engine, scaled** (stay on 1.5.x, works with 2.0 later):
keep the private read-only snapshot, add a result cache + single-flight,
superseded-request cancellation, atomic snapshot refresh, a configurable bind,
and run N identical replicas behind a load balancer.

**B — Shared DuckDB 2.0 server via Quack**: the locked server becomes a thin
web tier; it `ATTACH`es `quack:dbhost` and sends its *own* panel SQL there. The
DB server holds the live database; ETL writes to it concurrently.

**C — MotherDuck**: the web tier connects with a read-scaling token; panels run
on MotherDuck replicas.

**D — Browser speaks Quack directly** (Wasm → Quack server). Rejected
up front: the client would send SQL, and the only gate is an authorization
callback over SQL text — exactly the model `secure-serving.md` rules out.

| Criterion | A embedded+cache | B Quack server | C MotherDuck |
|---|---|---|---|
| No client SQL | yes (unchanged) | yes, if only the web tier holds the token | yes, same |
| Bound params | yes (`page_sql`) | unknown → fall back to `page_sql_inline` | supported by the client API |
| Typed var literals | yes (`SET VARIABLE`) | unknown where variables resolve (blocker) | yes, on the client session |
| Row cap | yes (`json_rows`) | yes (wrapping `LIMIT` is pushed down) | yes |
| Timeout / cancel | yes (`duckdb_interrupt`) | unknown across the wire (blocker) | client interrupt; server-side limits by plan |
| Read-only | snapshot `READ_ONLY` + locked config | must be enforced on the server: read-only attach of a second catalog, or a statement-type authorization hook | read-scaling token is `SELECT`-only |
| External access off | yes | server-wide setting, conflicts with ETL needing it | n/a (managed) |
| Many viewers | cache + single-flight dominate; one box's cores | shared engine, but still one box; web tier scales | up to 16 replicas |
| Freshness | snapshot refresh every N min (S3) | live (MVCC) | eventual, minutes |
| Deployment | one binary/extension; N replicas each with a snapshot | 2 tiers, token distribution, proxy for TLS | SaaS account, data leaves premises |
| DuckDB-Wasm | unchanged | unchanged (browser never talks Quack) | Wasm SDK possible, but needs a client token (not for untrusted viewers) |
| Ready | now | after 2.0.0 + blockers | now, commercial |

## 4. Recommended architecture

Option A now, structured so option B is a drop-in backend later:

```
browser ──POST /api/panel──▶ web tier (locked server)
                               ├─ parse_panel_request / page_sql (unchanged)
                               ├─ result cache + single-flight   (S1)
                               ├─ supersede/cancel registry      (S2)
                               └─ PanelBackend trait
                                    ├─ Snapshot  (today, + refresh S3)
                                    └─ Quack     (S6, after 2.0)
```

| Step | Change | Files |
|---|---|---|
| **S1** cache | LRU keyed by `(snapshot epoch, dashboard, panel, canonical vars JSON, page)`; correct by construction because the snapshot is immutable. Single-flight: concurrent identical requests share one execution. Options `cache_mb` (default 64), `0` disables. `Cache-Control: private, max-age=0` + `ETag` on the epoch-qualified key. | `serve.rs` `locked_panel`, `LockedOptions` |
| **S2** cancel | Client: one `AbortController` per (panel, page kind) in `servedRequest`, aborted when a newer request replaces it; send `X-Anofox-Supersede: <random viewer id>/<panel>/<kind>`. Server: map key → `Interrupter`; a newer request with the same key interrupts the older one (→ 499-style `409`). 128-bit viewer id, so one viewer cannot cancel another's. | `web/app.js:913`, `serve.rs`, `db.rs:232` |
| **S3** refresh | `options.refresh_s`: rebuild the snapshot in the background, swap `Arc<Snapshot>` atomically, bump the epoch (invalidates S1), drop the old one when its last request ends; `/healthz` reports epoch + age. Also `anofox_serve_refresh(port)`. | `serve.rs:607`, `Locked` |
| **S4** deploy | `options.bind` (default `127.0.0.1`; non-loopback allowed in locked mode only, mirrors `serve --bind`), `memory_limit` / per-query `threads` options; doc a stateless N-replica setup behind nginx/Caddy (TLS + auth at the proxy, sticky sessions not needed — requests are self-contained). | `serve.rs:513`, `docs/secure-serving.md` |
| **S5** measure | Load test (§5) before and after S1/S2; publish numbers in this file. | `scripts/loadtest/` |
| **S6** Quack (gated) | `PanelBackend::Quack { uri, token }`: setup + panel sent as one server-owned SQL text via `quack_query` with inlined escaped values (`page_sql_inline`), vars inlined as typed literals in place of `getvariable()`; DB server attaches data `READ_ONLY` for this catalog. Only after §6 is closed. | new `serve_quack.rs` |

## 5. Load-test plan

- **Tool**: [k6](https://k6.io) (scripted scenarios, p50/p95/p99, ramping VUs),
  `oha` for quick single-endpoint checks; resource use via
  `pidstat -r -u -h 1` on the server PID and DuckDB `duckdb_memory()`.
- **Reference dashboard**: `examples/serve-dashboards/dash/m5_analytics.sql`
  (charts, KPIs, a table) on the M5 sample, with its detail table switched to
  `::PAGED` (no example uses `::PAGED` yet — add one as part of S5), served with
  `anofox_serve_dashboards('examples/serve-dashboards/dash', 8095)`, 8-core box,
  `threads` = 8.
- **Virtual viewer**: open `/d/m5_analytics`, fire all panel requests
  concurrently (like the client), think 5 s, change one variable (re-fire),
  scroll the paged table (10 page requests at 200 ms, each superseding the
  previous), sort + filter once.

| Scenario | Viewers | Mix | Purpose |
|---|---|---|---|
| L1 cold | 1 → 50 ramp | unique vars per viewer (cache useless) | engine ceiling |
| L2 hot | 50 → 500 ramp | 5 var combinations shared by all | cache + single-flight |
| L3 scroll | 100 | paged-scroll only | cancellation (S2) |
| L4 refresh | 100 steady | L2 mix, snapshot refresh every 60 s | no error spike or latency cliff on swap |
| L5 abuse | 20 | max `page.limit`, wide filters, timeouts | limits hold, others unaffected |

| Metric | Target (8 cores, after S1–S3) |
|---|---|
| panel latency p50 / p95 (L2, 200 viewers) | ≤ 50 ms / ≤ 300 ms |
| full dashboard load p95 (L1, 20 viewers) | ≤ 2 s |
| cache hit rate (L2) | ≥ 90 % |
| wasted executions (L3: completed but superseded) | ≤ 10 % of page requests |
| errors (non-4xx-by-design) | 0 |
| RSS | ≤ snapshot size + `memory_limit` + cache |

Acceptance for B later: rerun L1–L4 against `PanelBackend::Quack` on the same
hardware; adopt only if p95 is within 1.5× of A and the §6 blockers are closed.

## 6. Blockers for option B ("not yet")

1. **2.0.0 unreleased** (calendar: 2026-10-21, tentative); Quack is 0.x and
   explicitly subject to change until then. Our `duckext` builds on the
   v1.5.6 C API; 2.0 compatibility of the C-API extension is untested.
2. **Security parity unproven**: no documented parameter binding over Quack;
   unknown whether `SET VARIABLE` / TEMP setup objects resolve server-side on
   an `ATTACH`ed catalog; the server's only per-query gate is an
   authorization callback over SQL text.
3. **Read-only + locked config on a live, writable server**: ETL needs write
   and external access on the same instance; viewers need a `READ_ONLY`
   catalog and `enable_external_access=false`. Needs a verified recipe
   (e.g. a second read-only attach of the same file, or a read-only Quack
   server per catalog).
4. **Timeout/cancel across the wire**: must verify `duckdb_interrupt` on the
   client stops the remote query (else a 30 s timeout still burns the
   server).
5. **Resource isolation**: no documented per-connection memory/thread limits
   on a Quack server; one heavy dashboard can starve others.
6. **Scale-out not solved by B alone**: the Quack server is still one
   process; B buys *live data*, not more cores. For read scale-out, N
   replicas of A (or MotherDuck read scaling) remain the answer.

Re-test checklist on 2.0.0: (2)–(5) as sqllogictests + an S6 spike against
`quack_serve` on localhost; then decide.
