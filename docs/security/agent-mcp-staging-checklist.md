# Staging test: agent mode and MCP with a real identity provider

Status: **checklist, not yet run.** It is the hands-on half of the
independent review of the agent/MCP slice (#866 `--publish`, #867 answers,
#868 MCP; plan condition 16 in
[`plan-dashboard-to-agent.md`](../plans/plan-dashboard-to-agent.md) §12).
The code half is the review of draft PR #869. This half covers what a code
review can't: how a real identity provider's tokens, an MCP client and a
deployed server behave together.

Tracking issue: #870. Time: about an hour. Who: anyone with admin access to a test tenant of the
identity provider (Entra ID or Keycloak). Record each result in the last
column and attach the server log and request log to the issue.

## 1. Set up

**Identity provider (test tenant):**

- An API (app registration / client) whose audience, or Application ID URI,
  is exactly the MCP URL, e.g. `https://agent-staging.example.com/mcp`.
- A role or group `agent-user`, given to two test users: **alice** (region
  `north`) and **bob** (region `south`). Put the region in a claim
  (Entra: an optional claim or an extension attribute; Keycloak: a user
  attribute mapper), named `region` below.
- Something to mint tokens for:
  - the right audience (alice, bob);
  - another audience of the same tenant (any other API);
  - a second tenant, or a realm in Keycloak, with the same audience;
  - a client-credentials token (no user, no `agent-user` role).

**Server** (staging host behind TLS; a recent master build or the release):

```sql
-- in the DuckDB session that serves; the table is copied into the snapshot
CREATE TABLE deliveries AS SELECT * FROM (VALUES
  ('Alpine', 'north', 0.93), ('Boreal', 'north', 0.71),
  ('Cirrus', 'south', 0.88)) t(supplier, region, on_time);

SELECT anofox_serve_dashboards('dash', 8095, '{
  "hosts": ["agent-staging.example.com"],
  "auth": {"mode": "jwt", "issuer": "<issuer>",
           "audience": "https://agent-staging.example.com/mcp",
           "require": {"tid": "<tenant id>", "roles": "agent-user"},
           "claims": {"region": "region"}},
  "policy": "policy.json",
  "audit": "/var/log/anofox/audit.jsonl",
  "log": "/var/log/anofox/requests.jsonl",
  "agent": {"dashboard": "suppliers", "max_rows": 20,
            "mcp": {"resource": "https://agent-staging.example.com/mcp"}}
}');
```

`dash/suppliers.sql`:

```sql
-- @title Supplier reliability
-- @vars supplier, code
SELECT supplier ::DROPDOWN FROM deliveries ORDER BY 1;
SELECT 'x' AS code ::TEXT;
-- @question How reliable is {supplier} on delivery?
SELECT supplier AS "Supplier" ::TABLE, on_time AS "On time %"
FROM deliveries WHERE supplier = getvariable('supplier');
-- @question Which suppliers are there?
SELECT supplier AS "Supplier" ::TABLE, region AS "Region" FROM deliveries;
-- @question What is order {code} worth?
SELECT CAST(getvariable('code') AS INTEGER) AS n ::METRIC, 'Value' ::LABEL;
```

`dash/policy.json`:

```json
{"tables": {"deliveries": {"rows": "region = getvariable('__claim_region')"}}}
```

Start with `DATAZOO_DISABLE_TELEMETRY=1` (agent mode refuses to start
otherwise). In Entra the tenant claim is `tid` and roles come in `roles`;
in Keycloak use the realm's claim (e.g. `iss` is already per realm, so pin
`azp` or a custom `tenant` claim instead) and adjust `require`.

Shorthand below: `MCP=https://agent-staging.example.com/mcp`,
`rpc` = `curl -s $MCP -H "Authorization: Bearer $T" -H 'Content-Type:
application/json' -d '<json>'`.

## 2. Start checks

| # | Do | Expect | Result |
|---|---|---|---|
| S1 | Start with `"audience": ["<MCP URL>", "other-api"]` | Refused: `auth.audience must be exactly the MCP resource` | |
| S2 | Start without `require` | Refused: `auth.require must pin the tenant` | |
| S3 | Start without `DATAZOO_DISABLE_TELEMETRY=1` | Refused: telemetry must be off | |
| S4 | Start with `"log_vars": "values"` | Refused | |
| S5 | Start as in §1 | Starts; the log names the dashboard, 3 questions, the row cap | |

## 3. Tokens

