# Plan: an AI assistant in the builder — a dashboard from a plain-language request

Status: **design note for owner review (2026-10-17)**, based on master `ba2eb9d`.
No product code yet. Issue: #145. Related: Explore data and the starter
dashboards (#97, #110, #127), the app bar config (#146), My dashboards
([plan-sidebar.md](plan-sidebar.md), #148), the Claude Code plugin skills.

**Goal.** In Edit mode a user types what they want to see, e.g. *"weekly
revenue by region against last year, and which products are behind plan"*.
An assistant writes the annotated SQL (`::ROLE` casts), checks that it runs
and lints clean, and offers it as a change the user applies or discards. The
user refines it in conversation (*"make the second chart a table"*). The
result is plain SQL in the editor, saved and shared like a handwritten
dashboard. It is never available on the locked server, in embed links or in
View mode.

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

## 8. Delivery: M1–M3, 8 PRs, about 14 days

The issue's M1 is front-heavy, and it has no way to know whether the result is
good. This cut moves a **10-prompt eval seed into M1**, so the prompt is tuned
against numbers from the first day, and it ships the lint export first because
it is useful without any AI.

| PR | Content | Files | Tests | Effort |
|---|---|---|---|---|
| **M1-1. Lint in the browser** | `lint_step` wasm export (§3.4); Edit-mode *Check* button listing diagnostics with statement links | `src/wasm.rs`, `src/lint.rs` (a small replay adapter), `web/app.js`, `web/index.html` | Rust: `lint_step` = native lint on all `web/examples/*.sql` and the explore starters; node test of the JS fixed-point loop; e2e: *Check* on a broken example | 1.5 d |
| **M1-2. Assistant core** | `web/assist.js`: transport interface, (a) browser-key via the SDK, the loop, the tools, the statement classifier and write gate, the budget and cancel; `web/assist/instructions.md`; `scripts/gen-assist-guide.mjs` + markers in the dashboards skill; the role reference from `roles_json()` | `web/assist.js`, `web/assist/*`, `scripts/gen-assist-guide.mjs`, the dashboards `SKILL.md` (markers only), `tests/web_js.rs` | `tests/web/assist_test.mjs`: the loop with a fake transport (self-correction, the round limit, cancel, refusal, `max_tokens`), the classifier table (≥ 60 cases), prefix stability hash; CI check that `guide.md` is current | 2.5 d |
| **M1-3. Panel and new-from-question** | the panel, *What will be sent*, preview, diff, apply / undo (native + history + toast), the key dialog, the *New dashboard* entry, phone sheet, a11y; `assistant` block in `app-config.json`; 10-prompt eval seed + `scripts/assist-eval.mjs` (live, manual) | `web/assist-ui.js` (new module, not in the 12.7 K-line `app.js`), `web/app.js` (hooks), `web/index.html`, `web/app-config.example.json`, `tests/assist-eval/` | `scripts/assist-e2e.mjs` (Playwright with a mocked Messages endpoint: prompt → preview → apply → Ctrl+Z → undo toast; the write-confirmation dialog; nothing sent before *Send*); `a11y-e2e` + phone; a test that the module is never loaded with `window.__served` or `?embed=1` | 2 d |
| **M2-1. `/assist` on serve** | `src/host/assist.rs` (validation, key, streaming relay, budget, log); routes in both authoring servers; `--assist-*` flags and `anofox_serve` options; transport (b) | `src/host/assist.rs`, `src/bin/serve.rs`, `duckext/src/serve.rs`, `docs/DOCS.md`, the serving skill | Rust: the guard is applied (no token → 403, foreign Origin → 403), the model allowlist, `max_tokens` cap, server tools refused, the key never in a response or log, budget → 429, SSE relayed against a local fake upstream; **locked server: `/assist/*` → 404** (both servers) | 2 d |
| **M2-2. Edits and Explore** | *Change this panel* from the toolbox; follow-ups in one conversation; `edits: [{stmt, sql}]` for long scripts; the stale-proposal check; *Build with the assistant* in Explore | `web/assist.js`, `web/assist-ui.js`, `web/app.js` (toolbox, explore) | node tests: edit merging and statement numbering; e2e: change panel 2 to a table; Explore entry with the scope | 2 d |
| **M3-1. Local models** | transport (c): the OpenAI-compatible translator; `--assist-provider openai` on the proxy as well | `web/assist.js`, `src/host/assist.rs` | node test: the translation both ways (tools, tool calls, errors); replay of three eval transcripts through (c) | 1.5 d |
| **M3-2. Sample rows** | the opt-in per conversation, the preview of the rows, `sample_rows_allowed`, rows in `run_sql` with the opt-in | `web/assist.js`, `web/assist-ui.js` | node test: no row values in any request without the opt-in (scan of the serialised requests for every eval dataset); e2e of the preview | 0.5 d |
| **M3-3. The full eval** | 30 prompts, the judge with calibration, recheck + replay + freshness in CI, the live `workflow_dispatch`, first stored results | `tests/assist-eval/*`, `scripts/assist-eval.mjs`, `.github/workflows/assist-eval.yml`, `.github/workflows/ci.yml` | the eval itself; acceptance ≥ 80 % first-try runs, 100 % safety | 2 d |

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
