# Plan: scenarios with anofox-scenario

Status: **design note, waiting for the owner's decisions (§9)**, based on
master `a49fd18`. No product code yet. Refs #1243.

**Goal.** A planner opens a dashboard, starts a scenario ("demand +10 %",
"second shift in March") from the current plan, changes it in a form or an
editable table, compares it with the plan and with other scenarios, and an
approver freezes it or makes it the official plan. Every step is recorded.
The scenarios are stored by
[anofox-scenario](https://github.com/DataZooDE/anofox-scenario): Git-like,
copy-on-write branches of the database (delta tables inside the host
database file), read with `ATTACH 'name' AS s (TYPE scenario)`, compared
with `scenario_diff`, approved with `scenario_freeze` / `scenario_merge`.

**What exists today (level 1).** Parameter scenarios as rows:
- a `::FORM`, a name field, and `::SAVE` / `::SAVE_FORM` upsert one row per
  scenario into a write-back table (`options.writeback`, #684);
- panels compare the rows (one fit per row, e.g. a grouped model fit).

That is enough when a scenario is **a few numbers**. This plan is for
scenarios that change **data** (plans, capacities, prices, BOMs) and have a
lifecycle.

## 1. The decisions this note proposes

1. **The locked server owns every scenario operation.**
   - A request never runs `ATTACH`, `scenario_*` or a write as SQL.
   - Rust runs a fixed set of operations (create, write, freeze, merge,
     drop) on a **server-owned writable connection** to the live database,
     with names and values bound, never spliced into SQL text.
   - The operations are declared in `options.scenarios`, as write-back is
     in `options.writeback`.
2. **Reads come from the snapshot, as today.** After an operation the
   server refreshes its read-only snapshot, so panels stay on the locked,
   read-only path.
   - **Refresh:** the existing `refresh` machinery (epoch++) reloads
     changed databases.
   - **Branch catalogs:** each listed scenario is attached in the snapshot
     instance, read-only, under a reserved name (`scn_<id>`), before the
     configuration is locked.
3. **A picker chooses the branch the panels read.** It is a new input,
   `::SCENARIO`.
   - Its value is a scenario name, or `main`.
   - The server sets that request's `search_path` to `scn_<id>` (or the
     base) before the panels run. Unqualified names (`FROM plan`) then read
     the branch, and the dashboard's SQL doesn't change.
   - Panels that compare scenarios name them explicitly, through a server
     helper (§4).
4. **Writes reuse the `::SAVE` vocabulary.** A save gets a target scenario:
   `::SAVE_SCENARIO` names the variable holding it (default: the picker's).
   - Form and table saves write into that branch through its `ATTACH`, on
     the server's connection, only to tables declared in
     `options.scenarios.tables`.
   - "Add scenario" is a `::SAVE` with `'create'` as its action.
5. **Approval is a separate, narrower right.** `freeze`, `unfreeze`, `merge`
   and `drop` need `options.scenarios.approvers` (users or groups), which is
   distinct from `writers`. A merge runs `scenario_merge(name, on_conflict
   := 'abort')` and answers with the conflicts instead of applying them.
6. **Every operation is logged**: who, when, which scenario, what changed
   (row counts per table). The log uses the write-back log's format and
   guarantees (`flock`, `O_NOFOLLOW`, `fdatasync` before the `201`), with
   `"kind":"scenarios"`. With `options.audit`, a line goes to the audit
   log as well.

## 2. Vocabulary in a dashboard

```sql
-- the picker: main + every scenario the server exposes (scenario_list())
SELECT name AS scenario ::SCENARIO FROM anofox_scenarios();

-- panels read the chosen branch: no change to their SQL
SELECT week ::XAXIS, sum(qty) AS "Planned" ::BARCHART FROM plan GROUP BY 1;

-- a form or an editable table saves into the chosen branch
SELECT field AS cap ::FORM, … FROM capacity_params;
SELECT 'Save to scenario' ::SAVE, 'cap' ::SAVE_FORM, 'capacity' ::SAVE_TABLES, 'scenario' ::SAVE_SCENARIO;

-- a new scenario from the chosen one
SELECT 'Demand +10 %' AS new_name ::TEXT;
SELECT 'Add scenario' ::SAVE, 'create' ::SAVE_ACTION, 'new_name' ::SAVE_KEY, 'scenario' ::SAVE_SCENARIO;

-- compare: KPIs per scenario, and what changed
SELECT scenario, sum(qty) FROM anofox_scenario_rows('plan') GROUP BY 1;   -- §4
SELECT * FROM anofox_scenario_diff(getvariable('scenario'), 'plan');      -- §4

-- approve (approvers only)
SELECT 'Freeze' ::SAVE, 'freeze' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
SELECT 'Make official' ::SAVE, 'merge' ::SAVE_ACTION, 'scenario' ::SAVE_SCENARIO;
```

The names are proposals. `::SAVE_ACTION` (`write` the default, `create`,
`freeze`, `unfreeze`, `merge`, `drop`) keeps one button role.

## 2b. In the page: how a planner adds and uses scenarios

The roles above render as one **scenario bar** at the top of the dashboard
(like the filter bar). Nothing is typed as SQL.

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ Scenario: [ main ▾ ]   [＋ New scenario]   [Compare ▾]   [Freeze] [Make official] │
└──────────────────────────────────────────────────────────────────────────────┘
```

1. **Pick:** the dropdown lists `main` and every scenario: name, its parent,
   who made it, a 🔒 when frozen. Picking one re-runs the page on that
   branch, and the URL keeps it (`?v.scenario=…`), so a link opens the same
   scenario.
2. **New scenario:** the button opens a small dialog: a name, an optional
   description, "based on: <the current one>". *Create* calls the server
   (`::SAVE_ACTION 'create'`). The new scenario becomes the picked one, and
   the bar shows "editing *Demand +10 %*".
3. **Change it:** the dashboard's forms and editable tables now show
   "Save to *Demand +10 %*" on their `::SAVE` buttons. Unsaved edits are
   marked, as in the builder. Every save writes into the branch, never to
   `main`. A frozen scenario disables them, with the reason.
4. **Compare:** *Compare ▾* picks other scenarios (or `main`) to show beside
   the current one.
   - Panels that use `anofox_scenario_rows()` draw one series per chosen
     scenario.
   - A "What changed" panel lists `anofox_scenario_diff()` rows: added,
     removed, modified, with old → new values.
5. **Approve** (approvers only; others see the buttons disabled with the
   reason):
   - *Freeze* locks the scenario.
   - *Make official* previews the merge (`scenario_merge_preview`): N rows
     per table, conflicts listed. After a confirmation it merges, and the
     scenario ends frozen.

Without the server options (a static site, a server without
`options.scenarios`), the bar shows `main` only, with a one-line reason.
The level-1 pattern (a name field plus `::SAVE` into a write-back table)
works everywhere write-back does.

## 3. Server options

```json
"scenarios": {
  "database": "warehouse",
  "path": "/var/lib/anofox/scenarios.jsonl",
  "dashboards": ["sop-plan"],
  "writers":   {"groups": ["planners"]},
  "approvers": {"users": ["head.of.planning@example.com"]},
  "tables": {
    "plan":     {"key": ["item", "week"], "columns": {"item": "VARCHAR", "week": "DATE", "qty": "DOUBLE"}},
    "capacity": {"key": ["line"], "columns": {"line": "VARCHAR", "per_shift": "DOUBLE"}}
  },
  "mode": "delta",
  "max_scenarios": 50,
  "max_rows_per_save": 5000,
  "rate": {"per_minute": 12, "burst": 5}
}
```

- **`database`:** the attached DuckDB database whose tables are branched.
  The server opens its own writable connection to it. That database must
  not be attached `READ_ONLY` in the serving session.
- **`tables`:** the only tables a save may write, the only columns, each
  value coerced to its declared type (as write-back). Other tables are
  branched by anofox-scenario but can't be written from a dashboard.
- **`mode`:** how `create` makes a scenario: `delta` (cheap, sees later base
  changes) or `materialized` (isolated, reproducible).
- **`auth`:** required, with personal sign-ins only (no API keys, no shared
  embed identity), exactly as write-back.

## 4. Reading several scenarios at once

A comparison needs every scenario in one query, but the request
connection can't `ATTACH`. The server attaches the branches at snapshot
time and exposes two read-only table functions, registered on the snapshot
connection:

- **`anofox_scenarios()`:** `name, mode, frozen, parent, created_at,
  created_by, description, merged_at`, from `scenario_list()` plus the
  log's `created_by`.
- **`anofox_scenario_rows(table)`:** `scenario` plus the table's columns,
  over `main` and every exposed branch. It is a `UNION ALL BY NAME` the
  server generates from the declared tables only (a table name outside
  `tables` is an error).
- **`anofox_scenario_diff(scenario, table [, against := 'main'])`:**
  `scenario_diff` of a declared table, read-only.

## 5. Request flow (a save)

1. `POST /api/save` with `{dashboard, panel, id, vars, form | table | action,
   scenario}`. It takes the same CSRF gate, body cap, rate limit and
   idempotency key as write-back.
2. **Checks:** the dashboard is listed; the user is in `writers` (or in
   `approvers` for freeze/merge/drop); the scenario exists and isn't
   frozen (unless the action is `unfreeze`); the table is declared; the
   values coerce.
3. **On the server's writable connection**, in one transaction:
   `ATTACH ? AS w (TYPE scenario)`, then `INSERT … ON CONFLICT` with bound
   values, then `DETACH w`. Create, freeze and merge call the
   `scenario_*` function with bound arguments.
4. **The log line** is written and fsynced before the `201`.
5. **Snapshot refresh**, debounced (a burst of saves gives one refresh).
   The answer carries the new epoch, and the page re-runs once the
   snapshot has it.

## 6. Threat model

| Threat | Mitigation |
|---|---|
| SQL injection through a scenario name or value | Names: `^[A-Za-z0-9 _.+%-]{1,64}$`, bound as parameters, never spliced; the attach alias is fixed (`w`). Values: coerced to the declared types and bound. |
| Writing an undeclared table, or the internal `__anofox_scenario` schema | Only `options.scenarios.tables`. Writes go through the scenario `ATTACH`, which anofox-scenario keeps away from its internal schema. |
| A viewer escalating to approval | Separate `approvers` list. Freeze/merge/drop refused (`403`) otherwise, and the buttons disabled with the reason. |
| Merging over someone else's change | `on_conflict := 'abort'` only; conflicts are shown, never auto-resolved. |
| Unbounded growth (scenarios, rows) | `max_scenarios`, `max_rows_per_save`, rate limits, and `drop` for approvers. |
| A request reaching the writable connection | It is Rust-only: no SQL from a request ever runs on it; panels only see the read-only snapshot. |
| Denial of service through refreshes | Debounced refresh, at most one at a time (the existing refresh lock). |

The security review comes before the first write slice (§8, S2).

## 7. The browser builder and `--check`

- **Builder (DuckDB-Wasm):** anofox-scenario has no wasm build today. Until
  it does, the builder shows the picker with `main` only, the scenario
  buttons disabled with the reason, and the comparison helpers return
  `main`. Measure the extension's wasm build before promising more.
- **Live authoring (`serve`):** the same operations run on the session's
  own connection, without the locked server's checks, like `::SAVE` today
  (TEMP-level or direct writes, for the author only).
- **`--check`:** validates the roles, that each `::SAVE_SCENARIO` names a
  declared input, and that saved tables are declared when an options file
  is given (`--options`).
- **Static outputs:** the picker renders as `main`.

## 8. Slices

| Slice | What | Needs |
|---|---|---|
| S1 | Read-only: the snapshot attaches the branches; `::SCENARIO` picker via `search_path`; `anofox_scenarios()`, `anofox_scenario_rows()`, `anofox_scenario_diff()` | anofox-scenario loaded in the serving session; no writes |
| S2 | Create and write: `::SAVE_SCENARIO`, `::SAVE_ACTION 'create'`, the writable connection, the log, the refresh | the security review |
| S3 | Approve: freeze/unfreeze/merge/drop for `approvers`, the conflict report | S2 |
| S4 | Builder: wasm, if anofox-scenario builds for it | an anofox-scenario wasm build |

An example dashboard per slice, served e2e like `form-served-e2e.mjs`.

## 9. Decisions for the owner

**Decided (owner, 2026-11-19): D1–D6 as recommended.**

- **D1:** server-owned operations on a writable connection plus snapshot
  refresh (proposed), rather than the browser or a request running scenario
  SQL. *Recommendation: yes. It is the only form that keeps the locked
  server's guarantees.*
- **D2:** reuse `::SAVE` with `::SAVE_SCENARIO` / `::SAVE_ACTION`, rather
  than new button roles. *Recommendation: reuse. One button concept,
  documented once.*
- **D3:** separate `approvers` for freeze/merge/drop. *Recommendation:
  yes.*
- **D4:** the default `mode` for `create`: `delta` (cheap), or
  `materialized` for reproducible approved plans. *Recommendation: `delta`
  for create; materialize at freeze time (freeze a materialized copy) when
  `options.scenarios.freeze_materialized` is set.*
- **D5:** order: S1 (read-only) first, then S2 after the security review.
  *Recommendation: yes.*
- **D6:** level 1 (parameter rows via write-back) stays as is and is
  documented as the lighter option.

## 10. Open questions to verify before S1

- Does `ATTACH … (TYPE scenario)` work in the snapshot instance after the
  databases are copied (`COPY FROM DATABASE` copies the internal schema as
  plain tables), or must the snapshot attach the live file read-only
  instead of copying it?
- Does `search_path` per request reach a scenario catalog's tables with
  the same names as the base's?
- What does a refresh cost on a large base with many delta-mode scenarios?
  Measure it on the load-test data.
