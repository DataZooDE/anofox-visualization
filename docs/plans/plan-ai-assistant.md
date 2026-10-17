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
| CSP | None in `index.html` today. If one is added later, `connect-src` must allow the model endpoint. | `web/index.html` |
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

  Everything else is passed through unchanged, so prompt caching keeps
  working.
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
| **M1-3. Panel and new-from-question** | the panel, *What will be sent*, preview, diff, apply / undo (native + history + toast), the key dialog, the *New dashboard* entry, phone sheet, a11y; `assistant` block in `app-config.json`; 10-prompt eval seed + `scripts/assist-eval.mjs` (live, manual) | `web/assist-ui.js` (new module, not in the 12.7 K-line `app.js`), `web/app.js` (hooks), `web/index.html`, `web/app-config.example.json`, `tests/assist-eval/` | `scripts/assist-e2e.mjs` (Playwright with a mocked Messages endpoint: prompt → preview → apply → Ctrl+Z → undo toast; the write-confirmation dialog; nothing sent before *Send*); `a11y-e2e` + phone; a test that the module is never loaded with `window.__served` or `?embed=1` | 2 d |
| **M2-0. Security review of the design (a gate, not a PR)** | an independent review of §12 and §2.3 against `docs/secure-serving.md`; its findings are folded into this note before M2-3 starts (§12.12) | this note | — | 0.5 d |
| **M2-1. `/assist` on serve** | `src/host/assist.rs` (validation, key, streaming relay, budget, log); routes in both authoring servers; `--assist-*` flags and `anofox_serve` options; transport (b) | `src/host/assist.rs`, `src/bin/serve.rs`, `duckext/src/serve.rs`, `docs/DOCS.md`, the serving skill | Rust: the guard is applied (no token → 403, foreign Origin → 403), the model allowlist, `max_tokens` cap, server tools refused, the key never in a response or log, budget → 429, SSE relayed against a local fake upstream; **locked server: `/assist/*` → 404** (both servers) | 2 d + review |
| **M2-2. Edits and Explore** | *Change this panel* from the toolbox; follow-ups in one conversation; `edits: [{stmt, sql}]` for long scripts; the stale-proposal check; *Build with the assistant* in Explore | `web/assist.js`, `web/assist-ui.js`, `web/app.js` (toolbox, explore) | node tests: edit merging and statement numbering; e2e: change panel 2 to a table; Explore entry with the scope | 2 d |
| **M2-3. Ask core (server)** | `src/host/ask.rs`: the server-side loop over the Messages API (raw HTTPS through the existing `ureq`, because there is no official Rust SDK; non-streaming model calls, with progress sent to the client as server-sent events), the `list_panels` / `get_panel` / `answer` tools behind a panel-runner trait, the catalog builder, the citation check, masking and its reverse, the conversation store, the limits, the log lines; `-- @ask on` / `-- @ask skip` in `parse_dashboard_meta` and `dashboard_meta`; the lint checks `header/bad-ask` and `ask/raw-detail`; `web/assist/ask-instructions.md` (shared with the builder) | `src/host/ask.rs`, `src/host/serving.rs`, `src/lint.rs`, `src/wasm.rs`, `web/assist/ask-instructions.md` | Rust with a fake upstream: the loop, the turn and size caps, citation pass / fail, the masking round trip, history never taken from the client, the limits → 429, the key never in output or logs | 3 d + review |
| **M2-4. Ask on the locked server (A)** | `POST /api/ask` (+ SSE progress) in `duckext/src/serve.rs`; `get_panel` → `locked_panel` with the viewer's claims and scope; the `ask` options and their start checks; API keys refused, `ask.embed`; `web/ask-ui.js` (the View-mode panel, the notice, citation chips → `openTabNow` + highlight, suggested questions, phone, a11y), loaded only when the page says Ask is on; `docs/secure-serving.md` (Ask section + threat model) | `duckext/src/serve.rs`, `web/ask-ui.js`, `web/app.js` (hooks), `web/index.html`, `docs/secure-serving.md`, `docs/DOCS.md`, the serving and access-control skills | the deterministic security tests of §12.9 (canary rows under a policy with two users, `@ask skip`, hidden tabs, 404 / 403 matrix, forged vars, inert output), `get_panel` = `/api/panel` byte-identical, `served-e2e` with a fake upstream: ask → answer → chip → tab + highlight; `a11y-e2e` + phone | 3 d + review |
| **M2-5. Ask in the builder** | the Ask tool set in `web/assist.js` over the builder's panel results (a re-run of a panel with other input values for `get_panel`), the same `ask-ui.js`, *Build* / *Ask* tabs in Edit mode, *What will be sent* lists the panels to be sent | `web/assist.js`, `web/ask-ui.js`, `web/assist-ui.js` | node tests with a fake transport (the same cases as the server tests where they apply); e2e in View mode | 1.5 d |
| **M2-6. Ask on the static render server (B)** | the `Ask` form and the server-rendered answer page in `src/bin/serve.rs`, `get_panel` over its panel runner with the `@param` allowlist, the same options and limits | `src/bin/serve.rs` | Rust: the 404 / 403 matrix, `@param` validation in `get_panel`, the escaped answer page (an XSS case list as in `tests/xss.rs`), the canary test against B | 1.5 d + review |
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
| A browser key is stolen (XSS, CDN, the shared `github.io` origin) | `sessionStorage` by default, the spend-limit advice, a custom demo domain (Q1), the key never in the SQL, links or logs; (b) is the default wherever it's possible |
| Personal data in profiles | top values only for low-cardinality, non-PII-looking columns, the preview, the installation switch |
| Prompt injection through data | data in a labelled block; read-only tools; the write gate; nothing applied without the user |
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

