---
name: anofox-visualization-access-control
description: >
  Who may see which dashboard rows and columns on an anofox-visualization
  locked server: login (options.auth / serve --auth) through an auth proxy
  (oauth2-proxy, Caddy forward_auth, nginx auth_request, Cloudflare Access,
  shared secret, trusted_proxies), built-in JWT bearer tokens, OIDC login
  (PKCE, sessions, logout, revocation_file), signed embed links for iframes
  (Metabase / Superset-style guest embedding), hashed API keys (serve api-key
  new / revoke / list), the reserved claim variables (__claim_user,
  __claim_groups, __claim_<name>), -- @vars, row and column access policies
  (policy.json: row filters, deny / null / mask / hash columns,
  table_functions, functions, policy_key_env), what a policy refuses,
  `dashboard --check --policy` (policy/* lints), the request log and the
  audit log. Use when the user asks for "login / SSO / OIDC / Keycloak /
  Entra / Auth0", "row-level security", "each customer / tenant / region
  sees only its rows", "hide or mask a column", "embed a dashboard in our
  app", "API key for a script", "audit who looked at what", or gets a 401 /
  a refused start from auth or policy. Serving modes, options and deployment
  are in the serving skill.
version: 2026.10.18
user-invocable: false
---

# Anofox Visualization — Login and Row / Column Policies

**Applies to:** the locked server `anofox_serve_dashboards(dir, port, options)` (options `auth`, `policy`, `log`, `audit`); `serve --dashboards` takes `--auth` / `--log` / `--audit` but has **no policies** | **Docs:** `docs/secure-serving.md` (reference + threat models), `docs/deployment.md`

DuckDB has no users, `GRANT` or row-level security, so the server enforces it: a login method yields **claims**, the panels (or a policy) filter on them.

## What never enforces access

A static HTML export, the browser builder / DuckDB-Wasm demo (the data is in the client) and a MotherDuck token (the whole database). Only the locked server (A) and the static render server (B) keep data on the server; only A has policies.

## Login methods (`auth`: one object or a list, each `mode` once)

```sql skip
SELECT anofox_serve_dashboards('dashboards', 8095, '{
  "hosts": ["dash.example.com"],
  "auth": [
    {"mode": "oidc", "issuer": "https://login.example.com", "client_id": "dashboards",
     "client_secret_env": "OIDC_CLIENT_SECRET", "redirect_url": "https://dash.example.com/auth/callback",
     "groups_claim": "groups", "claims": {"tenant": "tid"}, "session_secret_env": "ANOFOX_SESSION_SECRET"},
    {"mode": "jwt", "issuer": "https://login.example.com", "audience": "dashboards-api"},
    {"mode": "embed", "secret_env": "ANOFOX_EMBED_SECRET", "claims": ["tenant"]},
    {"mode": "api_keys", "file": "/etc/anofox/keys.json"}
  ],
  "policy": "/etc/anofox/policy.json",
  "log": "/var/log/anofox/requests.jsonl", "audit": "/var/log/anofox/audit.jsonl"
}');
```

| `mode` | For | Key settings |
|---|---|---|
| `proxy` | an auth proxy that already logs people in | `shared_secret_env` (≥ 32 chars, the proxy adds it on every request), `trusted_proxies` (CIDRs), `header_user` (`X-Forwarded-User`), opt-in `header_groups`, `claims: {"tenant": {"header": "X-Forwarded-Tenant", "required": true}}` |
| `jwt` | bearer tokens from an IdP | `issuer` (https), `audience`, optional `jwks_url`, `algorithms` (asymmetric only), `user_claim` (`sub`), `groups_claim`, `claims` (dotted paths), `cookie`, `ca_file` |
| `oidc` | browser login, authorization code + PKCE | the `jwt` keys + `client_id`, `client_secret_env`, `redirect_url` (`https://…/auth/callback`), `scopes`, `session_ttl_s` (8 h), `session_secret_env` (survives restarts, shared by replicas), `revocation_file` (durable, shared logouts) |
| `embed` | a host app embeds `/d/<id>?embed=<token>` in an iframe | HS256 link `{"dashboard", "user", "groups", "claims", "vars", "exp", "jti"}` signed with `secret_env`; `max_link_ttl_s` (300), `session_ttl_s`, `one_time`; the link's `vars` are **locked** (URL / panel requests can't change them), the session reads only that dashboard |
| `api_keys` | scripts, server-to-server | `Authorization: Bearer afx_<id>_<secret>`; the file holds SHA-256 hashes, is re-read on change (revocation without restart); `serve api-key new --file keys.json --user svc-etl --groups etl --claim tenant=acme --dashboards sales --expires-days 90` prints the key once; `serve api-key revoke --file keys.json --id <id>`, `serve api-key list --file keys.json` |

Rules the server enforces: exactly one credential per request (an invalid one is `401`, never skipped); a broken `auth` (unknown key, short or reused secret, `http://` issuer, `HS*` / `none` algorithm) **fails the start** — no fallback to anonymous. Prefer `*_env` (an environment variable name) over a literal secret in SQL. An e-mail identity needs `email_verified: true`. Cookies are `Secure`: serve over HTTPS (TLS at the reverse proxy). `GET /healthz` without a login answers `{"status":"ok"}` only.

Proxy mode: the proxy must **set or remove every configured identity header on every request** (never pass the client's through) and be the only way to the server; tested nginx / Caddy / oauth2-proxy recipes are in `docs/secure-serving.md` → *Proxy recipes*.

## Claims in panel SQL

Every method sets, after the viewer's own variables:

| Variable | Type |
|---|---|
| `getvariable('__claim_user')` | `VARCHAR` |
| `getvariable('__claim_groups')` | `VARCHAR[]` (`[]` if none) |
| `getvariable('__claim_<name>')` | `VARCHAR` (`NULL` if absent) |

```sql skip
-- rows of the viewer's regions; the 'admins' group sees all
SELECT region::XAXIS, sum(revenue)::BARCHART, 'Revenue in your regions'::TITLE FROM orders
WHERE region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user'))
   OR list_contains(getvariable('__claim_groups'), 'admins')
GROUP BY ALL ORDER BY 2 DESC;
```

- **Filter only on `__claim_*`**, never on a viewer variable (inputs, `?v.*`, `selected` can hold any value). Viewers can never set `__claim_*` or `anofox_data` (`400` / ignored).
- `-- @vars region, channel, selected` in a dashboard's header limits the variables a viewer may set (others are dropped). List its inputs, named selections and `selected`.
- Without `auth` the claims are `NULL`, so claim-filtered panels return no rows (not every row).
- The result cache, single-flight and cancel tickets are keyed per viewer's claims.

## Row and column policies (`policy`)

With `auth` the server **refuses to start without a policy** (`options.policy`, or `policy.json` in the dashboard folder) unless `"policy": "none"` says the panels filter themselves. A policy moves the rule out of the panels: every table reference is rewritten to a per-viewer view.

```json
{
  "tables": {
    "sales": {"rows": "region IN (SELECT region FROM entitlements WHERE viewer = getvariable('__claim_user')) OR list_contains(getvariable('__claim_groups'), 'admins')"},
    "employees": {
      "rows": "getvariable('__claim_tenant') = tenant",
      "columns": {
        "salary": {"allow_groups": ["hr"], "else": "null"},
        "email":  {"allow_groups": ["hr", "support"], "else": "mask"},
        "name":   {"allow_groups": ["hr"], "else": "hash"},
        "ssn":    "deny"
      }
    },
    "ref.regions": {"rows": true}
  },
  "table_functions": ["read_parquet"]
}
```

| Key | Meaning |
|---|---|
| `tables.<t>.rows` | required: one SQL condition over the table's columns, reading the viewer only via `getvariable('__claim_…')`; `true` = every row |
| `tables.<t>.columns` | `deny` (column gone), `null`, `mask` (`'****'`), `hash` (HMAC, joins / `count(DISTINCT)` still work); or `{"allow_groups": […], "allow_users": […], "else": …}` |
| `table_functions` | file readers dashboards may call (off by default; files have no row policy) |
| `functions` | extra reviewed scalar / aggregate functions (an extension's) |

Server option `"policy_key_env": "ANOFOX_POLICY_KEY"` keeps `hash` values stable across restarts. **Keep the policy file outside the dashboard folder** when dashboard authors must not decide who sees what.

**Fail closed** — a dashboard statement that would get around the views stops the start: a table without a rule (including `information_schema`, a file path, a view over other tables), a database-qualified name, `query()` / `query_table()` / catalog table functions, a user macro, a function not on the reviewed allowlist (`current_setting`, `json_serialize_plan`, `getenv`, …), time travel, and any statement other than a query, `CREATE TEMP TABLE|VIEW … AS <query>` or `SET VARIABLE`. CTEs, subqueries, joins, set operations, `PIVOT` in `FROM` and the dashboard's own temp tables work. Under a policy viewers see a generic panel error; the detail goes to the server log.

**Lint before serving** (uses the server's classifier):

```sh
dashboard --check dash.sql --db snapshot.duckdb --policy policy.json
```

`policy/refused` (the server would not start), `policy/denied-column` (some viewers can't see a column the panel uses; an error when nobody can), `policy/masked-column` (note), `policy/invalid`, `policy/duckdb-version` (the `duckdb` on PATH is not the pinned 1.5.6).

## Logs

- `"log"` (`serve --log`): JSON lines per page view, panel request and refusal — `user`, `status`, `ms`, `cache` (`hit` / `shared` / `run`), `deny` reason; `log_vars`: `names` (default), `values` (may be personal data — opt in), `none`.
- `"audit"` (`serve --audit`): logins, logouts, embed exchanges, refused credentials with their reason, API key changes. Tokens, keys, cookies and secrets are never logged. Both files are `0600`, rotate by rename.

## Gotchas

1. **Policies are locked-server only.** `serve --dashboards --auth …` has none: every panel must filter on the claims itself.
2. **A proxy header the proxy doesn't overwrite is the client's.** Configure groups / claims only for headers your proxy sets or removes on every request.
3. **Embed links are bearer credentials until `exp`**: mint per page view, keep `max_link_ttl_s` at minutes; `one_time` is per server process.
4. **Without `revocation_file`, a logout is per replica** and forgotten on restart; keep `session_ttl_s` short.
5. **`hash` is pseudonymisation, not encryption.** Column rules decide by user and group only; other claims drive row filters.
