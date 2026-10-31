# Plan: the P3 gaps to Shaper (#769–#773, #775)

Status: **design note, waiting for owner decisions (§8). No code before they are taken.**
Follows the gap analysis of 2026-10-30. P1 and P2 are done (#763–#768).
Each item here adds something the locked server didn't have: a new write
path (#769, #770, #772, #775), a new credential (#771) or author-controlled
styling (#773). So each needs a decision before code and a security
self-review in its PR, like #767/#768
([plan-tasks-reports.md](plan-tasks-reports.md)).

The locked server's rules that every item keeps (docs/secure-serving.md):
- Viewers can't change the server, its SQL or its options.
- Dashboards are reviewed files.
- The snapshot is read-only.
- Secrets come from the environment.
- Every panel runs through the same policy path.

## 1. Ingest API (#769)

`POST /api/data/<table>` with a JSON object or an array of objects.

- **Credential:** an API key that has an `ingest` scope naming its tables
  (`"ingest": ["events"]`). API keys today carry claims and a dashboard list;
  scopes are new. No viewer login or embed link can ingest.
- **Where it writes (I1):** never into the snapshot. It writes to one
  ingest database (`options.ingest.database`, attached read-write by the
  operator, like `tasks.write`), table by table. The rows become visible at
  the next refresh, or right away with `options.ingest.refresh: true`
  (debounced, at most once every 30 s).
- **Schema:** the first batch creates the table, with types inferred by
  DuckDB (`read_json` over the request body) plus `_id` (UUID) and `_ts`
  (server time). A later batch with a new key adds a column (`ALTER TABLE …
  ADD COLUMN`); a type that conflicts is a `400`, never a silent cast.
  Optional: a fixed schema per table in the options, which turns new keys
  into a `400` too.
- **Limits:** 1 MiB per request (`max_body_bytes`), 10 000 rows per request,
  and a per-key rate (requests/minute). Table and column names must be
  plain identifiers; the JSON never becomes SQL text (bound parameters / a
  temp file read by `read_json`).
- **Runs on:** the replica with `tasks.run` (one writer, D3 of the tasks
  plan). Other replicas answer `503`; operators route `/api/data` to the
  writer.

## 2. Temporary dashboards (#770)

`POST /api/dashboards` with a dashboard's SQL returns `{id, url, expires}`.

- **Credential:** an API key with a `preview` scope. The dashboard runs
  under that key's claims and dashboard scope, so the access policy applies
  as for any viewer.
- **Checks before it is stored:** the same parse and statement checks as a
  file dashboard at start (one query or an allowed setup form per
  statement; with a data folder, the path rules), then `--check --strict`.
  A failed check is a `400` with the lint messages.
- **Storage (P1):** in memory, plus a file per preview in `options.preview.dir`
  so replicas behind one volume and restarts keep them. 24 h by default
  (`ttl_s`, at most 7 days), at most N live previews per key (default 20),
  and 256 KiB of SQL.
- **The URL:** `/p/<random id>` (128 bits). It needs a login like
  `/d/<id>`; `"share": true` in the request gives it a share link (#763)
  instead. Previews never appear in the dashboard list.
- **What it can't do:** no `::SAVE` (write-back), no tasks, no reports.

## 3. Embed SDK and token endpoint (#771)

- **Token endpoint (E1):** `POST /api/auth/token` with an API key that has
  an `embed` scope returns a short embed token (the same HS256 JWT format as
  today's signed embed links, `exp` ≤ `max_link_ttl_s`). Its `dashboard`,
  claims and locked vars come from the request, capped by the key: a key
  with `claims: {region: "EU"}` can't mint a token for `region: "US"`. The
  host's backend calls it, so the host never holds the signing secret. A
  browser must never hold the API key.
- **Web component (E2):** `<anofox-dashboard src="https://dash…/d/sales"
  token-callback="getToken">`, a small module served by the server at
  `/sdk/anofox-embed.js`. The issue asks for no iframe. Recommendation:
  **keep an iframe inside the component**:
  - **Without an iframe:** the dashboard's scripts would run in the host's
    origin, and the host's CSP would have to allow our wasm, DuckDB-Wasm and
    inline-nonce scripts. A host page script could also read every panel's
    data.
  - **With an iframe:** the CSP and data stay ours, and the component handles
    sizing (`postMessage` height), token refresh (`getToken()` before
    expiry), and events.
- **Events:** a selection, a link click, a variable change, posted to the
  parent with a fixed origin (`frame_ancestors` from the options) and a
  schema. The host can set variables through `postMessage` only where the
  token's locked vars allow.
- **React:** a 30-line wrapper around the element, published with it.

## 4. Deploy workflow (#772)

- **`dashboard deploy <dir> --server https://… [--validate-only]`:** builds a
  bundle (the `.sql` files, `assets/`, `tasks/`) and `PUT`s it to
  `/api/admin/bundle` with an API key that has an `admin` scope. The server:
  1. checks it exactly like a start (parse, statements, `--check --strict`,
     task files, report names);
  2. on `--validate-only`, answers with the result;
  3. otherwise writes it to a new directory next to the old one and swaps a
     symlink (atomic), then reloads the dashboards and rebuilds the snapshot
     like `anofox_serve_refresh`.

  A failed check leaves the running set untouched.
- **Stable ids:** `-- @id sales-weekly` in a file overrides the id derived
  from its path. Folders become sections in the list (`-- @group` already
  exists, so a folder only sets a default group).
- **GitHub Action:** `DataZooDE/anofox-visualization-action` running
  `dashboard --check --strict` on PRs and `dashboard deploy` on merge, with
  the key in a repository secret.
- **Alternative (D2-alt):** no upload endpoint at all. The Action checks,
  then the operator's own deploy (rsync, a container image) copies files, and
  a SQL `anofox_serve_reload(port)` (from the session) reloads. That avoids
  an admin write route on the server; the trade-off is less convenience.

## 5. White-labelling (#773)

- **Operator stylesheet (W1):** `options.css_file` (a file next to the server
  options, not in the dashboards folder) is served at `/static/site.css` and
  linked after the theme. It is an operator file, so it is trusted. It must
  still respect the CSP: no remote `url()` (`img-src`/`font-src` stay
  `'self' data:`), which the server checks at start by refusing `url(` with
  a scheme other than `data:` or a relative path, and `@import`.
- **Favicon:** the builder's app-bar options already take a `favicon`;
  the locked server gets the same option.
- **Per-dashboard `::CSS` / `::CLASS` (W2):** dashboard authors are less
  trusted than the operator. Recommendation:
  - **`::CLASS` only:** a panel gets one of the operator stylesheet's classes
    (`'kpi-hero'::CLASS`, names `[a-z0-9-]`). The look stays in the
    operator's CSS.
  - **`::CSS` not built:** dashboard CSS can spoof the page (a fake login
    box), hide policy notices, or exfiltrate through attribute selectors and
    `url()`. If wanted later, only behind `options.dashboard_css: true`, with
    the CSS parsed and limited to the panel scope.

## 6. State durability (#775)

The per-viewer state store (#517) and the write-back log (#684) are JSON
Lines files on the node.

- **Snapshots (S1):** every `snapshot_every_s` (default 300) and at stop, the
  server copies each file (a consistent copy under its own lock) to
  `options.durability.s3: {bucket, prefix, region, endpoint?}` through
  DuckDB's `httpfs` (`COPY … TO 's3://…'`), or a plain HTTPS `PUT` with SigV4.
  Recommendation: **DuckDB httpfs**, since the operator already configures
  S3 secrets there for data, and it is no new dependency.
- **Restore:** at start, when the local file is missing (a new node), the
  server fetches the newest snapshot. A local file always wins, so restore
  never overwrites newer local data.
- **Several replicas taking writes (S2):** one writer only. Viewer state
  and `::SAVE` are routed to the replica with `tasks.run`, and the others
  answer `503` for writes. That is documented and checked, with no lease or
  consensus. A lease (S3 conditional writes) is possible later if one writer
  isn't enough.
- Snapshots contain personal data (who answered what): bucket encryption
  and retention are the operator's, which the docs say.

## 7. Order and size

| Item | Size | Security review | Depends on |
|---|---|---|---|
| #773 W1 (`css_file`, favicon, `::CLASS`) | S | yes (CSS) | — |
| #772 deploy (validate-only + Action first, upload second) | M | yes (admin route) | API-key scopes |
| #769 ingest | M | yes (write path) | API-key scopes, one writer |
| #770 previews | M | yes | API-key scopes |
| #771 token endpoint + element | M | yes (tokens, postMessage) | API-key scopes |
| #775 durability | M | yes (personal data off-node) | one writer |

API-key **scopes** (`ingest`, `preview`, `embed`, `admin`) are shared by
four items. They come first, as a small PR of their own: keys without
scopes keep exactly today's rights (read dashboards).

## 8. Decisions for the owner

- **D1 — API-key scopes** (§7): add `scopes` to API keys (`ingest`,
  `preview`, `embed`, `admin`), with no scope meaning read-only as today.
  *Recommended.*
- **D2 — Deploy:**
  - (a) an upload endpoint `/api/admin/bundle` with validate-only and an
    atomic swap; or
  - (b) no upload route: the Action checks, the operator's deploy copies,
    and `anofox_serve_reload(port)` reloads.

  *Recommended: (b) first* (no new admin write route; most operators deploy
  with images or rsync anyway), then (a) only if asked.
- **D3 — Ingest target:** one operator-attached ingest database, written
  only by the `tasks.run` replica; rows visible after a (debounced) refresh.
  *Recommended.*
- **D4 — Ingest schema:** inferred and widened (new keys add columns;
  conflicts are a `400`), with an optional fixed schema per table.
  *Recommended.*
- **D5 — Previews:** stored as files with a TTL, behind a `preview` key, run
  under the key's claims, `--check --strict` first, no write-back, tasks or
  reports. *Recommended.*
- **D6 — Embed:** keep an iframe inside `<anofox-dashboard>` (our CSP and
  data stay in our origin); token minting is capped by the key's own claims.
  *Recommended* over a true iframe-free render.
- **D7 — White-labelling:** an operator `css_file` (checked for remote
  `url()` and `@import`), a favicon, and `::CLASS` naming the operator's
  classes. No per-dashboard `::CSS` for now. *Recommended.*
- **D8 — Durability:** S3 snapshots through DuckDB `httpfs`, restored only
  into an empty node, and one writer for viewer state and `::SAVE` (no
  lease). *Recommended.*
- **D9 — Order:** D1 scopes → #773 → #772 (b) → #769 → #770 → #771 →
  #775. *Recommended.*