- **The panel catalog.** Built on the server from the plan it already holds
  for each dashboard. Per statement:
  - its index, its tab, and its kind (KPI, line, bar, table, input…);
  - its title and `::LABEL`;
  - each column's display name and role;
  - its `::INFO` definitions;
  - for a `::THRESHOLDS` panel, the rule text;
  - the band pairs (`::BAND_LOWER` / `::BAND_UPPER`, with their level);
  - its `::MARKDOWN` / `::CALLOUT` text, which is where a dashboard states
    its takeaway.

  It also holds `-- @title` / `@description` and the `::ASOF` freshness of
  the dashboard.

  Left out of the catalog:
  - **panels on tabs hidden by an unmet `@requires`**;
  - **panels marked `-- @ask skip`** (§12.4);
  - for a viewer with a scope, panels the scope doesn't allow.

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
| `get_panel` | `{panel, vars?, page?}` | the panel's rows for this viewer, with column names and roles, and the row count. `vars` may change **input values the viewer could set** (e.g. "and for region US?"). `page` (sort / filter / offset) applies to `::PAGED` tables. | **One panel request, built and checked exactly like `POST /api/panel`**. Each result is at most 200 rows / 16 KB of JSON to the model (it says "first 200 of 1 834 rows; sort or filter for others"). All panel data in one question is at most 64 KB. At most 8 `get_panel` calls per question. |
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