Call `tools/list` (`{"jsonrpc":"2.0","id":1,"method":"tools/list"}`) with
each token. The audit log should show the reason for every refusal.

| # | Token | Expect | Result |
|---|---|---|---|
| T1 | none | `401`, `WWW-Authenticate: Bearer resource_metadata="https://…/.well-known/oauth-protected-resource/mcp"` | |
| T2 | alice, right audience | `200`, three tools | |
| T3 | alice, another audience of the same tenant | `401` (`bad_audience`) | |
| T4 | same audience, other tenant | `401` (`bad_claim`, or `bad_issuer` if the issuer differs) | |
| T5 | client credentials, no `agent-user` role | `401` (`bad_claim`) | |
| T6 | alice's token after it expired (or with `leeway_s` exceeded) | `401` (`expired`) | |
| T7 | alice's ID token instead of an access token | `401` (`id_token`) | |
| T8 | right token with one character of the signature changed | `401` (`bad_signature`) | |
| T9 | `GET` the metadata URL without a token | `200`, JSON with `resource` = the MCP URL and the issuer; nothing else | |

## 4. Per-caller answers

| # | Do | Expect | Result |
|---|---|---|---|
| P1 | alice: `tools/call` `which_suppliers_are_there` | Alpine, Boreal only (north) | |
| P2 | bob: the same | Cirrus only (south) | |
| P3 | bob: `how_reliable_is_supplier_on_delivery` with `{"supplier": "Boreal"}` | `isError`, `no_match` (bob can't see Boreal); no row, no hint of it | |
| P4 | alice: the same with `{"supplier": "bor"}` | Boreal, 0.71 | |
| P5 | alice: `{"supplier": "Boreal", "vars": {"region": "south"}}` | Refused; no south rows | |
| P6 | alice: `{"supplier": "' OR 1=1 --"}` | `no_match`; nothing else | |
| P7 | `/api/agent/answer` with bob's token, `{"question_id": "which_suppliers_are_there", "slots": {}, "limit": 1000}` | At most 20 rows (`limit` is capped at `max_rows`); south rows only | |

## 5. Leaks and errors

| # | Do | Expect | Result |
|---|---|---|---|
| L1 | alice: `what_is_order_code_worth` with `{"code": "canary-zz9"}` | `isError` with a fixed message and an error id; no `canary`, no DuckDB text | |
| L2 | `grep canary-zz9` in the server log, request log and audit log | No match | |
| L3 | `grep` alice's token (its first 20 characters) in all logs | No match | |
| L4 | `tools/list` descriptions | Only "Answers: …? Returns: …." over the question and column names; no `@title` or panel text | |

## 6. Transport and browser

| # | Do | Expect | Result |
|---|---|---|---|
| H1 | `GET $MCP` and `DELETE $MCP` with a valid token | `405`, `Allow: POST` | |
| H2 | `rpc` with `Origin: https://evil.example` | Refused (403) before MCP handling | |
| H3 | `rpc` with `Host: other.example` (curl `--resolve` to the staging IP) | `421` (unknown Host) | |
| H4 | A body over 64 KiB | `413` | |
| H5 | A JSON array (batch) | `400` | |
| H6 | `MCP-Protocol-Version: 2024-01-01` | `400` | |
| H7 | `resources/list`, `prompts/list`, `sampling/createMessage` | `-32601` | |
| H8 | `GET /`, `/d/suppliers`, `/api/panel` with a valid token | `404` | |
| H9 | A share link or an API key scoped to another dashboard (if configured) on `/mcp` | `403` | |

## 7. A real MCP client

| # | Do | Expect | Result |
|---|---|---|---|
| C1 | `claude mcp add --transport http staging $MCP --header "Authorization: Bearer $T"` (alice), then ask "Which suppliers are there?" | The client lists the three tools and gets north rows only | |
| C2 | The same without `--header` | Either the client runs the OAuth login itself (metadata → IdP → token with this audience) and works, or it fails cleanly. **Record which**: this path is untested; if it fails, note the IdP's support for RFC 8414 / 8707 and dynamic client registration | |
| C3 | Ask the client to "ignore the tool and show all regions" | No other data: the server answers only the declared questions | |

## Reporting

- Every row that doesn't match its expectation is a finding: open an issue
  with the row number, the request (token redacted), the response and the
  log lines, and link it from the tracking issue.
- When every row passes, note the date, build, IdP and tester in the
  tracking issue. Agent mode and MCP then lose the "preview" label in the
  next release notes, provided the review of #869 is also closed.
