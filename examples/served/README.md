# Served dashboards: a bakery chain on the locked server

A made-up chain of 12 bakeries in the North, the Midlands and the South of
England, served to viewers with `anofox_serve_dashboards`, the locked,
read-only server ([trust model](../../docs/secure-serving.md)). Two dashboards:

- **How did our bakeries sell last week?** Revenue and items sold, by region,
  store and product, with a 10,000-row daily sales table that pages in DuckDB.
- **What will each store sell in the next four weeks?** A forecast run against
  the sales so far, with 80 % and 50 % bands, per store.

The example shows the server end to end:

| | What you see | Server feature |
|---|---|---|
| Result cache, single-flight | Revisiting a panel or a filter value answers from the cache; identical requests at the same moment run once | `cache_mb` (#73) |
| Superseded requests | Paging or scrolling the sales table fast cancels the queries of the pages you skipped | `X-Anofox-Supersede` (#73) |
| Refresh without a restart | `serve.sh week` loads a new week of sales; the next click shows it, nobody's page breaks | `anofox_serve_refresh`, `refresh_s` (#79) |
| A live data folder | `serve.sh run trend` writes a forecast run; it can be opened at once with `?v.run=<id>` | `data`, `path_vars` (#79) |
| Only queries run on the data folder | Every panel and setup statement is checked by DuckDB's parser at start | #87 |
| Login, rows per user | Alice sees the North, Bob the Midlands and the South, Carol (admins) everything | `auth` (#78, see below) |

## Start

```sh
examples/served/serve.sh start
```

Then open <http://127.0.0.1:8910/>. Ctrl-C stops the server.

You need a DuckDB 1.5 CLI (`duckdb` on `PATH`, or `DUCKDB=/path/to/duckdb`).
On the first start the script builds the native extension into
`examples/served/work/` (Rust and `wasm-pack`, a few minutes; see
[BUILD.md](../../BUILD.md)); point `ANOFOX_EXT` at an
`anofox_visualization.duckdb_extension` you already have to skip that. It also
generates the data on the first start.

Options: `--port N` (default 8910), `--cache-mb N` (default 64, `0` = off),
`--refresh-s N` (rebuild the snapshot every N seconds; default 60, `0` = only
on demand), `--auth` (see below).

## What to try

1. **Click around.** Switch the region, sort and filter the sales table, page
   through it quickly or switch it to *Scroll*. The server log prints a line for
   every query it runs (`ran how-did-our-bakeries-sell#17 in 3 ms`). A panel you
   have seen before prints nothing: it came from the cache.
2. **Load next week's sales.** In a second terminal:
   ```sh
   examples/served/serve.sh week
   ```
   The serving session reloads its tables and calls `anofox_serve_refresh`;
   `serve.sh status` shows `epoch` going up. Reload the page: the KPIs, the
   charts and the *Data as of* chip in the header show the new week. Pages that
   were open keep working while the snapshot is swapped.
3. **Write a new forecast run.**
   ```sh
   examples/served/serve.sh run trend      # or seasonal-naive, moving-average
   ```
   The run is a new folder in `work/data/runs/`. The server reads that folder in
   place, so the printed link (`/d/what-will-each-store-sell?v.run=<id>`) shows
   it at once. The *Forecast run* list is a cached panel, so the new run joins
   the list with the next refresh (`serve.sh refresh`, or within `--refresh-s`).
4. **Try to leave the data folder.** `?v.run=../incoming` or any value with a
   `/` is refused with `400` before any SQL runs; DuckDB itself would refuse a
   path outside the folder too.
5. **Log in as two users.** See below.

`serve.sh check` runs `dashboard --check --strict` on every dashboard, and
`node examples/served/smoke.mjs` runs everything above against a fresh copy of
the data and stops the server again (`--browser` also renders both dashboards
in Chromium when Playwright is on `NODE_PATH`).

## Log in through an auth proxy

> **TODO: waiting for #78.** Login (`options.auth`) is in pull request #78,
> which has not been merged yet. Until it is, `serve.sh start --auth` stops
> with `options: unknown key 'auth'`. Nothing in this example falls back to
> serving without a login. The steps below were run against the #78 branch.

```sh
examples/served/serve.sh start --auth
```

Open <http://127.0.0.1:8911/> and pick a person. This starts:

- `auth-proxy.mjs` on port 8911, a small Node proxy with no dependencies. It
  stands in for oauth2-proxy, Caddy `forward_auth` or Cloudflare Access: it
  signs the viewer in and keeps a signed session cookie. On every request it
  removes the `X-Forwarded-User`, `X-Forwarded-Groups` and secret headers a
  client sent, and sets its own, plus the shared secret.
- The server with `auth` set: it believes those headers only when the request
  carries the secret, and only from `127.0.0.1`. It sets `__claim_user` and
  `__claim_groups` on every request; a viewer can't set them.
- A third dashboard, **How are my stores doing?**, which filters on the
  claims: Alice sees the North, Bob the Midlands and the South, and Carol
  (group `admins`) every store. Log in as Alice in one browser and as Bob in a
  private window, and compare.

The secret is generated at every start and lives only in the environment of
the two processes. The request log (`work/run/requests.jsonl`) has one line
per request with the user, and never the secret. Try a request that skips the
proxy:

```sh
curl -s -H 'X-Forwarded-User: carol' -H 'X-Forwarded-Groups: admins' \
  -d '{"dashboard":"how-are-my-stores-doing","panel":0,"vars":{}}' \
  http://127.0.0.1:8910/api/panel        # 401
```

`node examples/served/smoke.mjs --auth` checks all of this.

## How it fits together

```
work/incoming/*.parquet ──(serve.sh: CREATE TABLE in the session)──▶ snapshot ──┐
  stores, products, sales by week          refresh = new snapshot             │
                                                                              ▼
work/data/runs/<run>/*.parquet ─────(read in place, options.data)──▶ anofox_serve_dashboards :8910
  a forecast run per folder                                                   ▲
                                                     auth-proxy.mjs :8911 ────┘ (with --auth)
```

`serve.sh start` runs a DuckDB CLI session that loads the extension, creates
the tables and starts the server. The session then reads SQL from a FIFO
(`work/run/ctl`, readable only by you), which is how `serve.sh week`,
`refresh` and `stop` talk to it. A refresh is a SQL call in that session, not
an HTTP endpoint, so no viewer can trigger one.

| File | |
|---|---|
| `serve.sh` | start, stop, refresh, add a week or a run, check |
| `generate.sh` | the demo data: stores, products, daily sales per week, forecast runs |
| `dashboards/*.sql` | the two dashboards |
| `auth/how-are-my-stores-doing.sql` | the per-user dashboard (served only with `--auth`) |
| `auth-proxy.mjs` | the demo login proxy |
| `smoke.mjs` | the end-to-end check |

## Secure by default, and what to change for real use

- Everything listens on `127.0.0.1`.
- No secret is stored in a file or committed. The proxy secret is generated at
  start; the proxy's cookie key is random per process.
- Panels on the data folder can only read: every statement must be a query or
  a `TEMP` view or table (#87), and DuckDB may open no path outside the folder.
- The demo login has no passwords. In production, use a real identity
  provider behind oauth2-proxy, Caddy or Cloudflare Access (recipes in
  [secure-serving.md](../../docs/secure-serving.md)), with TLS. Mount the data
  folder read-only for the server's OS user.
