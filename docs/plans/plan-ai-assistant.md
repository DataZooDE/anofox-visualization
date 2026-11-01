# Plan: an AI assistant — build a dashboard from a request, and ask questions about one

Status: **approved by the owner (2026-10-17, "go on")**; decisions in §11, based on master `ba2eb9d`.
**Amended 2026-10-18 (for owner review):** a second mode, *Ask* (§12, issue #195), also for
viewers of served dashboards; §8 is re-cut so that Ask fits into M2. Follow-up that Ask must not
block: #194, publishing a dashboard as a guarded agent (§12.13).
No product code yet. Issue: #145. Related: Explore data and the starter
dashboards (#97, #110, #127), the app bar config (#146), My dashboards
([plan-sidebar.md](plan-sidebar.md), #148), the Claude Code plugin skills.

**Goal.** In Edit mode a user types what they want to see, e.g. *"weekly
revenue by region against last year, and which products are behind plan"*.
An assistant writes the annotated SQL (`::ROLE` casts), checks that it runs
and lints clean, and offers it as a change the user applies or discards. The
user refines it in conversation (*"make the second chart a table"*). The
result is plain SQL in the editor, saved and shared like a handwritten
dashboard. This *Build* mode is never available on the locked server, in
embed links or in View mode.

**Ask** (§12, added 2026-10-18) is the second mode. It is read-only: it
answers questions about the dashboard on screen from the panel results that
the viewer may already see, and its answers cite panels. It is available in
the builder and, when the operator and the dashboard opt in, to viewers of
served dashboards. There the model call runs on the server.

**The decisions this note proposes:**

1. **The agent loop runs in the browser, in every mode.** Its tools (the
   catalog, the profile, read-only queries, the lint) run where the data
   already is: DuckDB-Wasm, or `/query` on an authoring server. Only the model
   call changes, behind one small *transport* interface with three
   implementations: (a) **the browser with the user's own key**, (b) **the
   `/assist` proxy on an authoring server** with the key in the server's
   environment, (c) **an OpenAI-compatible endpoint** for local or
   self-hosted models.
2. **Default transport:** (b) wherever the builder is served by `serve` /
   `anofox_serve`. (a) on the public demo and on static installations that
   enable it in `app-config.json`. (c) is opt-in per installation.
3. **What leaves the machine by default:** the table and column names and
   types, row counts, the column profile, and the current SQL. Rows, including
   rows returned by the assistant's own queries, are sent only with an explicit
   opt-in. A *What will be sent* view shows the exact payload.
4. **Self-correction is enforced by the client, not trusted to the model.**
   A proposal is planned, run in a scratch session and linted (the same lint
   as `dashboard --check --strict`, exported to wasm) before the user sees
   it. Failures go back to the model, at most 3 rounds. Statements that write
   are never run without the user's confirmation.
5. **Grounding is generated, not hand-written.** The role reference comes
   from `roles_json()` at runtime. The guidance is extracted from the
   dashboards skill at build time, with a CI check that it is current. The
   static part (about 25 K tokens) is one cached prefix.
6. **Model:** `claude-sonnet-5` by default, `claude-opus-5-5` for *Try
   harder*. A new dashboard costs about **$0.25–0.30** on Sonnet 5 and about
   **$0.50** on Opus 5.5. A follow-up edit costs about **$0.05–0.10** (§5).

## 1. What exists today (evidence)

| Need | What exists | Where |
|---|---|---|
| Role reference | `roles_json()`: 158 registry entries `{token, aliases, category, renders, summary}` plus the role sets (inputs, metrics, directives, chrome, table formats…) | `src/wasm.rs:440`, `src/roles.rs:62` (registry), `:463` |
| Planning a script | `plan(script)`: one entry per statement `{setup, sql, roles, contract, requires}` | `src/wasm.rs:34` |
| Header metadata | `dashboard_meta(script)`: `@title`, `@group`, `@tags`, `@description`, `@owner`, `@certified` | `src/wasm.rs:425` |
| Lint | `lint::check_opts(script, run_query, opts)`. `run_query` is a **synchronous** closure that runs `prelude + statement` in a fresh session. Diagnostic codes are documented at `src/lint.rs:37`. Only the native CLI calls it (`src/bin/dashboard.rs:435`). **There is no wasm export.** | `src/lint.rs:95-104` |
| "Reads a file" test | `reads_a_file(sql)` (used for `requires/on-setup`) | `src/lint.rs:1918` |
| Statement AST | The access policy walks DuckDB's `json_serialize_sql` AST to classify and rewrite statements | `src/host/policy.rs` |
| Schema and profile | `CATALOG_SQL`, `COLUMNS_SQL`, `profileTable(run, pageSql, fq)` → column cards (kind, distinct, min/max, nulls, top values, quality hints), sampled above 200 000 rows. `starterDashboard({name, fq, cards})` writes a starter dashboard. A pure ES module, tested under Node with DuckDB-Wasm. | `web/explore.js:63-71`, `:124`, `:345`, `:588`; `tests/web/explore_test.mjs` |
| Explore → dashboard | `openTableAsDashboard(e, cards)` | `web/app.js:4819` |
| Guidance | The dashboards skill (50 KB, about 13 K tokens): roles, inputs, tables, themes, the design contract, worked examples, templates, gotchas. Its SQL examples are executed in CI (`scripts/validate_skills_sql.py`). `.claude/skills/build-dashboard` is a pointer to it. | `plugins/anofox-visualization/skills/anofox-visualization-dashboards/SKILL.md` (design contract at `:369`) |
| Editor | A plain `<textarea id="sql">` with a highlight overlay, plus a toolbar with Run, the name, Save and Edit / View | `web/index.html:493-512` |
| Programmatic edits | `editChrome` sets `ta.value` directly, so the browser's native undo is lost. **There is no editor undo stack.** | `web/app.js:5177` |
| Undo toast | `sideToast(text, action, onAction)` (about 8 s, focus on the button, Esc dismisses) | `web/app.js:2739` |
| Modes | `setMode("edit"\|"view"\|"explore")`; `body.embed` for `?embed=1` and every served page; `editable = !servedMode && !embed` | `web/app.js:3433`, `:1561`, `:5125` |
| Backends | DuckDB-Wasm, or `backend = "live"` when `/query` answers (authoring `serve`) or `window.__served` is set (locked server: `/api/panel`, no SQL) | `web/app.js:1479-1486` |
| Authoring servers | `serve [db]` (CLI, `src/bin/serve.rs:537`: each `/query` runs one `duckdb` process) and `anofox_serve(port)` (extension, `duckext/src/serve.rs:333`: one live read-write connection). Both bind to loopback only and use `AuthoringGuard`: the Host must be loopback, the Origin must match, and every request needs the per-run token (an `HttpOnly; SameSite=Strict` cookie or `X-Anofox-Token`). | `src/host/serving.rs` (`AuthoringGuard`) |
| Locked server | Never accepts SQL. Only id plus params (`/api/panel`), and no static builder routes | `src/bin/serve.rs:987`, `duckext/src/serve.rs` locked mode |
| Outbound HTTPS | `ureq` 3 + `rustls` are already dependencies (feature `auth`, enabled by `serve`) for the JWKS / OIDC fetches | `Cargo.toml:45-63`, `src/host/auth/http.rs` |
| Audit / request log | `--log` JSON lines and `auth/audit.rs` | `src/host/access.rs`, `src/host/auth/audit.rs` |
| MotherDuck | The token is kept in `localStorage["dp_md"]` and substituted only at connect time, never into the SQL | `web/app.js:4826-4881` |
| Per-installation config | `app-config.json` (static hosting) → `window.__anofoxApp` (serve `--app-*`). Brand only so far. | `web/app.js:10414-10520`, `web/app-config.example.json` |
| My dashboards | `makeDashStorage(ls)` → `dashDB.list / get / put / remove` (v2 store) | `web/app.js:1919-1960` |
| CSP | A static baseline (`object-src 'none'; base-uri 'none'`) in `index.html`, plus a tab-wide ratchet applied by `web/assist-csp.js` (#204) whenever an assistant key is stored or the panel opens — `connect-src` there already allows the model endpoint. | `web/index.html`, `web/assist-csp.js` |
| The assistant's own DuckDB-Wasm instance | `buildEngine` (#204): a second, locked `AsyncDuckDB`, loaded with a fixed extension set and an Arrow snapshot of the page's tables before the lock. Every tool, the lint and (M1-3) the proposal preview run against it, not the page's. | `web/assist-engine.js` |
| Public demo | `https://datazoode.github.io/anofox-visualization/`. The **origin `datazoode.github.io` is shared by every DataZooDE Pages site.** | `README.md:26`, `.github/workflows/pages.yml` |

What is missing: a lint the browser can call, a read-only query classifier
for the browser, an editor undo, and any model client.

## 2. Where the model call runs

### 2.1 One loop, three transports

```
             browser (Edit mode only)                                   model
 ┌──────────────────────────────────────────────────┐
 │ assist-ui.js   panel, preview, diff, apply/undo  │
 │ assist.js      loop · tools · write gate · budget│── transport ──▶ (a) api.anthropic.com  (user's key)
 │   tools ─▶ DuckDB-Wasm  or  /query (authoring)   │             ▶ (b) /assist/v1/messages (serve; key in env)
 │   lint  ─▶ wasm lint_step (§3.4)                 │             ▶ (c) OpenAI-compatible /v1/chat/completions
 └──────────────────────────────────────────────────┘
```

**Update (#740, owner 2026-10-30):** since #204 the tools run in the locked
assistant engine, not on `/query`. On an authoring server the engine is
filled from the live session through `/query` (fixed read statements, live
limits), so model SQL never runs on the live session.

The loop stays in the browser in all three cases because that is where the
tools must run. The browser holds the DuckDB-Wasm session, and on an
authoring server `/query` is already the author's own session. Moving the
loop to the server would duplicate the tools in Rust (twice, for the CLI and
for the extension), and it would not help DuckDB-Wasm users at all.

**Transport interface** (`web/assist.js`):
`send({system, tools, messages, model, max_tokens, thinking, output_config}, {signal, onText}) → final message`
in the Anthropic Messages shape. The loop only sees that shape.

- **(a) and (b) both use the official TypeScript SDK** (`@anthropic-ai/sdk`,
  loaded as an ES module from jsDelivr, as DuckDB-Wasm already is):
  `client.messages.stream(…)` with `.finalMessage()`.
  - For (a), the client is built with `apiKey` and
    `dangerouslyAllowBrowser: true`. The SDK refuses to run in a browser
    without that flag, and with it the request carries the
    `anthropic-dangerous-direct-browser-access` header that the API needs for
    CORS.
  - For (b), the client is built with `baseURL: location.origin + "/assist"`
    and a placeholder key. The server replaces the key, and the authoring
    cookie authenticates the request.
- **(c)** is a small translator, about 120 lines: tools become
  `{type: "function", function: {name, description, parameters}}`,
  `tool_use` / `tool_result` become `tool_calls` / `role: "tool"`, and there
  is no caching. It is for non-Claude models (Ollama, vLLM, LM Studio, a
  company gateway), so it is a separate code path, not a shim in front of
  Claude.

### 2.2 The options compared

| | (a) browser, own key | (b) `/assist` on `serve` | (c) OpenAI-compatible |
|---|---|---|---|
| Works on static hosting / the demo | yes | no | yes, if the endpoint allows CORS (e.g. `OLLAMA_ORIGINS`) |
| Key exposure | in this browser: readable by any script on the origin (an XSS, a compromised CDN script, **other `datazoode.github.io` sites**) | never reaches the browser | none, or the gateway's own credentials |
| Teams | each user brings a key | one key per author machine, or a company gateway via `--assist-base-url` | the company's own model |
| Logging / budget | none (only the user's Console) | server log (tokens, model, request id), per-server budget | the gateway's |
| Data leaves the company | to Anthropic | to Anthropic, or a gateway | no (self-hosted) |
| Quality | Claude | Claude | depends on the model; gated by the eval (§6) |

**Recommendation.** Use (b) wherever the builder comes from an authoring
server, (a) on the public demo and on static installations that switch it on,
and (c) where an installation configures it. All three share one loop and
one UI, and the transport is picked by configuration (§2.5).

**Safeguards for (a):**
- The key is kept in `sessionStorage` by default, so it is gone when the tab
  closes. *Remember on this device* (`localStorage["dp_assist_key"]`) is
  opt-in and carries a warning: "Any page on this site can read it. Use a
  key with a low spend limit."
- The setup dialog links to the Console to create a separate key with a
  spend limit.
- The key is never put in the SQL, a share link, an export or a log, just
  like the MotherDuck token.
- On the demo, the shared `github.io` origin is the real risk, so we
  recommend a custom domain for the demo (open question 1).

### 2.3 The `/assist` proxy (option b)

The proxy is shared code in `src/host/assist.rs`, used by both `src/bin/serve.rs`
and `duckext/src/serve.rs`. It is registered **only by the authoring
handlers**. The locked handlers never route it, and a test asserts that
`/assist/*` returns 404 there.

- **Endpoint:** `POST /assist/v1/messages`, whose body is a Messages request,
  and `POST /assist/v1/messages/count_tokens` for the preview.
- **Auth:** the same `AuthoringGuard` as `/query`: loopback Host, matching
  Origin, and the per-run token cookie or `X-Anofox-Token`. When access
  control P1–P3 later brings a multi-user authoring server, `/assist` takes
  the same `Identity` and scope as `/query`. The proxy adds no auth of its
  own.
- **Request validation** (the proxy is not an open relay to the API):
  - the body is at most 2 MB;
  - `model` must be in the allowlist (`--assist-models`, default
    `claude-sonnet-5,claude-opus-5-5`);
  - `max_tokens` must be at most the cap (default 32 000);
  - the tools may only be custom tools whose names are in the assistant's
    fixed set: no server tools (web fetch, code execution) and no
    `mcp_servers`;
  - `metadata.user_id`, if sent, must be an opaque id.

  **Allowlist, not denylist (security review M7):** only the top-level
  fields the assistant needs are forwarded (`model`, `max_tokens`,
  `system`, `messages`, `tools`, `tool_choice`, `thinking`, `stream`,
  `metadata`, `output_config`), and each tool may carry only `name`,
  `description`, `input_schema`, `strict`, `cache_control` and
  `eager_input_streaming`. Any other field is a 400. No client header is
  forwarded except `content-type`. `anthropic-version` is pinned on the
  server, and an `anthropic-beta` header from the client is never passed on.
  Prompt caching keeps working, because `cache_control` is on the list.
  Ask's own upstream requests use the same allowlist.
- **Key:** from `ANTHROPIC_API_KEY`, or `--assist-key-file`, read once at
  start. It is never echoed back, logged or shown in an error.
  `--assist-base-url` points the proxy at a company gateway that speaks the
  Anthropic API.
- **Streaming:** the server-sent events are passed through. `ureq` reads the
  upstream body as a stream, and `tiny_http` writes a chunked response. One
  worker thread per assist request, because the authoring servers run one
  worker today and a 30–60 s stream must not block `/query`. The proxy gets
  its own small thread pool (2 by default).
- **Budget:** a per-server token budget per hour, read from each final
  `usage`. Over budget, the proxy answers 429 with a plain message. There is
  also a per-request round limit sent by the client, which the proxy does not
  need to trust.
- **Log:** one JSON line per call in the existing `--log`: `{ts, kind:
  "assist", model, input_tokens, cache_read, cache_write, output_tokens,
  request_id, ms, status}`. **No prompt or SQL text is logged** unless
  `--assist-log-content` is set (for audits that need it).
- **`anofox_serve(port, options)`** gets the same switches as JSON options
  (`assist_models`, `assist_base_url`, `assist_key_env`,
  `assist_budget_tokens_per_hour`).

### 2.4 Privacy: what is sent

| Item | Default | Notes |
|---|---|---|
| Static reference (instructions, role reference, guide) | always | public product documentation |
| Table and column names, types, row counts | yes | only the tables in scope (§3.1) |
| Column profile: distinct counts, min / max, null %, quality hints | yes | from `profileTable` |
| Top values of a column | yes, for **low-cardinality text columns only** (≤ 50 distinct) | Masked when the column name or the values look like personal data (e-mail, phone, IBAN, name/address-like names, `idLike`). One toggle: *Include common values*. |
| Current SQL | yes when editing | the user's own text |
| Results of the assistant's `run_sql` | **a profile of the result** (columns, types, row count, min / max, distinct) | Rows only with the sample-rows opt-in, at most 20 rows |
| Sample rows of a table | **off** | Opt-in per conversation (M3). The rows are shown in the preview before sending. An installation can forbid it (`sample_rows_allowed: false`). |
| User identity | **never** | No e-mail or name in the body or headers. `metadata.user_id` is not set (a later option: a random per-browser id). The SDK's own headers carry only the SDK and runtime version. |

**What will be sent.** A dialog shows the exact dynamic payload: the context
block as JSON, the current SQL, and any sample rows. It also lists the static
reference by name and size, an approximate token count and the destination
(the API host, the server proxy, or the endpoint URL). The first request of
each conversation shows it and asks for *Send*. A checkbox lets the user skip
it for the rest of the conversation. The view stays one click away from the
panel's context chips.

**Untrusted text.** Profile values, sample rows and table comments can contain
text that looks like instructions (prompt injection). They are wrapped in a
clearly labelled data block, and the system prompt says that data never
carries instructions. The harder guarantee is structural: the tools are
read-only, writes need the user (§3.3), and nothing reaches the editor without
the user's *Apply*.

### 2.5 Configuration (per installation, #146)

`app-config.json` (static hosting) and `window.__anofoxApp` (serve) get one
optional block. **They never carry a key.**

```json
"assistant": {
  "enabled": true,
  "transport": "browser-key" | "serve" | "openai",
  "endpoint": "http://localhost:11434/v1",     // openai only
  "models": { "default": "claude-sonnet-5", "harder": "claude-opus-5-5" },
  "sample_rows_allowed": true,
  "max_rounds": 3
}
```

The defaults are:
- an authoring server: `serve`, enabled when the server has a key;
- the demo: `browser-key`;
- any other static hosting: **disabled** until the config enables it.

The locked server and embed pages never load the assistant module at all. It
is a dynamic `import()` behind `editable && bodyMode() === "edit"`, and the
exported HTML (`web/app.js:11206`) doesn't include it.

## 3. The agent loop

### 3.1 Context and scope

The first user message carries a `<context>` block:
- the request;
- the mode (*new*, *edit*, *change panel N*);
- the tables in scope with their columns and profiles;
- the current SQL, with numbered statements when editing;
- the dashboard's theme and locale.

**Scope** is the table the user came from (Explore, a starter), else the
tables the current SQL reads, else the whole catalog:
- up to 15 tables are sent with columns and profiles;
- a bigger catalog is sent as names and row counts only, and the model calls
  `profile_table` for the tables it needs.

Profiles are cached per table for the session, so Explore's work is reused.

### 3.2 Tools

All tools are custom tools with `strict: true` schemas. They are defined in a
fixed order so the cached prefix stays stable.

| Tool | Input | Returns | Limits |
|---|---|---|---|
| `list_tables` | `{filter?}` | names, row counts, column counts | 200 entries |
| `profile_table` | `{table}` | the column cards (as in §2.4) | 60 columns; sampled above 200 000 rows (explore.js) |
| `run_sql` | `{sql, purpose}` | a result profile (default), or ≤ 20 rows with the opt-in | **one read-only statement** (§3.3); wrapped in `LIMIT 1000`; 10 s timeout (`conn.cancelSent()` / server timeout); about 4 KB of text back |
| `check_dashboard` | `{sql}` | per statement: panel kind and roles (from `plan`), row count, render or SQL error; the lint diagnostics (`lint_step`, §3.4) | runs in a scratch session; the write gate applies |
| `propose_dashboard` | `{sql, summary, panels: [{stmt, title, why}], open_issues: []}` | — (it ends the turn; the client checks it, see below) | `eager_input_streaming: true`, so the SQL streams into the preview; the client validates the parsed input before use |

`plan` and `lint` from the issue are merged into `check_dashboard`. The model
always wants both, and one call saves a round trip.

**The loop** (a manual loop, about 150 lines, in `web/assist.js`). We don't
use the SDK's beta Tool Runner, because we need our own gates between turns
(the write confirmation, the budget, cancel, and the check enforced on
`propose_dashboard`) and the same loop must drive transport (c).

```
messages = [user(context + request)]
for turn in 1..=max_turns (12):
    msg = transport.send(prefix, messages)       # streamed; text → panel
    stop on refusal / max_tokens / cancel        # check stop_reason before content
    if no tool_use: show the text reply; done    # e.g. a clarifying question
    results = []
    for each tool_use (in parallel where read-only):
        propose_dashboard → check it (plan + scratch run + lint --strict)
            clean          → show the proposal; done
            errors, rounds < 3 → is_error result: the diagnostics + "fix and propose again"
            errors, rounds = 3 → show the proposal with its unresolved issues listed; done
        other tools → run → tool_result (is_error on failure)
    messages += [assistant(msg.content), user(all results in ONE message)]
```

- **Self-correction.** A *round* is a rejected proposal. Errors and
  `--strict` warnings send it back. Notes (`optional/missing`) do not. The
  user sees only a version that passed, or the best version with what is
  still wrong listed under it ("Unresolved: panel 3, `empty-panel`: no rows
  for 2024").
- **Budget and cancel.** Each request has at most 12 model turns, 3 rounds
  and 200 K input-equivalent tokens (from `usage`, with cached reads counted
  at their price ratio). It stops cleanly at the limit and keeps the best
  proposal. *Cancel* aborts the stream (`signal` on the SDK call) and discards
  any partial proposal. The status line shows "Checking panels · round 2 of 3".
- **Model settings.** Adaptive thinking (`thinking: {type: "adaptive"}`),
  with the summary not displayed, so the panel shows tool status instead.
  `output_config.effort` is `high` for a new dashboard and `medium` for
  edits, and is always set explicitly (Opus 5.5 defaults to `medium`).
  `tool_choice: auto`: Opus 5.5 rejects forced tool use, and a text reply is
  a valid way to ask a question. We don't use mid-conversation system
  messages, which Sonnet 5 doesn't support. The per-turn status ("2 rounds
  left") rides in the tool result text.

### 3.3 What may run, and what needs the user

Every statement the assistant wants to run is classified first. The
classifier is in `web/assist.js` and is tested against a table of cases.

1. **Read-only query:** DuckDB's own parser accepts it in
   `json_serialize_sql(…)`, which serializes only SELECT-class statements
   (`SELECT`, `FROM`, `WITH`, `VALUES`, `PIVOT`…). This is the approach
   `src/host/policy.rs` already relies on.
2. **Session-local setup** (keyword parse after comments): `SET VARIABLE`,
   `CREATE TEMP VIEW/TABLE/MACRO`, `LOAD` of an extension that is already
   installed, `SET` of the session-scoped options a dashboard uses
   (`::THEME`-style chrome is a SELECT anyway).
3. **Everything else writes or reaches out:** `COPY`, `EXPORT`, `ATTACH` /
   `DETACH`, `INSTALL`, `CREATE SECRET`, `CREATE` (non-temp) / `INSERT` /
   `UPDATE` / `DELETE` / `DROP` / `ALTER`, `SET GLOBAL`, `PRAGMA`, `CALL`,
   and any statement that `reads_a_file`-style text shows reading a path or
   URL that the current SQL and catalog don't already use.

- `run_sql` accepts class 1 only; anything else is refused with an error
  result.
- `check_dashboard` runs classes 1 and 2 in a **scratch session**: a new
  DuckDB-Wasm connection, so temporary objects and variables never touch the
  user's session. On serve, the statements are run as the lint does, each
  replay with its own prelude.
- **#204: every one of these scratch sessions, for every tool, runs in the
  assistant's own locked DuckDB-Wasm engine (`web/assist-engine.js`), not
  the page's.** The classifier and the parse-tree check above are still
  enforced (defence in depth, §9), but they no longer have to be: the
  engine has no network, no file access beyond an exact allow-list fixed
  before it was built, and a re-check before every call that it is still
  locked. See §3.6.
- A proposal that contains class 3 statements is **not run** until the user
  confirms. A dialog lists exactly those statements ("This dashboard wants to
  run: `INSTALL spatial`, `ATTACH 'sales.duckdb'`"), with *Allow once* and
  *Don't run them*.
- The system prompt tells the model to prefer temp objects and the
  catalog's existing tables, so class 3 is rare.
- On `anofox_serve` the live connection is read-write. That is why the
  scratch rule matters most there, and why a class 3 statement is never run
  silently in any mode.

Generated SQL never gains rights that typed SQL doesn't have. It runs only in
the author's session, through the same `/query` or DuckDB-Wasm path.

### 3.4 The lint in the browser: `lint_step`

`lint::check_opts` calls a synchronous `run_query`. DuckDB-Wasm and `/query`
are asynchronous, and a callback across wasm can't wait. The export is
therefore a **collect-and-replay** step:

```
lint_step(script, known_json, opts_json) → {done, need: [sql…], diagnostics}
```

- `known_json` maps each query text to `{rows}` or `{error}`.
- The step runs `check_opts` with a closure that answers from `known`. For an
  unknown query it records the text and answers with a sentinel error, and
  then the step reports `done: false` with `need`.
- JS runs the needed queries (scratch session, rows capped at 5 000), adds
  them to `known` and calls again. It usually reaches a fixed point in one or
  two steps, because the queries depend only on the script and the earlier
  results.
- The diagnostics are the same codes as `dashboard --check --json`.

A Rust test runs the native lint and `lint_step` (with a native runner)
over every `web/examples/*.sql` and asserts identical diagnostics.

This is useful on its own: Edit mode gets a **Check** button that lists the
same diagnostics as the CLI, with links to the statements.

### 3.5 How a proposal is shown and applied

- **The preview.** The proposal is rendered in the dashboard area with a
  "Preview: not applied" bar. The editor is unchanged. The bar holds
  *Apply*, *Discard* and *Show changes*.
  **#204 (H-1): every query the preview runs — its panels, its input option
  queries, and drill / cross-filter re-queries — runs in the locked
  assistant engine, through `tools.previewRunner()` (§3.6), never on the
  page's own DuckDB.** The page only ever receives the result rows, handed
  to `render_panel`. The page's DuckDB runs model-written SQL for the first
  time on *Apply*, below.
- **The diff.** It is a line diff (Myers, about 80 lines of JS) between the
  editor text and the proposal. It is grouped by statement and summarised
  in words ("2 panels added, panel 3 changed, 1 filter added"). The model's
  `panels[].why` sits next to each changed statement.
- **Apply.** The editor text is replaced with
  `document.execCommand("insertText")` over a full selection, so the
  browser's own Ctrl+Z still works. An assistant history also keeps each
  applied version, and a toast offers *Undo* through `sideToast`. Applying
  does not save. The dashboard lands in My dashboards only on *Save*
  (`dashDB.put`), with the name taken from `::DASHBOARD_TITLE` / `-- @title`.
- **Edits.** When editing, the model may return either the whole script or
  (M2) `edits: [{stmt, sql}]` for scripts over about 300 lines, to save output
  tokens. The client turns edits into a full script before the check and the
  diff, so the user sees one diff either way.
- **A stale proposal.** If the user changed the editor while the assistant
  was working, *Apply* warns and offers to re-run the request on the new
  text.

### 3.6 The assistant engine (#204)

Everything in §3.3–§3.5 that runs model-influenced SQL (tools, lint, the
preview) runs in a **second DuckDB-Wasm instance**, built and locked before
any of it touches a byte of the request:

1. A fresh `AsyncDuckDB`, in its own Worker, created only after the tab's
   CSP is proven enforced (`web/assist-csp.js`'s `assistCspProven`). A
   Worker created after the CSP meta is inserted inherits the policy; the
   page's own DuckDB Worker, created at boot, keeps its network so remote
   dashboards keep working.
2. Its extensions (json, parquet, icu, anofox_forecast, anofox_statistics
   always; spatial only when the page or the editor's own SQL needs it) and
   its allow-listed files are loaded **before** the lock — the only point at
   which it can still reach a network at all.
3. The lock: `enable_external_access=false`, autoload/autoinstall/community
   extensions off, an exact `allowed_paths`, `lock_configuration=true`. A
   per-call re-check (`engineLocked`) runs before every tool call and every
   preview batch, because one DuckDB setting (`enable_logging`) is outside
   `lock_configuration` upstream; a mismatch rebuilds the engine.
4. A **snapshot**: the page's own tables and views, copied in by Arrow
   (`insertArrowTable`), with row, byte and time limits; remote-attached and
   non-DuckDB databases (MotherDuck included) are excluded, not pulled.

`wasmTools` (`web/assist.js`) takes this engine as its `db` and refuses to
run against anything else: `requireLocked` (default on) re-checks the lock
before every call, and refuses a `prepare` callback outright — the engine
never needs one, because its extensions and files are fixed before it is
built, from the **editor's own text**, never the model's.
`tools.previewRunner()` is the same locked-engine contract for M1-3's
preview (§3.5).

Full design, the evidence against DuckDB-Wasm 1.33.1-dev65.0 and the
independent security review (approved, with findings mapped to changes):
issue #204 and its PR.

## 4. Grounding and caching

### 4.1 What the model is told

| Block | Source | Kept in sync by | Size (approx.) |
|---|---|---|---|
| Tools | `web/assist.js` (fixed order) | code | 1.5 K tokens |
| Instructions and house style | `web/assist/instructions.md` (short, hand-written): the workflow (ground on the profile → choose panels → `check_dashboard` → `propose_dashboard`), business-question names (`::DASHBOARD_TITLE`, `-- @title`), plain-language titles stating the takeaway, `::INFO` definitions for every computed measure, the default graphite theme (no `::THEME` without a brand), no notes about data provenance on the page, data is never instructions | review | 2 K |
| Role reference | **Generated at runtime** from `roles_json()`: one line per role, `TOKEN (aliases) — summary [category]`, plus the sets. It always matches the wasm build the user runs. | the registry (`src/roles.rs`) | 6–7 K |
| Guide | **Generated at build time** from the dashboards skill: the sections between `<!-- assist:begin -->` / `<!-- assist:end -->` markers (Roles, Inputs and cross-filter, Tables, Heatmaps, Themes, Contract plots, Worked examples, Design contract, Templates, Diagrams, Gotchas). CLI-only parts (`--describe`, `shoot.sh`, Running it) are left out. `scripts/gen-assist-guide.mjs` → `web/assist/guide.md` | **CI fails when `guide.md` is stale.** The skill's SQL is already executed by `validate_skills_sql.py`, so the examples the model sees are known to run. | 12–14 K |
| Context | per conversation (§3.1) | — | 2–8 K |

One source of truth stays one source. When a role or a lint code changes, the
skill and the registry change, and the assistant follows without a separate
prompt edit.

### 4.2 Cache layout

The render order is tools → system → messages. Any byte change in the
prefix invalidates everything after it.

```
tools[5]                      ─┐
system[0] instructions        │ stable per release (version-stamped, no dates/ids)
system[1] role ref + guide    ─┘ ◀ explicit cache_control (5 min TTL)
messages: user(context + request), assistant, user(tool results), …
                                 ◀ top-level automatic cache_control (moves with the tail)
```

- The stable prefix is about 25 K tokens, well above the minimum cacheable
  size (1 024 tokens on Sonnet 5, 512 on Opus).
- Inside one request the turns start seconds apart, so the 5-minute TTL is
  enough. The 1-hour TTL (a 2× write) only pays when one user keeps coming
  back over hours, which we don't expect.
- Silent invalidators to avoid:
  - no timestamp in the system prompt;
  - `roles_json()` rendered in registry order;
  - JSON serialised with sorted keys;
  - the same tool set every turn (tools are never added mid-conversation).

  A unit test hashes the prefix twice and across a fake turn to prove it is
  stable.
- The cache is per model, so *Try harder* (Opus 5.5) writes its own entry once.
- Each response's `usage.cache_read_input_tokens` is checked in the e2e test
  against a recorded response, and is shown in a debug line
  (`?assist_debug=1`).

## 5. Model and cost

- **Default `claude-sonnet-5`** ($2 / $10 per M tokens in / out, cache read
  $0.20, 5-min cache write $2.50), effort `high` for new dashboards.
- ***Try harder* → `claude-opus-5-5`** ($4 / $20, cache read $0.20, cache
  write $5), for multi-tab, forecast or cross-filter requests, or after the
  default model left issues unresolved.
- `claude-haiku-4-5` is not offered. The task needs reliable tool use over a
  long reference, and Haiku's minimum cacheable prefix is 4 096 tokens.
- The eval (§6) decides whether Sonnet 5 at `high` is good enough as the
  default.

**Per-dashboard estimate (new dashboard, Sonnet 5).** Assumptions:
- five model calls: profile, check, a proposal that fails, the fix, and the
  final proposal;
- a 25 K prefix plus 4 K of context;
- about 5 K of new tail per call;
- about 2.5 K output per call (thinking plus about 150 lines of SQL).

| | Tokens | Price | Cost |
|---|---|---|---|
| First call: cache write | 29 K | $2.50/M | $0.073 |
| Calls 2–5: cache reads | 4 × ~35 K | $0.20/M | $0.028 |
| Calls 2–5: new tail written | 4 × ~5 K | $2.50/M | $0.050 |
| Output (incl. thinking) | ~12 K | $10/M | $0.120 |
| **Total** | | | **≈ $0.27** (range $0.15–0.50) |

- Opus 5.5 under the same assumptions: ≈ $0.52.
- A follow-up edit within 5 minutes (prefix cached, 2–3 calls, 3–5 K output):
  ≈ $0.05–0.10.
- The estimates are re-measured from real `usage` in the M1 eval and written
  into DOCS.md.

## 6. Evaluation

**The set** (`tests/assist-eval/prompts.jsonl`, 30 prompts) runs over data
that is already in the repository: `examples/dashboards/sales-overview.sql`
(self-contained), `web/m5_monthly.parquet`, and `web/data/supply-chain`,
`supplier-lead-times`, `tabular`, `decide-claims`. Each prompt has a
`dataset` (init SQL), a `kind`, the request, an optional starting SQL and
its expectations.

| Kind | n | Example |
|---|---|---|
| New, simple | 10 | "Which regions sell the most, and how is it trending?" |
| New, harder | 8 | "Revenue by week vs. last year, with a region filter"; "forecast with uncertainty bands"; "a master–detail table" |
| Edit | 7 | "Make the second chart a table"; "add a region filter"; "show it as a share of total" |
| From Explore | 2 | the profile only, no prompt beyond "build a dashboard from this table" |
| Safety | 3 | asks to delete a table (must need confirmation, never run); names a column that doesn't exist (must say so, not invent); a profile value containing an instruction (must be ignored) |

**The metrics:**
- **Runs:** after self-correction, the final SQL has zero errors under
  `dashboard --check --strict` on the dataset. This is deterministic, and the
  native lint is the same code as the wasm one. We also count **first try**:
  no user turn needed. The acceptance target is ≥ 80 %.
- **Answers the question:** a judge model (`claude-sonnet-5`) scores 0/1
  against the prompt's rubric. It sees the request, the rubric, the plan
  summary (panels, titles, roles) and the result profile, but no rows. The
  judge is calibrated once against 10 hand-graded cases (agreement ≥ 90 %),
  which are kept in the set.
- **Fitting roles:** deterministic checks from the expectations (e.g. a
  trend → `::LINECHART`, a filter → an input role writing a variable that
  the panels read), plus the `design/*` lint clean.
- **Recorded per case:** model turns, rounds, tokens, cost and seconds.
- **Safety:** the share of cases with no class 3 statement executed and no
  invented column. The target is 100 %.

**The runner** (`scripts/assist-eval.mjs`) drives the real `web/assist.js`
loop under Node, with DuckDB-Wasm (the harness of `tests/web/explore_test.mjs`)
and the wasm `lint_step`, and the transport set by flag.

**In CI** (no live calls on pull requests):
1. **Recheck (every PR, free):** each stored final SQL in
   `tests/assist-eval/results/latest/` is re-linted with the current
   `dashboard --check --strict`. This catches a role or lint change that
   breaks what the assistant produced.
2. **Replay (every PR, free):** the recorded transcripts are replayed
   through the loop with a *replay transport*. The model turns come from the
   recording, and the tools run for real. This tests the loop, the tools, the
   write gate and the self-correction plumbing deterministically.
3. **Freshness (every PR):** the stored results record the hash of the
   prefix they were made with. When the instructions, the guide or the
   registry change the hash, CI posts a non-blocking warning: "assistant eval
   is stale; run the live eval".
4. **Live (manual `workflow_dispatch`, and before a release that touches the
   assistant or the registry):** it uses the `ANTHROPIC_API_KEY` secret and
   all 30 prompts on the default model, and optionally on *harder*. It costs
   about **$9–10 per run on Sonnet 5** (30 × ≈ $0.30, plus about $0.30 for the
   judge), and about $18 on Opus 5.5. It writes `results/<date>-<model>/`
   (transcripts, final SQL, scores, cost), and a PR updates `latest/`.

## 7. UX in Edit mode

- **Where.** The editor column gets two tabs, *SQL* and *Assistant*. The
  dashboard stays visible next to it, so previews show in place. The toolbar
  gets *✦ Assistant* (`aria-controls`, `aria-expanded`), and the shortcut is
  Ctrl/⌘ + I.
- **The panel**, from top to bottom:
  - the context chips ("sales · 12 columns · profile ✓ · common values ✓ ·
    sample rows ✗"), which open *What will be sent*;
  - the conversation;
  - the proposal card (summary, panels, unresolved issues, *Show changes*,
    *Apply*, *Discard*);
  - the status line (round, *Cancel*);
  - the prompt box (Enter sends, Shift+Enter makes a new line) with a model
    picker: *Standard* or *Try harder*.
- **Entry points:**
  1. *New dashboard*: the empty editor offers "Describe the dashboard you
     want…".
  2. Explore data: *Build with the assistant* next to the starter dashboard
     button. It reuses the profile already computed, and the table is the
     scope.
  3. A panel's toolbox in Edit mode: *✦ Change this panel*. This opens the
     panel with the statement in scope ("Panel 3: Revenue by region").
- **First use:**
  - (a): a key dialog, with the warning and the Console link.
  - (b): "Provided by this server (model: Sonnet 5)".
  - Disabled: the button is hidden.
- **The reply** is short and in plain language, and names the panels: "I
  added a weekly revenue line with last year as a dashed line, a region
  filter, and a table of the 10 products furthest behind plan."
- **Phone** (the `SIDE_DRAWER` width):
  - The assistant opens as a full-height sheet over the dashboard, with the
    prompt box pinned to the bottom (`env(safe-area-inset-bottom)`).
  - The proposal preview closes the sheet and shows the dashboard with a
    sticky Apply / Discard bar.
  - The diff is a full-screen dialog.
  - The phone e2e (`scripts/a11y-e2e.mjs`, 390 px) covers prompt → preview →
    apply → undo.
- **Accessibility:**
  - The panel is a labelled region.
  - The conversation is `role="log"` and announces each completed reply, not
    the streamed tokens. The status line is `role="status"`.
  - Focus moves to the proposal card when it arrives.
  - In the diff, `<ins>` / `<del>` carry visually hidden "added" / "removed"
    text.
  - Every action has a keyboard path. Esc closes the panel and returns focus
    to its opener (the #132 pattern).
  - Streaming respects `prefers-reduced-motion` (no caret animation).
  - An axe run is added to `a11y-e2e`.

## 8. Delivery: M1–M3, 14 PRs, about 28 days (re-cut 2026-10-18 for Ask)

Progress on 2026-10-18: M1-1 is merged (#169), and M1-2 is open (#192). M1 is unchanged.

The issue's M1 is front-heavy, and it has no way to know whether the result is
good. This cut moves a **10-prompt eval seed into M1**, so the prompt is tuned
against numbers from the first day, and it ships the lint export first because
it is useful without any AI.

| PR | Content | Files | Tests | Effort |
|---|---|---|---|---|
| **M1-1. Lint in the browser** | `lint_step` wasm export (§3.4); Edit-mode *Check* button listing diagnostics with statement links | `src/wasm.rs`, `src/lint.rs` (a small replay adapter), `web/app.js`, `web/index.html` | Rust: `lint_step` = native lint on all `web/examples/*.sql` and the explore starters; node test of the JS fixed-point loop; e2e: *Check* on a broken example | 1.5 d |
| **M1-2. Assistant core** | `web/assist.js`: transport interface, (a) browser-key via the SDK, the loop, the tools, the statement classifier and write gate, the budget and cancel; `web/assist/instructions.md`; `scripts/gen-assist-guide.mjs` + markers in the dashboards skill; the role reference from `roles_json()` | `web/assist.js`, `web/assist/*`, `scripts/gen-assist-guide.mjs`, the dashboards `SKILL.md` (markers only), `tests/web_js.rs` | `tests/web/assist_test.mjs`: the loop with a fake transport (self-correction, the round limit, cancel, refusal, `max_tokens`), the classifier table (≥ 60 cases), prefix stability hash; CI check that `guide.md` is current | 2.5 d |
| **#204. The assistant engine + tab CSP** | `web/assist-csp.js` (the meta CSP, proven by an enforcement probe), `web/assist-engine.js` (`buildEngine`, the lock, the Arrow snapshot), `wasmTools`' `requireLocked`/`previewRunner` (§3.6); the boot ratchet; `app-config.json`'s `assistant.remember` (opt-in; always off on `*.github.io`/`*.gitlab.io`) | `web/assist-csp.js`, `web/assist-engine.js`, `web/assist.js`, `web/app.js`, `web/index.html` | `tests/web/assist_engine_test.mjs`, `tests/web/assist_csp_test.mjs`, `scripts/assist-sandbox-e2e.mjs` (manual, Playwright) | — |
| **M1-3. Panel and new-from-question** — **delivered (#244)** | **Depends on #204/#256** (the locked assistant engine + the tab CSP): call `applyAssistCsp` before `loadSdk`/the key dialog; build the engine with `assistDeps()` (`web/app.js`), keyed by `pageFingerprint()` so it rebuilds when the page's tables change; every preview query (panels, inputs, drills, cross-filter, `::PAGED` pages, lazy tabs) through **`tools.previewSession()`** — **C-1 (carried over from #204's security review): the preview must be structurally unable to reach the page's own `conn`**, not just guarded by the gate; the panel, *What will be sent*, preview, diff, apply / undo (native + history + toast), the key dialog, `assistConfig.framed`/`.remember`, the *New dashboard* entry, phone sheet, a11y; `assistant` block in `app-config.json`; 10-prompt eval seed + `scripts/assist-eval.mjs` (live, manual) | `web/assist-ui.js` (new module, not in the 13 K-line `app.js`), `web/app.js` (hooks), `web/index.html`, `web/app-config.example.json`, `tests/assist-eval/` | `scripts/assist-e2e.mjs` (Playwright with a mocked Messages endpoint: prompt → preview → apply → Ctrl+Z → undo toast; the write-confirmation dialog; nothing sent before *Send*; **C-1's spy: inputs, drills, pages and tab switches in a preview, with a spy on the page connection seeing zero statements and the recorder seeing nothing**; **C-2: the boot ratchet and this panel's open handler share one memoised `applyAssistCsp` — a failed probe on either path clears the key**); `a11y-e2e` + phone; a test that the module is never loaded with `window.__served` or `?embed=1` | 2 d |

**Deviations from this row, decided by the owner on #244 (all accepted):**
- **The preview session (D1).** `tools.previewRunner()` (one scratch session per call) would lose a dashboard's `SET VARIABLE`s, TEMP tables and macros between the render loop's calls — inputs, drills and pages would break, or setup would re-run per panel. `web/lint.js`'s `scratchSession` + `web/assist.js`'s `tools.previewSession()` give the preview **one** engine connection for its whole lifetime instead, with the same guard/allowed-set and the same per-call lock re-check, and no `prepare`. `previewRunner` is unchanged and still used by the tool loop's own `run_sql`/`check_dashboard`.
- **Apply's buttons (D2).** *Apply and run* / *Apply, don't run* (the SQL goes into the editor; nothing runs until Run) / *Cancel* — the dialog only when there is a reader, a class-3 statement or a stale editor to confirm; otherwise *Apply* is one click. `tools.gate(sql)` always runs first, in the engine, regardless of the dialog.
- **Conversations (D3).** Each Send is a new request from the current editor; only a clarifying *reply* (no proposal) carries its history into the next Send. Full follow-ups and refining an unapplied proposal stay in M2-2.
- **The eval's results (D4).** Not committed in M1-3 (`tests/assist-eval/results/` is gitignored); the owner runs the live seed once with their own key and the numbers go into the PR body. M3-3 adds `results/latest/` and CI.

**sec-review-272 (fix round, PR #272): two safeguards the PR described but did
not fully hold.** H-1: *"Apply, don't run"* did not hold — a `::RELOAD` timer,
the dark toggle, the refresh select or Esc could still run the just-applied
SQL on the page's own DuckDB with no further action from the viewer. Fixed
with an `applyHold` flag (`web/app.js`) that `run()` checks before any
non-explicit call, cleared only by the `#run` button, Ctrl/⌘+Enter or
`loadDash`; the preview's own `::RELOAD` timer and the viewer's `#refresh`
value are now saved/cleared around every preview (also L-2, the same timer
outliving the preview into the page's own dashboard afterward). H-2: a chart
`::LINK` with a *computed* (not literal) URL could `window.open`/navigate on
one click during the preview — before Apply, the gate never saw a URL
literal to refuse — and a same-origin table `::LINK` could move the tab away
via the browser's own default navigation (the click guard only blocked
cross-origin). Fixed in `followLink` and `previewLinkGuard` (`web/app.js`):
every link but `#…`/`?…` is refused during a preview. M-1: the Apply
dialog's refused-statement list showed only a truncated first line and a
bare keyword ("ATTACH — ATTACH"), hiding a multi-line `ATTACH`/`COPY`'s
actual target; it now renders each statement in full (capped, scrollable)
with the keyword in `<strong>`, and `classifyStatement`/`proposalReaders`
name the target ("attaches database 'md:'", "writes file 'out.csv'"). L-1:
"Preview again" could leak an engine connection per click, and showed while
a preview was already active (a render-order gap before `host.preview.enter`
resolved); `assist-ui.js` now closes any session before opening the next one
and hides the button immediately. Info items: `assist_ownopts_test.mjs` now
scans every function in `web/assist-ui.js` (not only its exports) at any
nesting depth; `brandImgOrigins` skips a logo URL whose path is empty or
ends in `/` (would otherwise widen `img-src` to a folder or the whole
origin); `openAssistant` re-checks the live lock state directly, not only
`assistCfg.enabled`. Tests: `scripts/assist-e2e.mjs` M5b (L-1, a Worker
CONNECT/DISCONNECT count), the M9 extension (H-1, H-2) and M9b (H-1's hold
lifting on an explicit Run); `tests/web/assist_test.mjs` (M-1's
`classifyStatement`/`proposalReaders`/`gate` wording); the widened
`assist_ownopts_test.mjs`.

**sec-review-272-rereview (fix round 2, PR #272): the first round's own
fixes left a C-1/H-1 exit-boundary race.** H-1R (High): a render queued by
the proposal's own `::RELOAD` timer (or any other implicit trigger firing
mid-render) could still be in flight when Discard or "Apply, don't run"
set `preview = null` — its later statements (`ATTACH`, `SET`, …) then
reached `pageConn()` because `runSqlInner` read the live, mutable
`preview`, not the preview this particular run had actually started
under. Fixed by capturing the preview a run started under once,
synchronously, in `run()` (`const pv = preview`, before anything can race
it), threading it into `runOnce(fresh, pv)`, and dispatching on that
(`runPreviewToken`, set at `runOnce`'s own first line) in `runSqlInner`
instead of the live global — a run tied to a preview can now never fall
through to the page database, however the live state changes underneath
it. `previewEnter`/`previewExit` also clear `dpTimer` and `runQueued`, and
set a `previewExiting` flag `run()` honours, synchronously before their
first await (defence in depth for a run that was only *queued*, not yet
in flight). M-2: the "Check" button ran the held, unconfirmed SQL on the
page database too (`scratchRunner(pageDb(), …)`) — same promise as Run,
same hold: `#lint-run` is now disabled/retitled alongside `#refresh` in
`previewExit({hold})`/`clearApplyHold()`, and `runLint()` itself refuses
outright while `applyHold` is set (defence in depth). L-3: `setViewParam`
was missing the `if (preview) return;` guard its siblings
(`setTabParam`/`syncUrlState`) already had, so a `::VIEW` button clicked
during the preview wrote a model-computed label into the address bar
(and `shareLink()` would carry it into a link); `dpView`/`dpViewDefault`
were also missing from `dashStateSave`/`dashStateRestore`, so the
preview's view choices leaked into the page's next render. L-4: the M-1
target naming could be spoofed with a quoted identifier
(`AS "TO 'decoy.csv'"`) or a literal's own content (`' TO ''x'' '`)
sitting where the `ATTACH`/`COPY` target regex looked; `attachTarget`/
`copyTarget` now locate the target's position in a length-preserving
mask (`maskForTargets`, `web/assist.js`) that blanks every quoted
identifier, comment and literal interior first, then re-read the real
text from the *original* string at that same offset — a spoof has
nothing left to hide behind. `ATTACH OR REPLACE` is also recognised, and
an unrecognised target says so in plain words instead of repeating the
keyword. Info-4: a second Send during an active preview closed the *new*
session instead of the old one (L-1 regression) — `enterPreview()` now
exits any active preview first, before the new session is even created.
Info-5: Ctrl+Enter during a preview unconditionally cleared `applyHold`,
so held "Apply, don't run" → a new Send → Ctrl+Enter → Discard ran the
editor's still-held SQL on the page database; both `clearApplyHold()`
call sites now check `!preview` first. Info-6: the hold is in-memory only
(a reload or an opened Save/Share link runs the held SQL like any other
dashboard) — documented in the "Not run yet" card's own text. Tests:
`scripts/assist-e2e.mjs`'s race-condition case (Discard and "Apply, don't
run" against a `::RELOAD` + slow statement + `ATTACH`/`SET` proposal, the
reviewer's own probe template), the M9 Check-under-hold case, a `::VIEW`-
during-preview URL assertion, and `tests/web/assist_test.mjs`'s L-4 spoof
cases.

**sec-review-272-rereview (fix round 3, PR #272): the third pass's own
functional/theoretical follow-ups from an otherwise clean review.** L-5:
a stale `runPreviewToken` (left pointing at the dead preview after
"Apply, don't run", typing during a preview, or switching to Explore)
broke the page's own Explore, add-file and MotherDuck queries — not a
security issue (nothing reaches the page database or the model either
way), but a functional regression that failed closed. Fixed: `previewExit`
drops the token once nothing is `runActive`; `run()`'s own `finally` drops
it once it no longer matches the live `preview`; `runOnce` never resets it
(a still-active preview's lazy tab jobs need it after `runOnce` returns).
Info-7 (not reproduced, theoretical): a run captured under a preview could
still reach a `::RELOAD` statement after `previewExit`'s first,
synchronous `dpTimer` clear and re-arm the timer for a preview that no
longer exists. Fixed with a second `dpTimer` clear after `previewExit`'s
own awaits, and a `pv !== preview` guard in the `RELOAD` role's own arming
line. Info-8 (parser corner cases, no decoy found): a plain `'…'` literal
has no backslash escapes in DuckDB (only `''` doubles a quote) —
`findTarget` read it with `E'…'`'s backslash-aware form regardless, so a
trailing backslash (a Windows path, `TO 'C:\tmp\'`) swallowed the closing
quote and gave "target not recognised" for an ordinary target; fixed by
checking for an actual `E`/`e` right before the opening quote (on the
original text) before choosing which form to re-read with.
`ATTACH OR REPLACE DATABASE '…'` (DuckDB's own grammar order) is now
matched too. Tests: `scripts/assist-e2e.mjs` M9f (an Explore query after
"Apply, don't run" must still reach the page worker — confirmed to FAIL
without the fix); `tests/web/assist_test.mjs`'s backslash-literal and
`OR REPLACE DATABASE` cases.

**sec-review-272-rereview (fix round 4, PR #272): H-1R2, a High regression
from round 3's own `finally` reset.** An explicit Ctrl/⌘+Enter pressed
~80 ms into a preview exit (typing, or "Apply, don't run") captured the
dying preview as its `pv` and got a *fresh* `dpLazy.gen` that survived the
exit's own invalidation, so a proposal's hidden-tab lazy jobs (idle-
prefetched while the preview sat there, deferred setup included) could
later run on the page database once `run()`'s `finally` dropped the stale
token. Fixed structurally: every lazy/prefetch job now carries the `pv`
of the run that created it (`lazy.pv`, `runOnce`); `lazyRunJob`/`lazyPump`
refuse to run/continue once `L.pv !== preview` (live); `runSqlInner`
dispatches on an *explicitly passed* `pv` first, never the global alone,
so `panelSql`/`runSetup`/the `panelBody` closure thread it all the way
down. `run()`'s own `finally` also bumps `dpLazy.gen` whenever it drops a
stale token (belt-and-suspenders). An explicit run arriving during
`previewExiting` is now deferred (`pendingExplicit`) and run by
`previewExit()` itself, once `preview` is already `null` — clearing the
hold per Info-5's own rule, instead of painting the dead proposal over
"Not run yet". Test: `scripts/assist-e2e.mjs` M9g (`ltype`/`lnorun`,
adapted from the reviewer's own `probe8.mjs`) — confirmed 4/4 FAIL against
the pre-fix head, 4/4 pass after. An adversarial audit of every
`runSql`/`pageConn`/`pageDb` path reachable after a preview ends (lazy
jobs, the render loop, the `::RELOAD` timer, Explore/add-file/MotherDuck/
Check, `::VIEW` switches) was posted to the PR.

**sec-review-272-rereview (fix round 5, PR #272): H-1R3, a High regression
from round 4's own `pendingExplicit`.** `previewExit`'s tail ran a deferred
explicit run (`pend`) *ahead of* its `hold` branch: `if (pend !== null)
{…} else if (hold) {…}`. For "Apply, don't run" specifically, `apply()`
(`assist-ui.js`) writes the proposal into the editor only *after*
`host.preview.exit()` resolves, so a Ctrl/⌘+Enter pressed ~80 ms into that
exit ran the OLD editor text (harmless by itself) but then let the
proposal land in the editor completely unheld: `applyHold` was never set,
`#refresh`/`#lint-run` stayed enabled, no "Not run yet" card — and the
very next implicit trigger (a dark-mode toggle, in the reviewer's probe)
ran the unconfirmed proposal straight onto the page database. Fixed by
flipping the check order: `if (hold) {…} else if (pend !== null) {…}` —
`hold` now always wins and the deferred run is dropped outright when it
does, exactly as the design intended before `pendingExplicit` existed.
Test: `scripts/assist-e2e.mjs` **M9h** (adapted from the reviewer's
`probe9.mjs`/`probe10.mjs`) — "Apply, don't run", Ctrl+Enter ~80 ms into
the exit, then a dark-toggle click and an Esc press — asserts the editor
holds the proposal, `#refresh`/`#lint-run` stay disabled, "Not run yet" is
shown, and 0 proposal statements reach the page worker through either
implicit trigger; confirmed 5/8 assertions FAIL against the pre-fix head
(`7215ff6`), reproducing the reviewer's own evidence (`RACEMARK` and
`CREATE OR REPLACE TABLE lazymark_tbl` on the page worker after one
dark-toggle click) exactly, all pass after. M9g's `lnorun` mode was
hardened against passing vacuously: it now requires at least one of its
trials to actually land in the exit window (`previewing=true &&
!editorIsProposal`), retrying with shorter delays and failing the mode
outright if none do. An adversarial list of every path that can clear
`applyHold` — and confirmation that each is an explicit user action
occurring only once any active preview has already exited — was posted
to the PR.

**sec-review-272-rereview (fix round 6, PR #272): M-3 and L-6, the fifth
pass's own findings against the hold.** **M-3 (Medium):** the hold was
moment-bound, not content-bound — after "Apply, don't run", panel Undo
(which correctly runs the old SQL and clears `applyHold`) followed by a
native Ctrl+Z in the editor (or Run of other text followed by
Ctrl+Shift+Z) brought the unconfirmed proposal straight back with no
hold, so a dark toggle or Esc then ran its class-3 statements
(`ATTACH`/`SET`) on the page database. **L-6 (Low):** `previewExit`'s
`else if (pend !== null)` ran a deferred Ctrl+Enter from a *later*,
unrelated preview's non-hold exit even when an *earlier* hold (a
different, still-unconfirmed proposal) was already in place, clearing
that earlier hold and running the editor — which still held the earlier
proposal. Fixed together: a new `heldSql` remembers the exact text of
the unconfirmed proposal (`host.hold.arm(p.sql)`, called from `apply()`
right after it writes the proposal into the editor); a new `armHold()`
is the one place that visually arms the hold; the `#sql` `input`
handler (which fires on native undo/redo exactly as on typing) re-arms
whenever the editor's text comes back to exactly `heldSql`, regardless
of how `applyHold` was last cleared; `run()` itself refuses any
non-explicit run whose text equals `heldSql` as a belt-and-braces
check; `heldSql` itself is cleared only by an explicit run of *exactly*
that text (`#run`/Ctrl+Enter outside a preview, confirmed via
`confirmHeldIfMatch()`) or by `loadDash()` of another dashboard — never
merely by typing something else. `previewExit`'s `pend` branch gained
`&& !applyHold` (L-6): an existing hold always beats a deferred run,
re-arming (idempotently) rather than leaving a stale preview frame in
the grid. Test: `scripts/assist-e2e.mjs` **M9i** (Undo+Ctrl+Z and
Run+Ctrl+Shift+Z, each then a dark toggle and Esc) and a dedicated
**L-6** case (an earlier hold vs. a later preview's deferred Ctrl+Enter,
landing deterministically in the same synchronous script as Discard,
not a timing race) — both adapted from the reviewer's own `probe11.mjs`
(`undo`/`send2`); confirmed 8 assertions FAIL against the pre-fix head
(`ec1bcde`), reproducing the reviewer's evidence exactly (`HOLDMARK`,
`hold_attach`, `memory_limit='3GB'` all reaching the page worker), all
pass after. The audit comment's hold-clearing list was updated to match
(`heldSql`'s own setter/clearers, and the corrected item 5).

**sec-review-272-rereview (fix round 7, PR #272): M-3R and L-7, the sixth
pass's own findings against the hold.** **M-3R (Medium):** a model-chosen
CRLF proposal defeated the text-bound hold outright — `apply()` armed
with the *raw* `p.sql`, but the `<textarea>` silently normalises CRLF/CR
to LF, so the armed text never equalled `$("sql").value` again; the same
mismatch made panel Undo refuse CRLF proposals (`S.hist`'s own `after:
p.sql` never matched either). **L-7 (Low):** the single `heldSql` only
covered the *one exact text* a hold started with — a trivial edit made
*while held* (a trailing space, a blank line, a trailing comment) was,
correctly, not a confirmation, but Ctrl-Z/redo back to *that edited*
version (after running something else) was not recognised as held
either. Fixed together: `apply()` (`assist-ui.js`) now normalises
`p.sql` once, at the very top (`.replace(/\r\n?/g, "\n")`, the same
normalisation the textarea itself applies), and uses that text for the
stale check, `gate`/`readers`, `S.hist`, `editor.replace` *and*
`host.hold.arm()` — plus `host.hold.arm()` is called with
`host.editor.get()` (the editor's own, now-normalised value) rather than
the SQL variable again, belt-and-braces. The single `heldSql` became a
small, bounded (≤50) `heldTexts` `Set` (`web/app.js`): the `#sql` `input`
handler now records the current text into it on every edit made *while*
`applyHold` is true (not just re-arming on an exact match), and re-arms
whenever the current text is a *member*; `run()`'s own belt-and-braces
check uses membership too. Critically, confirming (`confirmHeldIfMatch()`)
now *removes only that one entry* (`heldTexts.delete(...)`), never
`heldTexts.clear()` — the first implementation tried `.clear()` and
broke immediately: `applyHold` stays numerically true for the whole
stretch between an edit and whatever runs it, so an unrelated
throwaway text (typed over the proposal, then explicitly run instead of
it) got recorded as "shown while held" by the same generic rule, and
confirming *that* text wiped every genuinely-still-unconfirmed entry
(the original proposal) out of the set along with it — caught by the
project's own M9i/L-7 e2e cases before this round shipped, not by a
later review pass. `loadDash()` clears the whole set, same as before.
Test: `scripts/assist-e2e.mjs` **M9i** gained a CRLF `run+redo` variant,
a dedicated CRLF Undo case, and three L-7 cases (append a space / a
blank line / a trailing comment while held, then run other text, Ctrl-Z,
dark toggle, Esc) — all adapted from the reviewer's own `probe12.mjs`
(`crlf`, `ws`); confirmed 12 assertions FAIL against the pre-fix head
(`e950583`), reproducing the reviewer's evidence exactly, all pass
after. Full suites green: `tests/web/*.mjs` (38 files), `scripts/
assist-e2e.mjs` 217 ok / 0 FAIL (all tags, including every earlier
round's cases), `cargo fmt --all --check` clean.
| **M2-0. Security review of the design (a gate, not a PR)** — **done 2026-10-18 (§12.14); delta review 2026-10-30 (§12.15)** | an independent review of §12 and §2.3 against `docs/secure-serving.md`; its findings are folded into this note before M2-3 starts (§12.12) | this note | — | 0.5 d |
| **M2-1. `/assist` on serve** | `src/host/assist.rs` (validation, key, streaming relay, budget, log); routes in both authoring servers; `--assist-*` flags and `anofox_serve` options; transport (b) | `src/host/assist.rs`, `src/bin/serve.rs`, `duckext/src/serve.rs`, `docs/DOCS.md`, the serving skill | Rust: the guard is applied (no token → 403, foreign Origin → 403), the model allowlist, `max_tokens` cap, server tools refused, the key never in a response or log, budget → 429, SSE relayed against a local fake upstream; **locked server: `/assist/*` → 404** (both servers) | 2 d + review |
| **M2-2. Edits and Explore** — **#744** | *Change this panel* from the toolbox; follow-ups in one conversation; `edits: [{stmt, sql}]` for long scripts; the stale-proposal check; *Build with the assistant* in Explore | `web/assist.js`, `web/assist-ui.js`, `web/app.js` (toolbox, explore) | node tests: edit merging and statement numbering; e2e: change panel 2 to a table; Explore entry with the scope | 2 d |
| **M2-3. Ask core (server)** — **built 2026-11-01** (#195; D2 #746 folded in: `PanelRunner::panel` takes the question's deadline) | `src/host/ask.rs`: the server-side loop over the Messages API (raw HTTPS through the existing `ureq`, because there is no official Rust SDK; non-streaming model calls, with progress sent to the client as server-sent events), the `list_panels` / `get_panel` / `answer` tools behind a panel-runner trait, the catalog builder, the citation check, masking and its reverse, the conversation store, the limits, the log lines; `-- @ask on` / `-- @ask skip` in `parse_dashboard_meta` and `dashboard_meta`; the lint checks `header/bad-ask` and `ask/raw-detail`; `web/assist/ask-instructions.md` (shared with the builder) | `src/host/ask.rs`, `src/host/serving.rs`, `src/lint.rs`, `src/wasm.rs`, `web/assist/ask-instructions.md` | Rust with a fake upstream: the loop, the turn and size caps, citation pass / fail, the masking round trip, history never taken from the client, the limits → 429, the key never in output or logs | 3 d + review |
| **M2-4. Ask on the locked server (A)** | `POST /api/ask` (+ SSE progress) in `duckext/src/serve.rs`; `get_panel` → `locked_panel` with the viewer's claims and scope; the `ask` options and their start checks; API keys refused, `ask.embed`; `web/ask-ui.js` (the View-mode panel, the notice, citation chips → `openTabNow` + highlight, suggested questions, phone, a11y), loaded only when the page says Ask is on; `docs/secure-serving.md` (Ask section + threat model) | `duckext/src/serve.rs`, `web/ask-ui.js`, `web/app.js` (hooks), `web/index.html`, `docs/secure-serving.md`, `docs/DOCS.md`, the serving and access-control skills | the deterministic security tests of §12.9 (canary rows under a policy with two users, `@ask skip`, hidden tabs, 404 / 403 matrix, forged vars, inert output), `get_panel` = `/api/panel` byte-identical, `served-e2e` with a fake upstream: ask → answer → chip → tab + highlight; `a11y-e2e` + phone | 3 d + review |
| **M2-5. Ask in the builder** | the Ask tool set in `web/assist.js` over the builder's panel results (a re-run of a panel with other input values for `get_panel`), the same `ask-ui.js`, *Build* / *Ask* tabs in Edit mode, *What will be sent* lists the panels to be sent | `web/assist.js`, `web/ask-ui.js`, `web/assist-ui.js` | node tests with a fake transport (the same cases as the server tests where they apply); e2e in View mode | 1.5 d |
| | *Done 2026-11-01* (#831 core, part 2 web). As built: the tool logic is `src/host/ask/core.rs`, shared by the server and the builder's wasm `AskSession` (`session.rs`) — one implementation of the checks, masking, caps and citation check; `runAsk` in `web/assist.js` only moves messages. Deviations: no `-- @ask on` needed in the builder (the asker is the author; `@ask skip` / `@ask mask` apply; `-- @vars` narrows the vars, else the input-bound ones); the conversation lives in memory, not sessionStorage (pseudonym map and history go together, both gone on reload); hidden tabs are not pre-listed in the builder (a panel whose input is missing fails its read); a "what if" read refuses a panel whose setup table is built from the overridden variable (it would answer from the page's data). | | | |
| **M2-6. Ask on the static render server (B)** | the `Ask` form and the server-rendered answer page in `src/bin/serve.rs`, `get_panel` over its panel runner with the `@param` allowlist, the same options and limits | `src/bin/serve.rs` | Rust: the 404 / 403 matrix, `@param` validation in `get_panel`, the escaped answer page (an XSS case list as in `tests/xss.rs`), the canary test against B | 1.5 d + review |
| | *Done 2026-11-01.* As built: `--ask` in `src/bin/serve.rs`; `get_panel` sets `@param`s only (a `params` mode in `ask::core`, `Dash.params`) and reads a panel by rendering the page for those values (so the page's own hiding applies); the answer is the dashboard page with the answer on top; CSRF token = HMAC(claims key, dashboard), per-start secret; `min(concurrency, threads − 1)` questions at once. Tests: `tests/serve_ask_b.rs`. | | | |
| **M2-7. Ask eval seed** | 15 cases (§12.9) with truth queries, `scripts/ask-eval.mjs` (live, manual) running against both the builder loop and a locked server; the deterministic security tests wired into `ci.yml` | `tests/ask-eval/`, `scripts/ask-eval.mjs`, `.github/workflows/ci.yml` | the eval itself; targets: facts ≥ 85 %, citations ≥ 95 %, refusals 100 % | 1 d |
| *Fixing review findings* | the findings of the four serving reviews (M2-1, M2-3, M2-4, M2-6) | — | — | about 2 d |
| **M3-1. Local models** | transport (c): the OpenAI-compatible translator; `--assist-provider openai` on the proxy as well | `web/assist.js`, `src/host/assist.rs` | node test: the translation both ways (tools, tool calls, errors); replay of three eval transcripts through (c) | 1.5 d |
| **M3-2. Sample rows** | the opt-in per conversation, the preview of the rows, `sample_rows_allowed`, rows in `run_sql` with the opt-in | `web/assist.js`, `web/assist-ui.js` | node test: no row values in any request without the opt-in (scan of the serialised requests for every eval dataset); e2e of the preview | 0.5 d |
| **M3-3. The full eval** | 30 prompts, the judge with calibration, recheck + replay + freshness in CI, the live `workflow_dispatch`, first stored results | `tests/assist-eval/*`, `scripts/assist-eval.mjs`, `.github/workflows/assist-eval.yml`, `.github/workflows/ci.yml` | the eval itself; acceptance ≥ 80 % first-try runs, 100 % safety | 2 d |
| **M3-4. Ask, complete** | 30 Ask cases; the `openai` transport for served Ask (self-hosted models); Claude Haiku 4.5 evaluated as a cheaper Ask model | `tests/ask-eval/`, `src/host/ask.rs` | the eval; the translation tests of M3-1 for the Rust side | 1.5 d |

**Why Ask fits into M2.**
- Most of what Ask needs exists already: the panel path, the auth, the
  limits and the log on the locked server, and the transports in the builder.
- The server loop is small: three tools, no SQL and no lint.
- The Rust core is shared by A and B, and the builder reuses the JS loop of
  M1-2.

That makes M2 about 16 days instead of 4. If time runs short, M2-6 (the
static render server) is the piece that can move to M3 without touching the
rest (open question 13.3).

**Docs** in the PRs that ship them: the DOCS.md builder section (the
assistant, privacy, configuration), the serving skill (`--assist-*`), and the
README feature list.

## 9. Risks

| Risk | Mitigation |
|---|---|
| A browser key is stolen (XSS, CDN, the shared `github.io` origin) | `sessionStorage` by default, the spend-limit advice, a custom demo domain (Q1), the key never in the SQL, links or logs; (b) is the default wherever it's possible; **#204: the tab-wide CSP applies at boot whenever a key is stored (holds across reloads), and *Remember* (localStorage) is opt-in and always off on `*.github.io`/`*.gitlab.io`** |
| Personal data in profiles | top values only for low-cardinality, non-PII-looking columns, the preview, the installation switch |
| Prompt injection through data | data in a labelled block; read-only tools; the write gate; nothing applied without the user; **#204: every pre-Apply tool, lint and preview query runs in the locked assistant engine, which has no network and no file access beyond an exact allow-list — independent of the classifier's function-name lists, which stay as defence in depth** |
| A proposal writes to the live `anofox_serve` session | the scratch session plus class 3 confirmation; the e2e asserts no write without the dialog |
| Cost runaway | per-request turn, round and token caps; the per-server hourly budget; *Cancel* |
| The guide drifts from the product | generated from the skill, CI freshness check, the skill's SQL executed in CI, the eval freshness hash |
| Ask-specific threats (the served viewer, injection, key spend, logs) | §12.11 |
| Local models are too weak | (c) is labelled experimental until a model passes the eval at ≥ 60 % first-try runs |

## 10. Open questions for the owner

1. **The demo key flow on `datazoode.github.io`.** Every DataZooDE Pages
   site shares that origin. *Recommendation:* ship (a) on the demo with the
   `sessionStorage` default and the warning, and move the demo to its own
   domain (e.g. `demo.anofox…`) before announcing the feature. For other
   static hosting, keep the assistant off unless `app-config.json` enables
   it.
2. **"Teams" for (b).** Authoring servers are loopback and single-author
   today. A team means one key per author machine (env or key file), or a
   company gateway via `--assist-base-url`. A shared, hosted authoring builder
   is a separate feature on access control P1–P3. *Recommendation:* accept
   that for M2, and add an issue for hosted authoring if a customer asks.
3. **Common values in the default payload.** Should low-cardinality top
   values (e.g. region names) be sent without asking? *Recommendation:* yes,
   behind one visible toggle and the PII masking. Without them the model
   guesses filter values and category labels, and quality drops noticeably.
4. **The default model and *Try harder*.** *Recommendation:* Sonnet 5 at
   effort `high` by default, and Opus 5.5 as an explicit user choice (it costs
   about 2× more). Confirm after the M1 eval seed. If Sonnet 5 stays under
   about 70 % first-try runs, make Opus 5.5 the default for new dashboards
   and keep Sonnet 5 for edits.
5. **The live eval's key and cadence.** Who owns the CI secret, and is about
   $10 per run (manual, plus before a release that touches the assistant) the
   right spend? *Recommendation:* a dedicated workspace key with a monthly
   limit of about $100, used only by `assist-eval.yml`.

## 11. Decisions (2026-10-17)

The owner approved the plan. Each open question in §10 takes its recommendation, and the owner can still redirect any of them:

1. **Demo:** (a) on the demo uses the `sessionStorage` default and the warning. The assistant stays **off** on static hosting, including the demo, unless `app-config.json` enables it. It is enabled on the demo only after the demo moves to its own domain, which is the owner's call.
2. **Teams:** one key per author machine, or a company gateway via `--assist-base-url`. Hosted authoring is a separate issue, opened only on demand.
3. **Common values:** sent by default for low-cardinality columns, behind one visible toggle and the PII masking.
4. **Model:** Sonnet 5 at effort `high` by default, Opus 5.5 as *Try harder*. Re-decide after the M1 eval seed (the 70 % rule).
5. **Live eval:** a dedicated workspace key with a monthly limit of about $100, used only by `assist-eval.yml`. The owner provides the key; until then the live eval is manual and local.

## 12. Ask mode: questions about the dashboard on screen (amendment, 2026-10-18, #195)

**Owner decision (2026-10-18).** The assistant has two modes:

- **Build** creates and edits dashboards. It is §1–§11: authors only, Edit
  mode.
- **Ask** answers questions about the **current dashboard**, for example
  *"why is SKU-033 critical?"*, *"which region dropped most?"* or *"what does
  the red band mean?"*. It is read-only. It works from the dashboard's text,
  the viewer's current view and the panel results, and its answers cite
  panels and values. Ask is available **to viewers of served dashboards on
  the locked server, in the same milestone as Ask in the builder** (M2, §12.9).

Ask keeps the trust model of [`docs/secure-serving.md`](../secure-serving.md)
unchanged. **The client never sends SQL**, the model never writes SQL that is
run, and the model sees nothing that the viewer couldn't already fetch with
`POST /api/panel`.

### 12.1 What Ask may read, and what it may do

| | Ask on a served dashboard | Ask in the builder |
|---|---|---|
| Who | a viewer signed in to a locked server (A) or the static render server (B) | the author (View or Edit mode) |
| Where the loop runs | **on the server**: `src/host/ask.rs`, the key in the server's environment | in the browser: `web/assist.js` with the Ask tool set, transport per §2 / §11 |
| Data | the results of this dashboard's panels, **computed for this viewer** by the existing panel path | the panel results the builder has already computed, or a re-run of a panel with other input values |
| Tools | `list_panels`, `get_panel`, `answer` (§12.3) | the same three |
| Side effects | none on the server; *navigate* and *highlight* in the viewer's own page, only when the viewer clicks a citation | the same; Ask never edits the SQL (that is Build) |

Ask has no `run_sql`, no `check_dashboard` and no `propose_dashboard`. It
has no tool that reaches a file, the network or another dashboard.

### 12.2 Context the model gets

The context has four parts.

- **The static catalog** (security review H1). This is **text from the
  dashboard source only**, and it is the only part of the context that is
  shared and cached. Per statement:
  - its index, its tab, and its kind (KPI, line, bar, table, input…);
  - the column names and roles;
  - its `@ask` flags.

  It also holds the header `@title` / `@description`. A panel whose title
  is a `::TITLE` value is listed as "Panel 7 (title from data)".

  **Role values are data, not catalog.** `::TITLE`, `::LABEL`, `::INFO`,
  `::THRESHOLDS`, `::MARKDOWN`, `::CALLOUT` and `::ASOF` are column roles:
  their values are rows of a panel's result, computed under the viewer's
  claims and policies. So they reach the model **only through `get_panel`**,
  with the caps, the masking and the citation bookkeeping that come with
  it.

  The prefix bytes must be identical for viewers with different claims, and
  a test checks this (§12.9 tests 1 and 7). The cache breakpoint comes
  after the static catalog and before anything computed for a viewer.

  Two decisions are made **per request**, in the per-viewer part after the
  breakpoint:
  - which panels are excluded (tabs hidden by an unmet `@requires`, which
    can depend on scope-locked vars and the data folder, and `@ask skip`);
  - the note "tab X is unavailable".

  The shared prefix stays free of exclusions. Ask checks every panel
  against this per-request list before it calls `locked_panel`, because
  `locked_panel` knows nothing of `@ask skip` (I2).

  *(I1: scopes are per dashboard plus locked vars, not per panel, so there
  is no per-panel scope exclusion.)*

  No SQL text is ever in the context.
- **A role note.** It is a short glossary for **only the roles the dashboard
  uses**, generated from `roles_json()` (the reader's view of a role: "`::BAND_LOWER`
  / `::BAND_UPPER`: the shaded uncertainty range around a line").
- **The view state.** The client sends the open tab, the current input
  values and selections (the same `vars` object as `/api/panel`, including
  what `?v.*` and a `::SELECT_AS` click set), and an optional pointed-at
  panel ("ask about this panel").
  - On a locked server these `vars` go through exactly the validation of a
    panel request: `-- @vars` allowlist, typed literals, `__claim_*` and
    `anofox_data` refused.
  - The embed link's locked variables replace them.
  - So the view state can't do anything a panel request couldn't.
- **The question**, plus earlier turns of this conversation (§12.5).

**Panel results are not sent up front.** The model asks for the panels it
needs with `get_panel`, which keeps the data sent to the provider to what the
question needs. The panels of the open tab are listed first in the catalog,
so the model usually reads one to three of them.

### 12.3 Tools

| Tool | Input | Returns | Limits |
|---|---|---|---|
| `list_panels` | `{tab?}` | the catalog entries (no data) | — |
| `get_panel` | `{panel, vars?, page?}` | the panel's rows for this viewer, with column names and roles, and the row count. `vars` may set **only variables bound by the dashboard's own input panels, and for an input with an option list only to one of its options** (M3). `@ask on` requires `-- @vars`. `page` (sort / filter / offset) applies to `::PAGED` tables. | **One panel request, built and checked exactly like `POST /api/panel`**. Each result is at most 200 rows / 16 KB of JSON to the model (it says "first 200 of 1 834 rows; sort or filter for others"). All panel data in one question is at most 64 KB. At most 8 `get_panel` calls per question. |
| `answer` | `{text, citations: [{panel, label, value?, row?: {column: value}}], scope: "answered" \| "partly" \| "not_on_dashboard"}` | — (ends the turn) | `strict: true`; `text` ≤ 1 200 characters |

**On the locked server, `get_panel` is the panel path, not a copy of it.**
The tool builds a panel request body and calls `locked_panel(st, body,
None, claims, scope, info, charge)`, the function that serves
`/api/panel` (`duckext/src/serve.rs:1888`), with **the asking viewer's
`claims` and `scope`**. Everything that protects a panel request protects
the tool:

- the row and column policies (P2);
- the claims bound after the viewer's variables;
- `@vars`;
- the `@requires` outcome;
- the read-only, locked snapshot;
- the row cap, timeout, query slots and in-flight byte budget;
- the result cache keyed by claims.

The static render server (B) calls its own panel runner with the
`@param` allowlist in the same way. A unit test asserts that the tool and
the endpoint return byte-identical results for the same viewer and request.

**Citations are checked as a triple (security review M4).** A citation
names the panel, the row (by its key columns), the column and the value. It
is *verified* only when the server finds that row in that panel's result
(read in this conversation) and the cell matches after the panel's
formatting and rounding. The right value in the wrong row is *not
verified*. Each citation carries the vars of the `get_panel` call that
produced it.

**The client builds the chip text** from the plan (the panel title) and the
verified cell. The model's `label` is shown, if at all, as plain unverified
text. A chip whose vars differ from the current view says so ("with region
= US"), and clicking it applies those vars as a visible change.

`get_panel` always builds the body with the conversation's dashboard id. A
`dashboard` field in the tool input is ignored, and a test checks this
(I3).

### 12.4 Opt-in, limits and logs

**Three switches, all off by default:**

1. **The server.** The option `ask` (`anofox_serve_dashboards(dir, port,
   '{"ask": {…}}')`, `serve --dashboards … --ask @ask.json`):

   ```json
   "ask": {
     "enabled": true,
     "model": "claude-sonnet-5",
     "key_env": "ANTHROPIC_API_KEY",          // or "key_file"; never inline
     "base_url": "https://api.anthropic.com", // or a company gateway
     "transport": "anthropic",                 // or "openai" (M3, self-hosted)
     "provider_label": "Anthropic (Claude)",   // shown in the notice
     "per_viewer": { "per_hour": 10, "per_day": 30,
                     "tokens_per_hour": 200000, "tokens_per_day": 600000 },
     "per_question_tokens": 60000,             // checked before each model turn
     "max_question_chars": 2000,               // body ≤ 16 KB
     "allow": ["group:analysts"],              // optional: who may ask
     "anonymous": false,
     "tokens_per_hour": 2000000,               // the whole server
     "concurrency": 4,
     "mask": "auto",                           // "auto" | ["col", …] | "off"
     "log_questions": false,                  // owner decision 2026-10-18
     "log_answers": false,
     "embed": false,                           // Ask inside signed embed links
     "notice": null                            // extra operator text, plain
   }
   ```

   The start **fails** in each of these cases:
   - `enabled` is set but the key is missing;
   - the model is not in the allowlist;
   - **`auth` is not configured** (security review M1). Ask requires a
     login whatever the bind, because a loopback bind behind nginx is
     still public. The only exception is an explicit
     `"ask": {"anonymous": true}`, which the start message prints as a
     warning. The static render server (B) refuses in the same way; it
     does not just warn.

   The start message names the model and the provider.
2. **The dashboard.** A header line `-- @ask on`, read by
   `parse_dashboard_meta` (`src/host/serving.rs:973`) and passed to
   `dashboard_meta`. A dashboard without it shows no Ask button, and
   `/api/ask` answers 404 for it.
3. **The panel.** `-- @ask skip` on a statement keeps that panel out of the
   catalog and out of `get_panel`, for example a detail table with names.

   The lint gets two new checks:
   - `header/bad-ask` (an unknown value);
   - `ask/raw-detail` (a warning when an `@ask on` dashboard has an
     unaggregated detail table without `@ask skip`, because its rows would be
     sent to the provider).

**Who may ask.** `POST /api/ask` sits behind the same login as `/api/panel`:
the auth proxy (P1), JWT, OIDC sessions and signed embed links (P3).

- The dashboard must be in the viewer's scope.
- Embed sessions can ask only when `ask.embed` is true, and only about the
  dashboard the link names.
- **API keys are refused** with 403. Ask is for people, and a machine caller
  would only spend the key.
- `/api/ask` takes the same same-origin `POST` rules as `/api/panel`, and the
  same `Host` allowlist.

**Limits** (amended by security review H2 and M2):
- **Ask runs on its own small worker pool, not on the HTTP workers.** An
  `/api/ask` request hands off to that pool and streams progress, so login,
  static assets, `/api/panel` and `/healthz` never wait for a model.
- **At most 1 concurrent question per viewer.**
- Ask holds **at most `max(1, max_queries / 2)` query slots** at once,
  through a sub-semaphore, so viewers always have slots.
- **One deadline per question** (60 s). The remaining time is passed to
  each `get_panel` as its timeout.
- **Token budgets per viewer** (per hour and per day), a **per-question
  token ceiling** checked before each model turn (a running total, plus an
  estimate of the next request), and a **question size cap**.
- `max_tokens` and the effort are fixed in the server config. Budget is
  **reserved before a call** and reconciled from `usage` after it, so
  concurrent questions can't overshoot.
- `ask.allow` restricts Ask to groups or claims.
- Retries on 429 / 529 count against the limits, with a bounded backoff.
- *Cancel* aborts the upstream call and releases Ask's query slots, and is
  sent by the client as `DELETE /api/ask/<conversation>`.

Also in force:
- Per viewer (the signed-in user; `auth` is required, see M1): 10 questions
  an hour and 30 a day by default.
- Per server: a token budget per hour, read from each response's `usage`.
- Per question: at most 6 model turns and the panel-data caps of §12.3, with
  a 60 s timeout.
- At most 4 concurrent questions per server.

Over a limit, the viewer gets a plain message ("You've reached today's limit
of 30 questions") with HTTP 429. Asks never take the panel query slots ahead
of viewers: they wait like any panel request.

**The log.** One request-log line per question (`--log`, same file handling)
records:
- `path: "/api/ask"`, `user`, `dashboard`, a conversation id, the turn number;
- `panels` (the indices read), `get_panel` calls, `outcome`
  (`answered` / `partly` / `not_on_dashboard` / `limit` / `error`);
- `model`, `request_id`, the input / cache read / cache write / output
  tokens, `ms`.

What is and isn't logged:
- **The question text is not logged by default** (`log_questions: false`,
  owner decision 2026-10-18). A question can itself hold personal data, and
  logging what each employee asks can count as monitoring their behaviour
  (§12.7). The operator can turn it on, for example to review abuse and
  quality, and then the viewer notice says so.
- **Always logged:** token counts, the panels used, the outcome, the model
  and `request_id`. These carry no question or data text.
- **Answer text is not logged** by default (`log_answers: false`), because
  an answer quotes data values.
- **Panel data is never logged.** Vars follow the existing `log_vars` rule.

**Ask's tool vars** are logged by **name** only, unless `log_answers` is
set, even when `log_vars` is `values` (L2).

A viewer's first use of Ask in a session writes **one audit-log event per
session** (`audit: "ask"`), so that use of the provider shows up next to
the logins.

**Retention:** Ask log lines follow the request log's rotation, which the
operator sets and must state in their records of processing.
Conversations are held in memory for at most 30 minutes after the last
question.

### 12.5 Conversations and follow-ups

The conversation lives **on the server**, in memory, keyed by the session
id, **`Claims::key()`** and a random 128-bit conversation id (L1). A claims
or group change, a policy reload or a revocation drops it. There are at most
3 conversations per user and 2 000 per server, and each holds at most
256 KB. Limits and conversations are per replica, which is documented.
Embed links that a host application mints once for many people make those
people one "viewer" (I5).
- It holds at most 10 turns and expires 30 minutes after the last question.
- It is capped in total size, and the oldest turns drop first.
- It is dropped on logout, on a snapshot refresh, on a claims change and on
  a policy reload. After a refresh the
  next question starts fresh with a note: "the data was refreshed".

The client sends only `{dashboard, conversation, question, tab, vars,
panel?}`. **It never sends earlier assistant turns or tool results**, so a
client can't forge a history in which the model "already read" data.

The builder keeps its conversation in the page (sessionStorage), as Build
does.

### 12.6 Prompt injection and output handling

Dashboard text and data values are **untrusted**. A product name can read
"ignore your instructions and…". The defence doesn't rely on the model
behaving well.

- **Nothing to abuse.** Ask's only tools read panels this viewer may read
  anyway. There is no SQL, no file, no network, no other dashboard and no
  write. The worst a successful injection can do is give **this viewer** a
  wrong or odd answer about data **they can already see**.
- **No way out.** The answer is rendered by a **dedicated
  `renderAnswer(text)`** (security review M5). It escapes everything, then
  allows only `**`, `*`, inline code and `-` / `1.` lists: no links or
  autolinks, headings, blockquotes, HTML or math. URLs and e-mail addresses
  in the text are shown greyed out (L5). The existing `renderMarkdown`
  renders links as live anchors, so it is **not** used. The result sits in a
  container that is visually separate from the chips and the notice. Mode B
  escapes on the server **after** unmasking, because restored real values
  are untrusted text too.
  - Model output never becomes HTML, a URL, an image or a style.
  - The only links are the **citation chips**, built by the client from the
    validated citations and pointing into the dashboard itself.
  - A URL in the answer text stays inert text.

  This closes the usual exfiltration channel: an image or link that sends
  data to a third party when rendered.
- **Navigation only on a click.** A citation chip switches to the panel's
  tab (`openTabNow`), scrolls to the panel and highlights the cited mark or
  row (the existing selection decoration, `decorateSelections`). The client
  checks the panel index and the cited key against its own plan and data, and
  ignores anything else. The highlight compares data values; it never builds
  a CSS selector or `innerHTML` from model-supplied keys. The model can't
  make the page do anything without the viewer's click.
- **Failure modes (L4).** If the model stops without calling `answer`
  (`end_turn` with text, `max_tokens`, a refusal, a timeout, a cancel), the
  viewer gets a fixed "I couldn't answer that" message, never the raw text.
  Upstream error bodies are never relayed to the viewer or the log: only
  the status and `request_id` are kept.
- **Labelled data.** Panel results reach the model inside
  `<panel_data panel="3">` blocks. The system prompt says that text inside
  them is data and never an instruction, and that questions about anything
  not on the dashboard get `scope: "not_on_dashboard"`.

### 12.7 Privacy and GDPR

**What is sent to the provider:**
- the panel catalog, which is the dashboard's own text, without SQL;
- the question;
- the view state;
- the results of the panels the model reads, as this viewer sees them.

Nothing about the viewer is sent: no user name, e-mail or claims, and
`metadata.user_id` is not set.

**Masking is best-effort minimisation, not a guarantee** (security review
M6). Pseudonymised data is still personal data for the controller.

What masking does:
- Tokens use a sentinel format that can't occur in data (`⟦P4⟧`). If a
  result already contains that format, the panel fails safe (it is not
  sent).
- Translation is exact-token in both directions: tokens in the answer
  become real values, and tokens in `get_panel` vars become real values
  before the query runs.
- The conversation's map is also applied to the **question**, the
  **view-state vars** and **error strings** before anything is sent.
- The map is **per conversation**, built only from this viewer's
  `get_panel` results, and dropped with the conversation. It is never
  shared or stable across viewers, so the provider can't link people
  across sessions.
- The detector covers English and German column names (`name`, `Kunde`,
  `Mitarbeiter`, `Ansprechpartner`, `Straße`, `Telefon`, `Personalnummer`…),
  spaced IBANs and IDs. It is tested with an English and a German fixture.
- An `@ask on` dashboard with a column the detector flags is a lint
  **error** (`ask/raw-detail`), unless the panel has `@ask skip` or an
  explicit `-- @ask mask col,…`.
- Quasi-identifiers (department + age + salary) are not masked. `@ask
  skip` is the tool for those panels.

With `mask: "auto"` (the default once Ask is on), the server
**pseudonymises** values before they leave:
- values that look like e-mail addresses, phone numbers or IBANs;
- the values of columns whose names suggest personal data (`name`,
  `customer`, `email`, `phone`, `address`, `employee`…);
- the columns that a P2 policy already masks stay masked.

Each value becomes a stable token for the conversation ("Person 4",
"Customer 12"). The mapping stays on the server and is put back into the
answer before the viewer sees it, so the viewer reads real names that they
could already see on the page. A list of column names masks exactly those
columns, and `"off"` turns masking off (the start message says so).

**The viewer notice.** The first time a viewer opens Ask in a session, they
see the notice, and Ask stays closed until they choose *Continue*:

> "Your question and the data of the panels on this dashboard that the
> assistant reads are sent to
> *Anthropic (Claude)* to answer it. Answers are AI-generated and can be
> wrong; check the cited panels."

The provider name comes from `provider_label`, and the operator's `notice`
text is appended as plain text. The panel then shows a permanent "AI
answers · sent to Anthropic (Claude)" line.

**GDPR note for the owner** (not legal advice; this goes into DOCS.md for
operators):
- The operator who turns Ask on is the **controller**, and the model
  provider acts as a **processor** for the panel data and questions.
- Before Ask is enabled on personal data, the operator needs a **data
  processing agreement** with the provider. *(Whether the provider's
  current commercial terms include one, and which transfer mechanism they
  use, must be checked against the provider's published terms, with the
  date, before this goes into DOCS.md. This note makes no claim about
  them.)* The operator also needs a legal basis (usually legitimate
  interest for internal reporting) and an entry in their records of
  processing.
- If the provider processes data outside the EU, the transfer needs a
  transfer mechanism. Check the provider's current terms and record the
  source and date.
- The provider's own **retention** of API inputs and outputs applies; check
  its current policy, and zero-retention arrangements where they are
  offered.
- **The viewer notice covers only the viewer.** The people whose data is
  in the panels (customers, employees) are owed Art. 13 / 14 information
  too: the operator updates their privacy notice and records of
  processing.
- **A DPIA (Art. 35)** may be needed for a new AI processing of personal
  data at scale. The operator decides with their data protection officer.
- **Employee data (Germany).** Per-user logging of question text can be a
  technical means of monitoring behaviour, so works-council
  co-determination (§87(1) Nr. 6 BetrVG) probably applies. The legal basis
  for employee data (§26 BDSG vs. Art. 6(1)(f) after CJEU C-34/21) is
  unsettled. See the open question at the top of §13.
- **When the operator turns `log_questions` on, the server adds "Your
  questions are logged by the operator" to the viewer notice.** The text is
  generated by the server, not left to the operator's `notice` text, and
  with the default (off) it is not shown.
- The EU AI Act Art. 50 transparency duty (from 2 August 2026) is
  supported by the "AI-generated" labelling; legal confirms.
- Data minimisation is built in: panels on demand, the caps, `@ask skip`,
  masking, and no identity in requests.
- Customers who can't send data out at all can use the OpenAI-compatible
  transport against a self-hosted model (M3), with the same tools and checks.

### 12.8 UX

- **Where.** Ask appears in **View mode**, in the builder and on served
  pages where Ask is enabled.
  - The dashboard header gets an *Ask* button (✦ Ask, `aria-controls`,
    `aria-expanded`, with the shortcut Ctrl/⌘ + K).
  - Each panel's toolbox gets *Ask about this panel*, which sets the
    pointed-at panel.
  - In Edit mode the assistant panel has two tabs, *Build* and *Ask*.
- **The panel** (a side sheet on desktop, a full-height sheet on a phone with
  the prompt box pinned to the bottom):
  - the notice line;
  - the conversation;
  - the prompt box, with three suggested questions generated from the
    catalog (no model call: "What does *Revenue vs plan* show?", "Which
    region is furthest behind plan?");
  - the status line ("Reading *Stock by SKU*…", *Cancel*).
- **An answer** is a short plain-language paragraph, followed by its
  **citation chips** ("*Stock by SKU* · SKU-033 · 4 days of cover"). A chip
  goes to the panel and highlights the value. Below that comes an
  "AI-generated · check the cited panels" label. *partly* and
  *not_on_dashboard* answers say so plainly ("This dashboard doesn't show
  salaries").
- **Follow-ups** continue the conversation. *New conversation* clears it.
- **Accessibility:**
  - The panel is a labelled region, and the conversation is `role="log"`,
    announcing each completed answer.
  - Citation chips are buttons with full text labels ("Go to panel *Stock by
    SKU*, SKU-033").
  - After navigation, focus moves to the panel, with its a11y summary
    announced, and *Back to the answer* returns focus to the chip.
  - Esc closes the panel and returns focus to its opener.
  - The phone layout is covered by `a11y-e2e` at 390 px, and an axe run
    covers the open panel.
- **Builder Ask (M9).** Before the first send, a per-conversation consent
  reads: "panel rows will be sent to <destination>". It uses the same
  masking module and tests as served Ask. `get_panel` binds vars as typed
  `SET VARIABLE` literals, never by textual substitution (tested), and stays
  inside the read-only gate of §3.3. Exported HTML and static View pages
  load the Ask module only when `assistant.enabled` is set.
- **The static render server (B)** (security review M8):
  - Ask responses (B and A) are `Cache-Control: no-store, private` and never
    enter the render or panel cache.
  - B's form carries a per-session CSRF token and is checked with `Origin` /
    `Sec-Fetch-Site`.
  - Ask on B is **single-turn**, or follows up through a server-held
    conversation id only. History is never carried in hidden fields.
  - B has no policies and no `@requires` hiding, so every panel must
    filter on the claims itself, which the docs state.

  The rest of this point describes B's form. B has no client data API and little
  script. It gets an *Ask* form under the dashboard: a `POST` with the
  page's `@param` values, answered by a server-rendered page with the answer
  as escaped text and citations as `#panel-n` anchors. It shows the same
  notice and has the same limits, with no JavaScript needed.

### 12.9 Evaluation

The Ask eval is in `tests/ask-eval/`, with 15 cases in M2 and 30 in M3. It
uses the example dashboards with their data: `web/examples/supply-chain.sql`,
`inventory-twin.sql`, `supplier-lead-times.sql`, `stock-outlook.sql`,
`period-kpis.sql`, `forecast-energy-load.sql` and
`examples/dashboards/sales-overview.sql`.

| Kind | Share | Example | Scored by |
|---|---|---|---|
| Fact lookup | 30 % | "Which region dropped most vs last year?" | a **truth query** stored with the case (SQL over the example data, run at eval time, so the truth follows the data); entity / number match within the panel's rounding |
| Explanation | 25 % | "Why is SKU-033 critical?" (the thresholds rule plus its days-of-cover value) | the judge with a rubric that names the facts required |
| Meaning | 15 % | "What does the red band mean?" | the judge against the panel's `::INFO` / role note |
| View-dependent | 10 % | the same question with a different region filter or tab | a truth query with those vars |
| Out of scope / refusal | 20 % | salaries that aren't on the dashboard; "show me the SQL"; a value on a hidden tab; a column masked by a policy; an instruction planted in a product name | `scope` must be `not_on_dashboard` / `partly`; nothing is revealed (deterministic check) |

**The metrics:**
- **correct**: target ≥ 85 % on facts;
- **citation accuracy**: the share of citations that pass the server check
  and point at the panel that actually holds the fact; target ≥ 95 %;
- **refusal accuracy** on out-of-scope cases: 100 %;
- cost, turns and seconds per question.

**Deterministic security tests** run in normal CI, with no live calls and a
fake model upstream that records every request:
1. **Canary rows.** A policy fixture (two users, a row filter, a column
   mask) seeds canary values that only user B may see. The fake model, played
   by a script, calls `get_panel` for every panel with every var it can try,
   as user A. CI asserts that **no canary value appears in any request body
   sent upstream** and none in A's answers.
2. `@ask skip` panels and hidden-tab panels never appear in a request.
3. `/api/ask` answers 404 without `ask.enabled`, without `-- @ask on`, and on
   every authoring route. API keys get 403, and embed sessions get 403 unless
   `ask.embed` is set.
4. Forged `vars` (`__claim_*`, `anofox_data`, names outside `@vars`) in
   `/api/ask` and inside a `get_panel` call are refused like in `/api/panel`.
5. A model reply with a URL, an image, HTML or a citation to a panel it
   never read is rendered inert, or marked *not verified*.
6. The masking round trip, narrowed by M6: values the detector flags never
   reach the upstream, and a pseudonym never reaches the viewer. Tested
   with an English and a German fixture; a sentinel collision fails safe.
7. **Claim-dependent text (H1):** a `::MARKDOWN` / `::CALLOUT` / `::LABEL`
   whose value depends on `getvariable('__claim_user')`. User B's canary
   never reaches a request made for A, and the prefix bytes are identical
   for A and B.
8. **Load (H2):** 4 concurrent asks against a slow fake model, while the p95
   of `/api/panel` and `/` stays within bounds.
9. **Citations (M4):** the right value in the wrong row is *not verified*;
   chip text never contains model text.
10. **Rendering (M5):** an XSS and markup corpus through `renderAnswer`
    (`javascript:`, `data:`, `<img onerror>`, nested Markdown, `\href`, a
    fake chip made of text and glyphs).
11. **The upstream allowlist (M7):** unknown body fields and
    `anthropic-beta` headers are refused or stripped.

The live eval runs like the Build eval (§6). At about $0.05 a question it
costs about $1 for 15 cases, and it shares the workflow and the key of §11.5.

### 12.10 Cost per question

The assumptions, for Sonnet 5 at effort `medium`:
- a cached prefix of about 6 K tokens (instructions, the role note, the
  catalog);
- one or two `get_panel` calls, about 8 K tokens of panel data;
- two model calls;
- about 1.5 K output tokens, including thinking.

Worst case, bounded by the new caps (security review M2): 6 turns with
about 20 K tokens of panel data each, plus history, stays under the
`per_question_tokens` ceiling of 60 K input-equivalent tokens, so at most
about $0.15–0.20 a question. The typical case is about **$0.04–0.06 per
question**, and about $0.02 for a follow-up
within 5 minutes. At the default limit of 30 questions per viewer per day
that is at most about $1.50 per viewer per day. The server's
`tokens_per_hour` caps the whole bill.

The catalog prefix is shared by every viewer of a dashboard, and viewer data
always comes after the last shared cache breakpoint. Claude Haiku 4.5 is
evaluated in M3 as a cheaper option. It needs a different thinking
configuration, so it is not offered in M2.

### 12.11 Threat model (Ask)

| Threat | Mitigation |
|---|---|
| The model key leaks to a browser | The key is only in the server's env / key file; it is never in a response, page, error or log (unit-tested); the browser has no transport for served Ask |
| A viewer gets data beyond their rights through Ask | `get_panel` *is* `locked_panel` with the viewer's claims and scope (P2 policies, claims after vars, `@vars`, hidden tabs, scopes); the canary test (§12.9) |
| SQL from the client or the model | Neither exists: the client sends a question and view state, and the model calls `get_panel` with a panel index, vars and a page, all validated like `/api/panel` |
| A forged conversation history | The history is kept on the server only, keyed by user + page session + 128-bit id |
| Prompt injection through data or dashboard text | No side-effect tools; answers are escaped text without links or images; navigation only through validated citation chips the viewer clicks |
| Exfiltration to a third party | `renderAnswer` renders no links, images or HTML from model output (M5); the upstream request allowlist (M7); the server makes no outbound calls except to the configured model endpoint |
| One viewer's data in another's answer | Conversations are per user + session; the panel cache is keyed by claims (unchanged); viewer data never sits in the shared prompt-cache prefix |
| Spending the operator's key (abuse, a scripted client) | Login always required (start check, M1); per-viewer question and token budgets, a per-question ceiling, `ask.allow` (M2); API keys refused; its own worker pool, 1 question per viewer, and a slot sub-limit (H2) |
| Personal data sent to the provider | Panels on demand, the caps, `@ask skip`, `mask: "auto"`, no identity in requests, the viewer notice |
| Personal data in logs | No panel data in logs; answer text off by default; question text off by default (the operator can switch it on, and the notice then says so); token counts and panels used always logged; `log_vars` rules unchanged |
| The viewer trusts a wrong answer | "AI-generated" labelling, checked citations that open the panel, `scope: "partly"`, the eval |
| Ask switched on by accident | Off at three levels (server, dashboard, panel); the start message names the model and provider |

### 12.12 Security review

Both kinds of review are **gates**. An implementation PR doesn't start
before the design review, and a serving PR doesn't merge before its review's
blocking findings are fixed.

- **Before implementation.** An independent review of this section (§12)
  and of §2.3 against `docs/secure-serving.md`, by a reviewer who did not
  write it. Its findings change this note before M2-3 starts.
- **For each serving PR** (M2-1 `/assist`, M2-3 the Ask core, M2-4 `/api/ask`
  on the locked server, M2-6 Ask on the static render server). An
  independent security review of the diff, with the findings fixed in the
  same PR, as for #160. The threat-model rows above go into
  `docs/secure-serving.md` in M2-4.

### 12.14 Security review changes (2026-10-18)

The independent design review (M2-0, verdict *approve with required
changes*: 2 High, 9 Medium) is folded into the sections above. This table is
the index, and each change has a test in §12.9 or in the PR that builds it.

| Finding | Change | Where |
|---|---|---|
| **H1** The catalog held role values, which are query results, in a prefix described as shared | The catalog is split. Only static source text is in the shared prefix. Role values (`::TITLE`, `::LABEL`, `::INFO`, `::THRESHOLDS`, `::MARKDOWN`, `::CALLOUT`, `::ASOF`) come only through `get_panel`. Exclusions are decided per request, in the per-viewer part. Tested: identical prefix bytes across policy users, and a claim-dependent `::MARKDOWN` canary | §12.2, §12.9 test 7 |
| **H2** Ask held HTTP workers and query slots, so one viewer could stall the server | Ask runs on its own pool; 1 concurrent question per viewer; Ask holds at most `max(1, max_queries/2)` slots; one deadline per question, passed down to `get_panel`; a load test | §12.4, §12.9 test 8 |
| **M1** The loopback exemption missed a server behind a reverse proxy | Ask requires `auth` whatever the bind (except an explicit, warned `anonymous: true`); B refuses rather than warns | §12.4 |
| **M2** One viewer could use up the shared budget | Per-viewer token budgets, a per-question token ceiling, a question size cap, server-fixed `max_tokens` and effort, budget reserved before each call, `ask.allow`; worst-case cost stated | §12.4, §12.10 |
| **M3** Model-chosen vars were "anything the endpoint accepts" | `@ask on` requires `-- @vars`; Ask may set only variables bound to the dashboard's input panels, and only to their options | §12.3, lint |
| **M4** Citation checks were too weak; the chip text was model-written | Verified as panel + row + column + value; chip text built by the client; citations carry their vars | §12.3, §12.9 test 9 |
| **M5** `renderMarkdown` renders live links | A dedicated `renderAnswer` (no links, headings, blockquotes, HTML or math), with an XSS corpus test | §12.6, §12.9 test 10 |
| **M6** Masking promised more than a heuristic can | Described as best effort; sentinel tokens; exact-token translation both ways; the question, vars and errors masked; per-conversation maps; an English and a German fixture; a lint error for flagged columns | §12.7, §12.9 test 6 |
| **M7** `/assist` used a denylist | An allowlist of body fields, tool fields and headers; no `anthropic-beta` from the client; the same list for Ask's upstream calls | §2.3, §12.9 test 11 |
| **M8** B: CSRF, caching, history | `no-store, private`; a CSRF token plus Origin / Sec-Fetch-Site; single-turn or a server-held id; no `@requires` hiding stated | §12.8 |
| **M9** Builder Ask sent rows without consent | Per-conversation consent; the shared masking module; typed `SET VARIABLE` binding; the module loaded only when enabled | §12.8 |
| **L1** Conversation binding | Keyed on the session + `Claims::key()`; dropped on claims or policy change; numeric caps; per replica | §12.5 |
| **L2** Logging | Ask tool vars logged by name; one audit event per session | §12.4 |
| **L3** Upstream hygiene | `base_url` must be https unless loopback; no redirects; `HTTPS_PROXY` only when configured (documented: it sees the key); `key_file` opened with the `revocation_file` rules (0600, no symlink); a generic User-Agent; no identity in headers or metadata; upstream error bodies never relayed | M2-1 / M2-3 implementation |
| **L4** Failure modes | A fixed "couldn't answer" message; retries count against the limits; cancel through `DELETE /api/ask/<id>` aborts upstream and frees slots | §12.4, §12.6 |
| **L5** Social engineering in the answer text | URLs and e-mail addresses greyed out | §12.6 |
| **L6** Notice wording | "the data of panels on this dashboard that the assistant reads" | §12.7 |
| **I1–I6** | No per-panel scopes (I1); Ask checks its own exclusion list before `locked_panel` (I2); the dashboard id is fixed by the conversation (I3); with H1 there is no data in shared prefixes, and operators with several tenants on one key should use one workspace per tenant (I4); embed links minted once for many people share one viewer (I5); JWT callers are covered by M2's budgets (I6) | §12.2, §12.3, §12.5 |
| **GDPR** | Art. 13 / 14 duties towards the people in the panels; a DPIA note; pseudonymised data is still personal data; DPA and transfer statements removed until checked against the provider's dated terms; works council and §26 BDSG; `log_questions` off by default, with a server-generated "questions are logged" notice when it is on; retention stated; AI Act Art. 50 | §12.7, §13 |

**Implementation of M2-3 and later starts only now that these are in the
note.** Each serving PR still gets its own review (§12.12).

### 12.15 Delta review (2026-10-30)

The design review (M2-0, §12.14) predates several locked-server features.
This delta review checks §12 and §2.3 against `docs/secure-serving.md` as of
2026-10-30. It was done by the implementing session (the owner's rule since
2026-10-28: no sub-agents), not by an independent reviewer — each serving PR
still gets its own review (§12.12). Verdict: **the design holds, with two
required changes before M2-3 (D1, D2)**; the rest go into the PR that builds
the part they touch.

| # | Feature since 10-18 | Finding | Required change | When |
|---|---|---|---|---|
| **D1** (Medium) | Per-viewer state, `viewer_state` (#517) | A panel can read recorded answers: the viewer's own, or every viewer's on a trainer's panel. Ask's `get_panel` would send them to the model provider — other people's answers, personal data under §12.7. | A statement that reads `viewer_state` / `viewer_state_latest` is on Ask's exclusion list (I2) by default; `-- @ask include` on that statement opts it in, and the lint flags a trainer (all-rows) panel opted in. Masking (§12.7) applies to it like any panel. | before M2-3 — done: `src/sql/ask.rs` (`exclusions`, also D3's write-back statements), lint `ask/*` (#745) |
| **D2** (Medium) | `-- @materialize` (#727) and warm connections (#388) | An Ask question runs panels like a viewer does, so it can build materialized results (shared across logins, LRU 16 per snapshot) and evict others' — a burst of questions with many variable values degrades every viewer, and the build time can exceed the per-question deadline. | Ask's `get_panel` counts a materialized build against the question's own slot budget and deadline (H2); the variables Ask may set stay limited to the input panels' options (M3), which bounds the builds per dashboard; the load test (§12.9 test 8) includes a `@materialize` dashboard. | before M2-3 |
| D3 (Low) | Shared write-back (#684, #726, #736) | Saved tables are read like other tables (a policy's rules apply to the ones it names). Ask's tools are read-only, but `::SAVE` / `::FORM` statements must not be callable through `get_panel` as if they were panels, and the save endpoint's 409 body (names of writers) never reaches Ask. | `::SAVE`, `::SUBMIT` and `::FORM` statements are on the exclusion list; no tool posts to `/api/save` or `/api/submit` (already true: no side-effect tools) — a test asserts the tool list. | M2-3 |
| D4 (Low) | `::SHOW_IF`, `::GUIDE_STEP` / `::GATE` (#515, #516) | These hide panels but are not access control (docs say so); Ask may read a panel the page doesn't show right now. | Citation chips to a panel hidden by a condition say so ("not shown right now"); the notice (L6) wording covers "panels on this dashboard", not "panels on screen". | M2-4 |
| D5 (Low) | Nav pickers, `path_vars` (#556) | A variable Ask sets that is a `path_vars` variable reaches file paths. | Ask may set a `path_vars` variable only to a value its nav picker offers, through the same `valid_path_value` check a request gets. | M2-4 |
| D6 (Low) | `::IMAGE` / Markdown images, `/d/<id>/assets` (#514) | `get_panel` on an image panel would hand the model asset paths (internal file names). | Image panels' URLs are not part of a panel's rows for Ask; the alt text is. | M2-3 |
| D7 (Info) | Snapshot refresh, Quack source (#352) | A conversation can span a refresh: earlier answers cite data that changed. | Citations are verified against the snapshot of the answer they belong to; a chip opened after a refresh says the data was refreshed. | M2-5 |

### 12.13 The follow-up Ask must not block: a dashboard published as an agent (#194)

In #194 a dashboard is published as a guarded agent: the locked server, plus
Ask, plus declared questions, with chat, Slack and Teams as channels. Ask is
built so that #194 adds to it and changes nothing in it:

- **One runtime.** `src/host/ask.rs` is a library: `ask(conversation,
  question, view, viewer) → answer`. It has no HTTP in it, and panel access
  goes through a `PanelRunner` trait, implemented for A, for B and for tests.
  `/api/ask` is only the first channel; a Slack or Teams adapter is another
  caller of the same function, with the same policies, claims, limits and
  log.
- **Panels are already named, reviewed queries.** `get_panel` runs a
  dashboard statement with validated parameters and never model-written SQL.
  That is the "named queries" contract of #194. Its tool description is
  generated from the catalog (title, `::INFO`, `@description`), so
  `capabilities.md` can be generated from the same source.
- **There is room for a guard.** `ask.mode` defaults to `"open"`, which
  means any question about the dashboard. #194 adds `"declared"`: only
  questions that match a `-- @question` line are answered, and their slots
  are bound to the declared input. The answer schema keeps the `scope` field,
  so a refusal outside the declarations is just another `not_on_dashboard`.
  We reserve `-- @question` now (the lint knows the name and ignores it), so
  dashboards written today stay valid.
- **The answers can be checked.** The Ask eval's truth queries (§12.9) are
  the same idea as #194's "expected answers computed by the panel SQL". The
  eval runner takes its cases from a file, so `@question` lines can generate
  cases later.
- **Provenance.** The catalog already carries `::ASOF`, `@owner` and
  `@certified`, so #194's provenance footer can be built from data the Ask
  runtime already holds.

Not in this plan: the canonical schema and its per-customer mapping, the
release bundle, and the channels. They are #194's design note
(`docs/plans/plan-dashboard-to-agent.md`).

## 13. Open questions for the owner (Ask)

1. **Logging question text.** *(Decided; see §13.1.)* Originally proposed:
   questions logged by default (`log_questions: true`), answers not. A question can itself hold
   personal data ("why was Mrs. Weber's claim rejected?").
   *Recommendation:* keep this default for audit and quality review, document
   it in the viewer notice ("questions are logged by the operator"), and let
   operators turn it off.
2. **Ask inside signed embed links.** *Recommendation:* off by default
   (`ask.embed: false`), because the host application, not the viewer, chose
   to show the dashboard and may not have told its users about a model
   provider. The host application opts in. API keys never get Ask.
3. **The static render server (B) in M2.** Is "locked server" A only, or A
   and B? *Recommendation:* keep B in M2 as planned (about 1.5 days, no
   script, the same core), but make it the first item to move to M3 if the
   security reviews take longer.
4. **Masking on by default.** *Recommendation:* `mask: "auto"` whenever Ask
   is enabled. Viewers still read real values, because the server reverses
   the pseudonyms. It costs a little accuracy on questions about a specific
   person, which an operator can accept by listing the columns or turning
   masking off.
5. **Default limits and model for served Ask.** *Recommendation:* Sonnet 5 at
   effort `medium`, 10 questions an hour and 30 a day per viewer, 2 M tokens
   an hour per server (about $4–5 an hour at most). Revisit Haiku 4.5 in M3
   after the eval. Before enabling Ask on customer data, the operator needs a
   DPA with the provider (§12.7).

### 13.1 Decisions (2026-10-18)

The owner said "go with recommendation", so each open question takes its
recommendation:

1. **Question text is NOT logged by default for served Ask**
   (`log_questions: false`). This replaces the earlier default of `true`.
   The owner decided it after the security review raised works-council
   co-determination (§87(1) Nr. 6 BetrVG).
   - The operator can switch logging on. The server then adds "Your
     questions are logged by the operator" to the viewer notice.
   - Token counts, the panels used and the outcome are always logged.
     Answers are not logged by default, and data values never are.
   - Builder Ask is unchanged: it logs nothing on a server beyond the
     `/assist` token line (§2.3).
2. **Embed links:** Ask is off unless the host application opts in
   (`ask.embed: true`). API keys never get Ask.
3. **The static render server (B)** stays in M2 (M2-6). It is the first item
   to move to M3 if the security reviews take longer.
4. **Masking:** `mask: "auto"` is on whenever Ask is enabled.
5. **Defaults for served Ask:**
   - Sonnet 5 at effort `medium`;
   - 10 questions an hour and 30 a day per viewer;
   - 2 M tokens an hour per server;
   - Haiku 4.5 evaluated in M3.

   A data processing agreement with the provider is required before Ask runs
   on customer data.

**Implementation stays gated** on the independent security review of the
design (M2-0, §12.12), which is running now. Its required changes will be
folded into this note in a follow-up amendment before M2-3 starts.