**Citations are checked before they are shown.** The server keeps every
`get_panel` result of the conversation. A citation must name a panel that was
read in this conversation. A cited `value` must occur in that result, with
numbers compared after the panel's own formatting and rounding. A citation
that fails is shown as *not verified* with a warning mark, and the miss is
counted in the eval. Numbers inside the free text are not checked one by one;
the eval measures them (§12.8).

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
     "per_viewer": { "per_hour": 10, "per_day": 30 },
     "tokens_per_hour": 2000000,               // the whole server
     "concurrency": 4,
     "mask": "auto",                           // "auto" | ["col", …] | "off"
     "log_questions": true,
     "log_answers": false,
     "embed": false,                           // Ask inside signed embed links
     "notice": null                            // extra operator text, plain
   }
   ```

   The start **fails** in each of these cases:
   - `enabled` is set but the key is missing;
   - the model is not in the allowlist;
   - the `bind` goes beyond loopback without `auth`, so an anonymous network
     could spend the key.

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

**Limits:**
- Per viewer (the signed-in user, or the peer address without `auth`):
  10 questions an hour and 30 a day by default.
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
- **The question text** is logged by default (`log_questions`). It is what
  the viewer typed, and an operator needs it to review abuse and quality.
- **Answer text is not logged** by default (`log_answers: false`), because
  an answer quotes data values.
- **Panel data is never logged.** Vars follow the existing `log_vars` rule.

Each question also gets one audit-log event (`audit: "ask"`) the first time
a viewer uses Ask in a session, so that use of the provider shows up next to
the logins.

### 12.5 Conversations and follow-ups

The conversation lives **on the server**, in memory, keyed by user, page
session and a random 128-bit conversation id.
- It holds at most 10 turns and expires 30 minutes after the last question.
- It is capped in total size, and the oldest turns drop first.
- It is dropped on logout and on a snapshot refresh. After a refresh the
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
- **No way out.** The answer is rendered as **plain text with escaped
  Markdown**: bold, italics, lists and inline code only, through the existing
  sanitising `renderMarkdown` path with links and images turned off.
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
  ignores anything else. The model can't make the page do anything without
  the viewer's click.
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

**Masking.** With `mask: "auto"` (the default once Ask is on), the server
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

> "Your question and the data of the panels you can see are sent to
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
  processing agreement** with the provider. Anthropic's commercial terms
  include one. The operator also needs a legal basis (usually legitimate
  interest for internal reporting) and an entry in their records of
  processing.
- If the provider processes data outside the EU, the transfer needs a
  transfer mechanism (SCCs or the EU–US Data Privacy Framework); check the
  provider's current terms.
- The provider's own **retention** of API inputs and outputs applies; check
  its current policy, and zero-retention arrangements where they are
  offered.
- The viewer notice above covers transparency to the viewer.
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
- **The static render server (B)** has no client data API and little
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
6. The masking round trip: a pseudonym never reaches the viewer, and a real
   value never reaches the upstream.

The live eval runs like the Build eval (§6). At about $0.05 a question it
costs about $1 for 15 cases, and it shares the workflow and the key of §11.5.

### 12.10 Cost per question

The assumptions, for Sonnet 5 at effort `medium`:
- a cached prefix of about 6 K tokens (instructions, the role note, the
  catalog);
- one or two `get_panel` calls, about 8 K tokens of panel data;
- two model calls;
- about 1.5 K output tokens, including thinking.

That is about **$0.04–0.06 per question**, and about $0.02 for a follow-up
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
| Exfiltration to a third party | No URLs, images or HTML are rendered from model output; the server makes no outbound calls except to the configured model endpoint |
| One viewer's data in another's answer | Conversations are per user + session; the panel cache is keyed by claims (unchanged); viewer data never sits in the shared prompt-cache prefix |
| Spending the operator's key (abuse, a scripted client) | Login required beyond loopback (start check); per-viewer and per-server limits; API keys refused; concurrency cap; Ask waits for query slots like a panel request |
| Personal data sent to the provider | Panels on demand, the caps, `@ask skip`, `mask: "auto"`, no identity in requests, the viewer notice |
| Personal data in logs | No panel data in logs; answer text off by default; question text on by default but switchable; `log_vars` rules unchanged |
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

1. **Logging question text.** Questions are logged by default
   (`log_questions: true`) and answers are not. A question can itself hold
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

1. **Question text is logged by default** (`log_questions: true`). The viewer
   notice says that questions are logged by the operator, and operators can
   turn logging off. Answers are not logged by default, and data values never
   are.
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
