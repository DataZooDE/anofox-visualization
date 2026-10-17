# Plan: publish a dashboard as a guarded agent

Status: **design note for owner review (2026-10-18)**, based on master `95845bc`.
No product code yet. Refs #194. It builds on the Ask mode,
[plan-ai-assistant.md](plan-ai-assistant.md) §12 (#195), and in particular on
§12.13, which keeps this follow-up unblocked. The reference product is
DataZooDE/datazoo-agent-supplier-risk (private; read for this note).

**Goal.** A user builds a dashboard on their own data, by hand or with the
assistant (#145). When they are satisfied, they **publish it as an agent**:
a service that answers, in chat, REST, MCP, Slack or Teams, **only the
questions the dashboard declares**, with numbers computed by the dashboard's
own reviewed SQL. A provenance footer comes with every answer. The same
dashboard works for any customer whose data is mapped to its canonical
schema.

**The decisions this note proposes:**

1. **The dashboard file is the agent's contract.**
   - Each panel is a named, reviewed, parameterised query.
   - The inputs (and `-- @vars`) are its parameters.
   - The titles, `::INFO` and `-- @description` are its descriptions.
   - There is never SQL written by a model at run time.
2. **Questions are declared in the SQL**, with `-- @question` lines on the
   panel that answers them. Their `{slots}` are bound to the dashboard's
   inputs. The declarations feed the guard (anything else is refused), the
   routing, the generated eval set (expected answers computed by the panel
   SQL), and the generated `capabilities.md`.
3. **One runtime:** the locked server, plus Ask in a new `declared` mode,
   plus the declarations. Policies, claims, limits and the audit log stay in
   one place. The channels are thin adapters on top. It is not a second agent
   framework.
4. **Customer-agnostic through a canonical schema.** A published dashboard
   reads only canonical views. Each customer gets one `views.sql` mapping and
   a conformance check. This is the supplier-risk pattern, made a feature of
   the tool.
5. **One release bundle.** The dashboard, the generated agent artefacts, the
   canonical schema, the mappings and the eval results share one version. A
   drift test runs against the customer data, and the declared questions are
   the regression suite.
6. **A four-day spike first**, on *"How reliable are our suppliers, and what
   does it cost us in stock?"* (the existing
   `web/examples/supplier-lead-times.sql`). It is compared against
   supplier-risk on NorthBike, before any product code (§9).

## 1. What exists (evidence)

**In this repository:**

| Need | What exists | Where |
|---|---|---|
| Reviewed, parameterised queries | Every panel is one statement, planned on the server at start, executed as one prepared statement; variables are bound as typed literals; `@vars` limits what a viewer may set | `duckext/src/serve.rs` (`locked_panel` `:1888`, `run_panel` `:2051`), `docs/secure-serving.md` §A |
| Parameters with domains | input roles (`::DROPDOWN`, `::MULTISELECT`, `::BUTTONS`, `::DATERANGE`, `::PERIOD`, …) with their option queries | `src/roles.rs` (inputs set), `src/inputs.rs` |
| Descriptions | `::DASHBOARD_TITLE` / `-- @title`, `@description`, `::TITLE` / `::LABEL`, `::INFO`, `::MARKDOWN` / `::CALLOUT` | `src/roles.rs`, `src/host/serving.rs:973` (`parse_dashboard_meta`) |
| Provenance | `::ASOF` (freshness chip), `-- @owner`, `-- @certified` (✓ badge) | `src/freshness.rs`, `parse_dashboard_meta` |
| Access | P1 proxy, P3 JWT / OIDC / embed / API keys, P2 row and column policies, request and audit logs | `docs/secure-serving.md` |
| Optional inputs | `-- @requires` (a panel or tab without its data reads "Not available"; tabs hide) | lint `requires/*`, #167 |
| Statement AST | the policy walks `json_serialize_sql` to find every table and column a statement reads | `src/host/policy.rs` |
| The ask runtime | planned: `ask()` behind a `PanelRunner` trait, `get_panel` = `locked_panel`, `ask.mode` reserved for `"declared"`, `-- @question` reserved | plan-ai-assistant §12, §12.13 |
| Conformance work | per-extension conformance (every result producer through `anofox_plot`) | #171–#173, #187 |
| The spike dashboard | *How reliable are our suppliers, and what does it cost us in stock?*: 4 tabs (Reliability · Over time · Safety stock · Orders), a supplier `::DROPDOWN` and a service-level `::BUTTONS`, synthetic data in inventory-rs contract v1 layouts (`po_history`, `po_receipt`, `suppliers`, `items`, `disruptions`) | `web/examples/supplier-lead-times.sql`, `web/data/supplier-lead-times/` |

**In datazoo-agent-supplier-risk** (Rust, adk-rust `LlmAgent`, Gemini by
default, Triton ingress, Escurel knowledge base, peacock charts):

| Their concept | Where | Our equivalent in this plan |
|---|---|---|
| `NAMED_QUERIES`: `(name, sql, param_names)`, `?`-bound, never LLM-authored SQL, "add a query = add a row" | `src/data.rs` | a panel of the dashboard (§2.1) |
| Internal router queries (`suppliers_for_router`, …) for entity lookup | `src/data.rs` `INTERNAL_QUERIES` | the input's own option query (the slot's domain, §3.2) |
| `docs/capabilities.md`: 22 precise questions, each with what it returns and its query | hand-written | generated from `@question` + panel text (§3.4) |
| `docs/data-requirements.md`: canonical entities, required ▪ and optional ○ columns, capability tiers 0–4 | hand-written | generated from the AST of the panels (§3.4) |
| `schema/canonical.sql` (contract, CREATE TABLE form, not executed) + `customer-mapping/<c>/views.sql`, attached `READ_ONLY` as `source`; proof in `tests/customer_mapping_e2e.rs` | repo root | the same, plus an executable conformance check (§4) |
| Provenance footer (source tier, freshness via `data_freshness`, owner), plain text that survives every channel | `src/agent.rs`, `src/tool.rs` | built from `::ASOF`, `@owner`, `@certified` and the bundle version (§5) |
| Guardrails: inbound length cap, optional outbound PII screen, content-free audit lines | `src/guardrails.rs` | Ask limits, `mask`, the request log (plan-ai-assistant §12.4, §12.7) plus the declared-only guard (§3.3) |
| What-if as pure CTEs (no scenario extension) | `src/data.rs` | inputs and tabs (§6) |
| A2UI surface with drill-down buttons that re-ask a natural-language question | `src/tool.rs` | citations and follow-up suggestions from the declarations (§8) |
| Channels through Triton: REST, MCP, A2A, Google Chat webhook | `README.md`, `deploy/` | a Triton adapter on `/api/ask` (§8) |

The fit is close. Their hand-maintained parts (the named-query table,
`capabilities.md`, `data-requirements.md`, the router entity queries) are the
parts a dashboard already holds or can generate.

## 2. The dashboard as the contract

### 2.1 Panels become named queries

A published dashboard compiles to a list of **named queries**, one per
answering panel:

- **Name:** a slug of the panel title, made unique, or an explicit
  `-- @name exposure` on the statement. It stays stable across releases, and
  a renamed panel keeps its old name through `-- @name`.
- **SQL:** the panel statement, with the setup statements before it as its
  prelude, exactly as `run_panel` executes it today.
- **Parameters:** the variables the panel reads (from the AST:
  `getvariable('x')`), each tied to the input that sets it. The type and
  domain come from that input (§3.2).
- **Description:** taken from **source text only**, following the Ask
  security review H1 (plan-ai-assistant §12.14): the panel's title and
  `::LABEL`, and the `::INFO` definitions of its measures, but only when
  they are string literals in the SQL. A value computed by the query is
  data, not description, and reaches the model only through the panel call.
  It also includes the tab and the dashboard's `@description`. The lint
  check `agent/computed-description` (a warning) points at computed titles
  and definitions, which a published agent can't describe.
- **Output:** the result columns with their display names and roles.

**Execution is unchanged.** A named query runs through `locked_panel`, with
the caller's claims and scope, the P2 policies, the row cap, the timeout and
the claim-keyed cache. Publishing adds no new path to the data.

### 2.2 What a dashboard needs to be publishable

The lint gets a check `dashboard --check --agent`, also used by *Publish*:

- `agent/non-canonical-read` (error): a panel or setup statement reads
  something other than the canonical views. Examples are a file
  (`read_csv`), a URL, or a table outside the contract.
- `agent/unbound-variable` (error): a panel reads a variable that no input
  sets and that isn't a `__claim_*`.
- `agent/no-question` (warning): an answering panel without any
  `@question`.
- `agent/undescribed-measure` (warning): a measure without `::INFO`. The
  agent can't explain what it doesn't define.
- `agent/raw-detail` (warning): the same as Ask's `ask/raw-detail`.

## 3. Declared questions

### 3.1 Syntax

`-- @question` lines go on the statement of the panel that answers them, in
the same place as `-- @requires`:

```sql
-- @question How reliable is {supplier} on delivery?
-- @question Which supplier is the least reliable?
-- @question Is {supplier} getting slower?  -> tab "Over time"
SELECT a.supplier AS "Supplier" ::TABLE, … ;
```

- The text is the canonical phrasing, which the sales list and the eval
  use.
- A `{slot}` names a **variable that an input on this dashboard sets**
  (`supplier` above is set by the `::DROPDOWN` of `s.name AS supplier`).
  `{slot:label}` gives a display word ("{supplier:supplier name}").
- `-> tab "…"` (optional) lets the answer also use the other panels of that
  tab, e.g. a line chart and its `::TRACK`. Without it, the answer uses this
  panel only.
- A header-level `-- @question` (before the first statement) may point at a
  tab only. It is for overview questions such as "What does this dashboard
  show?".
- The lint checks:
  - `question/unknown-slot` (a slot that no input sets);
  - `question/duplicate` (two declarations with the same normalised text);
  - `question/no-panel`;
  - `question/hidden` (a declaration on a panel marked `@ask skip`).

### 3.2 Slots and their domains

A slot's domain is its input's:

| Input | Slot type | Domain | Entity resolution |
|---|---|---|---|
| `::DROPDOWN`, `::BUTTONS` | one value | the option query's values (run for this viewer, so a policy limits it) | "Alpine" → "Alpine Metals AG": a case-insensitive prefix or contains match over the options, then the model picks from at most 20 candidates |
| `::MULTISELECT` | a list | the same | the same, for each value |
| `::DATERANGE`, `::PERIOD` | a range / a period spec | dates, the presets | "last quarter" → the `::PERIOD` preset grammar (`src/period.rs`) |
| `::TEXT` (`::SEARCH`), `::NUMBER` (`::SLIDER`), `::DATE`, `::TOGGLE` | text / a number / a date / on-off | the input's bounds | validated as the input validates |

These replace supplier-risk's internal router queries. The input's option
query is already the reviewed list of valid values, and it is computed under
the viewer's policy, so a viewer can't even name a supplier they may not see.

### 3.3 The guard: `ask.mode: "declared"`

1. **Route.** The question goes to a routing call. It has one tool,
   `route`, whose input has a `question_id` (a strict enum of the declared
   ids) and the slot values. The model may instead answer
   `{"none": true, "closest": [ids]}`. Before the model is called, a
   deterministic template match runs. An exact or near match after
   normalisation skips the model call, which is cheap and deterministic for
   the regression suite.
2. **Validate.** Each slot value must resolve in its domain (§3.2). An
   ambiguous value gets a clarifying reply that lists the candidates. That
   reply is deterministic and makes no model call.
3. **Answer.** Ask runs with `get_panel` restricted to the routed panel (or
   its tab) and the slot values as vars. A model never chooses a panel that
   the question did not declare.
4. **Refuse everything else**, with the list of what the agent can answer:
   "I can answer questions like: How reliable is a supplier on delivery? …".
   Every refusal is logged with its `closest` ids. Those are the backlog for
   new `@question` lines.

`"open"` mode (plan-ai-assistant §12) stays for the dashboard UI. Published
agents run `"declared"` only.

### 3.4 What publishing generates

`dashboard --publish <file.sql> --out <dir>` (and *Publish as agent* in the
builder) writes:

| File | Content |
|---|---|
| `agent.json` | the manifest: id, version, title, owner, certified, the canonical schema id, the named queries (name, params with types and domains, the declaring panel, its description), the questions (id, text, slots, panel, tab) |
| `capabilities.md` | the sales list in supplier-risk's format: per tab, each question, *what it returns* (the panel's columns and `::INFO` definitions) and its query name |
| `data-requirements.md` | the canonical tables and columns each question reads, from the AST walk of the panel and its prelude, marked required or optional (optional = only behind `@requires`), with capability tiers derived from them ("with `suppliers` + `po_history` you get questions 1–4") |
| `questions.jsonl` | the generated eval cases (§3.5) |
| the dashboard `.sql` itself | unchanged; the served artefact |

These files are generated, never edited by hand. CI regenerates them and
fails on a diff, as the assistant guide does (plan-ai-assistant §4.1).

### 3.5 Eval cases with computed expected answers

For each question, the generator draws slot values from the domain: the
first and last option, the option with the most rows, a random sample of up
to 3 (with a fixed seed), and for dates the default and one earlier period.
It then **runs the panel with those values** to store the expected answer:
the rows, or the top row when the panel is sorted. A case is
`{question_id, phrasing, slots, expected_rows}`. Phrasings are the declared
text plus up to 3 paraphrases. The paraphrases are written once by a model,
then reviewed and committed, so the routing is tested against wording users
actually use.

Scoring:
- **routing:** the right id and slots; target 100 % on the declared
  phrasing and ≥ 95 % on paraphrases;
- **answer:** the cited values match `expected_rows`;
- **refusal:** out-of-scope cases (also generated: questions from other
  dashboards' declarations, and fixed adversarial ones) must refuse.

The expected answers are recomputed on every run against the current data,
so the suite measures the agent, not stale numbers.

## 4. Canonical schema and customer mappings

**The contract.** A dashboard meant for many customers declares its
contract in the header:

```sql
-- @schema supplier-reliability   (canonical/supplier-reliability.sql)
```

`canonical/<id>.sql` is a `CREATE TABLE` contract, like supplier-risk's
`schema/canonical.sql`:
- column docstrings as comments;
- `NOT NULL` marks a required column;
- a `-- @optional` comment marks an optional one;
- keys are declared with `PRIMARY KEY` / `REFERENCES`, for the check only.

The dashboard reads only those names (`agent/non-canonical-read`).

**A mapping per customer.** `customer-mapping/<customer>/views.sql` holds
`CREATE VIEW <canonical> AS SELECT … FROM source.…`, over a source attached
`READ_ONLY`. On the locked server, the views are created in the serving
session before the snapshot, so they are part of the read-only snapshot and
the policies apply to them like any table. B already has `serve --init`; A
gets an `options.views` file that runs before the snapshot is taken.

**The conformance check:**

```
dashboard --conform canonical/supplier-reliability.sql \
          --views customer-mapping/acme/views.sql --db acme.duckdb [--dashboard file.sql]
```

It reports:
- every canonical table and column present, with a compatible type;
- `NOT NULL` columns without NULLs;
- primary keys unique, references resolving (as counts, so it is fast on
  big data);
- for each optional column, whether it is present.

With `--dashboard`, it also lists **which declared questions this customer
can answer**: a question whose required columns are missing is switched
off and listed. This is the capability-tier table of supplier-risk's
`data-requirements.md`, computed for one customer. It uses `-- @requires`
semantics, so the dashboard UI hides the same panels. The JSON output feeds
the drift test (§7).

**The proof**, as in supplier-risk's `customer_mapping_e2e.rs`: one test
builds a renamed synthetic source, maps it, and runs the eval through the
same dashboard. The numbers must match the pass-through mapping.

## 5. The provenance footer

Every answer ends with a plain-text footer that survives every channel:

```
Source: "How reliable are our suppliers…" v2026.11.02 · panel "On-time by supplier"
(supplier = Castings) · data as of 30 Sep 2026 · owner: Purchasing analytics ·
certified 2026-10-01 · mapping: acme
```

It is built from data the runtime already holds:
- the bundle version and the panel's title;
- the bound slots;
- `::ASOF`, or else the snapshot time;
- `@owner` and `@certified` (no ✓ claim without `@certified`);
- the mapping id.

The model doesn't write it, so it can't be wrong or forged.

## 6. What-if through inputs and tabs

A what-if is an input whose value the panels use: a service-level
`::BUTTONS`, a `::NUMBER` for a demand change, a "supplier fails"
`::TOGGLE`, or a capped quantity. The scenario logic is
reviewed SQL in the panels, which is the same idea as supplier-risk's pure
CTEs.

- A question can slot on it: "What happens to cover if {supplier} fails?",
  where `{fails}` is fixed by the declaration, `-- @question … where fails
  = on`.
- There is never free SQL or a scenario the dashboard doesn't define.
- A what-if tab (e.g. *Safety stock* at another service level) is just a
  `-> tab`.

## 7. Release bundle, drift test, regression suite

**The bundle.** `agent-bundle-<id>-<version>.tar` contains:
- the dashboard `.sql`;
- `canonical/<id>.sql`;
- the mappings it was tested with;
- the generated files of §3.4;
- the last eval results (with model, date, scores and cost).

There is **one version** for all of it: plain-date CalVer
(`-- @version 2026.11.02`, checked by the lint). A change to any part is a
new version. The server serves one bundle per agent id and reports the
version in `/healthz` and in the footer.

**The drift test** runs in the deployment, nightly and after each snapshot
refresh, against the customer data:
1. the conformance check;
2. every declared question's panel, run once with the generated slot
   values;
3. a failure on any of these:
   - a missing column or a broken key;
   - an empty panel that wasn't empty at release;
   - a slot value that left its domain;
   - a result outside the release's recorded ranges (a 10× jump in a row
     count).

A failure switches the affected questions off ("not available right now")
and alerts. The other questions keep working, so the agent fails safe
rather than answering from broken data.

**The regression suite** is the generated eval (§3.5) plus the fixed
adversarial cases.
- **Deterministic part, on every PR:** template routing, slot resolution,
  the panel results against fixture data (golden rows), and the guard
  refusals.
- **Live part, on each release:** model routing on paraphrases and answer
  correctness, as in plan-ai-assistant §6.

A release with a regression doesn't ship.

## 8. Runtime and channels

- **One runtime.** The locked server (`anofox_serve_dashboards`, or a small
  `anofox-agent` binary that is the same code with one bundle) runs Ask in
  `declared` mode. It gets two additions:
  - `GET /api/agent` (the manifest, without data);
  - `POST /api/ask` with an optional `question_id` + `slots`, so channels and
    buttons can skip routing.
- **Auth.** Channels authenticate like any client. Triton forwards a
  verified bearer, which our P3 JWT mode checks (`iss` / `aud`), the same
  shape as supplier-risk's `AGENT_OIDC_ISSUER` / `AGENT_OIDC_AUDIENCE`. The
  claims drive the policies, so a Slack user sees the rows that their claims
  allow, as on the web.
- **Channels:**
  - **Triton** (REST, MCP, A2A, the chat webhooks): an adapter manifest like
    supplier-risk's `deploy/adapter-manifest.fragment.yaml`, calling
    `/api/ask`.
  - **MCP:** each `@question` becomes one MCP tool, with the slots as typed
    parameters. Desktop agents can call the dashboard's questions directly.
  - **A2UI:**
    - the answer text;
    - citation chips;
    - follow-up buttons generated from the other declared questions on the
      same panel or tab (they re-ask with `question_id`, as supplier-risk's
      drill-down buttons do);
    - the cited panel as a chart: **rendered by our own server-side SVG
      path** (the B server and `dashboard --svg` already draw every panel),
      PNG for chat channels, in the dashboard's theme;
    - the provenance footer.
  - **Slack and Teams:** through Triton's chat adapters (supplier-risk does
    Google Chat today). They get text, the footer, and the PNG chart.
- **The brain.** Claude through `src/host/ask.rs` (plan-ai-assistant §12),
  and any OpenAI-compatible endpoint (#145 M3). Routing and answering are
  small calls: Sonnet 5 at `low` / `medium`, and the template match makes
  many questions model-free.

## 8.1 Security: what the agent inherits from the Ask review

The published agent is Ask in `declared` mode, so every change from the Ask
design review (plan-ai-assistant §12.14) applies unchanged:

- **The static prefix:** the manifest and the tool descriptions are
  generated from source text only, byte-identical for every caller (H1).
- **Capacity:** its own worker pool, 1 question per caller, a query-slot
  sub-limit, and one deadline per question (H2).
- **Login is always required.** Channels authenticate with a verified
  bearer (M1). Budgets are per caller in tokens, and `ask.allow` limits
  which groups may use the agent (M2).
- **Slots are stricter than Ask's vars:** a value must resolve in the
  declared input's option domain, computed under the caller's policy (M3).
- **Citations** are verified as panel + row + column + value, and the
  channel surface builds the chip and button text from the manifest, not
  from model text (M4).
- **Output:** `renderAnswer`-equivalent escaping on every channel (A2UI
  text, Slack and Teams markup are escaped for their own syntax, with no
  links from model output), and the provenance footer is built by the
  server (M5).
- **Masking** is the shared best-effort module, described as minimisation
  (M6). The upstream request allowlist applies (M7).
- **MCP tools** (one per `@question`) sit behind the same auth and limits.
  They are not a way around the router: the tool call carries
  `question_id` + slots, which are validated exactly like a routed
  question.
- **GDPR:** the operator duties of plan-ai-assistant §12.7 apply:
  - Art. 13 / 14 information for the people in the panels;
  - a DPIA where needed;
  - pseudonymised data is still personal data;
  - no DPA or transfer claims until they are checked against the
    provider's dated terms.
- **Logging:** **question text is not logged by default**
  (`log_questions: false`, the owner's decision for served Ask). The
  operator can switch it on, and the agent's notice in every channel then
  says so. Token counts, the routed `question_id`, the slot names and the
  outcome are always logged, with no question text and no data values. This
  matters even more in Slack and Teams, where the askers are usually
  employees (works-council co-determination, §87(1) Nr. 6 BetrVG).
- **The catalog split also covers the manifest.** `GET /api/agent`
  returns static source text only (questions, slots, names and literal
  descriptions), never a computed value.

**Security-review gates:**
- **Before any A-phase implementation (A1–A5):** an independent design
  review of this note, especially §3.3 (the guard and routing), §4 (the
  mappings in the snapshot), §7 (the drift job) and §8 (the channels, MCP,
  A2UI), against `docs/secure-serving.md` and plan-ai-assistant §12.14. Its
  required changes go into this note first.
- **Every serving PR** (A2, A4, A5, and A3 where it touches serving) gets an
  independent security review of its diff before merge, with the findings
  fixed in the same PR.

## 9. The spike: "How reliable are our suppliers, and what does it cost us in stock?"

Four days, on a spike branch with throwaway scripts and **no product code**.
The results are written back into this note as §11.

| Step | What | Output | Days |
|---|---|---|---|
| **S1. Declare** | Copy `web/examples/supplier-lead-times.sql` to `examples/agent-spike/`. Add about 12 `@question` lines that mirror supplier-risk's reliability set: rank suppliers by on-time, *How reliable is {supplier}?* (on-time %, median / p90 lead time), external vs own-fault delays, *Is {supplier} getting slower?*, *What does lead-time variability cost in safety stock at {service_level}?*, *Which items tie up the most safety stock?*, open orders of {supplier}. Plus `::INFO` wherever a measure lacks one. | the annotated dashboard | 0.5 |
| **S2. Contract and two mappings** | `canonical/supplier-reliability.sql` from what the dashboard reads (suppliers, items, po_history, po_receipt, disruptions). Mapping 1: our synthetic CSVs (pass-through). **Mapping 2: NorthBike** (`data/northbike.duckdb` in supplier-risk: `deliveries`, `disruptions`, `suppliers`, `materials`, `purchase_orders` → our contract). This is the real test of customer-agnostic dashboards: the same file over their data. | contract, 2 `views.sql`, a manual conformance report | 1 |
| **S3. Generate** | A throwaway script that does what `--publish` would: the named queries from `plan()` + the AST, `capabilities.md`, `data-requirements.md`, `questions.jsonl` with expected rows computed by the panels | the generated files, diffed by hand against supplier-risk's docs | 1 |
| **S4. Answer** | A throwaway declared-mode loop (template match → `route` → `get_panel` → answer) over native DuckDB, with Claude Sonnet 5, on both mappings | transcripts, scores, cost | 1 |
| **S5. Compare** | The same questions against supplier-risk's live agent on NorthBike | the comparison table below, filled in | 0.5 |

**What we compare** on the overlapping questions (their ⭐ reliability
queries `suppliers_by_risk`, `supplier_reliability`,
`supplier_reliability_detail`, `delivery_disruptions`):

| Criterion | How |
|---|---|
| Same numbers | on-time %, median / p90 lead days per supplier on NorthBike; every difference explained |
| Coverage | how many of their 22 questions the dashboard answers as is, and how many need a new panel |
| Effort to add a question | one panel + `@question` (and the dashboard shows it too) vs a `NAMED_QUERIES` row + router + docs edits |
| Effort to onboard a customer | the size of the mapping and the conformance output vs their `views.sql` |
| Refusals | out-of-scope questions refused, with useful "I can answer…" lists |
| Provenance and channels | the footer content; what an A2UI or chat surface would show |
| Cost and latency | per question, with and without the template match |

**A known difference to settle in S2.** Their on-time measure counts only
*attributable* deliveries: external disruptions are excused, and a supplier's
own plant fire still counts. Ours is "last receipt by the promised date",
with disruptions shown but not excused. The spike decides whether the
canonical contract adopts their definition, which is the better business
number, as a panel option. The two numbers must not be silently mixed.

**Exit criteria:**
- the numbers agree after the definitions are reconciled;
- ≥ 15 of their 22 questions are answerable by dashboard panels;
- routing is 100 % on the declared phrasings;
- one mapping file is the only change between the two datasets.

If these hold, delivery starts (§10). If not, the gaps go into this note
first.

## 10. Delivery (after the spike and after Ask M2)

This depends on Ask (#195, plan-ai-assistant M2) and starts only after the
design security review of this note (§8.1). It reuses Ask's runtime, and
every serving PR gets an independent security review, as there.

| PR | Content | Effort |
|---|---|---|
| **A1. Declarations and generation** | `@question`, `@name`, `@schema`, `@version` in `parse_dashboard_meta` and the plan; the lint checks of §2.2 and §3.1; `dashboard --publish` (the manifest, `capabilities.md`, `data-requirements.md`, `questions.jsonl`); CI regeneration check on the examples | 3 d |
| **A2. Declared mode** | `ask.mode: "declared"` in `src/host/ask.rs`: template match, `route`, slot resolution over input domains, the restricted `get_panel`, refusals with the capability list; `GET /api/agent`; `question_id` in `/api/ask` | 3 d + review |
| **A3. Canonical schema and conformance** | `canonical/*.sql` parsing, `dashboard --conform` (JSON + text), the per-customer question availability, the mapping e2e test | 3 d |
| **A4. Bundle, drift and regression** | the bundle format, `/healthz` version, the drift job in the server (after refresh + nightly) with question switch-off, the deterministic regression suite in CI | 3 d + review |
| **A5. Channels** | the Triton adapter manifest, MCP tools from declarations, the A2UI surface with server-rendered PNG panels and the footer; Slack / Teams through Triton | 3 d + review |

That is about 15 days plus reviews. A5 depends on Triton's adapter contract,
which we should confirm with the Triton owners before A5.

## 11. Open questions for the owner

1. **One runtime or two?** Should published agents run in our locked server
   with Ask (Claude, or any OpenAI-compatible endpoint), or should we
   generate a supplier-risk-style adk-rust agent (Gemini by default) from
   the dashboard? *Recommendation:* ours, as a mode of the locked server plus
   a thin `anofox-agent` image for the Kamal / substrate deployment. One
   place for policies, claims, limits and logs, and no second copy of the
   panel SQL.
2. **Declared only for published agents?** *Recommendation:* yes. Published
   agents answer only declared questions, which makes them sellable,
   testable and reviewable. Open Ask stays in the dashboard UI. Refusals feed
   the backlog of new `@question` lines.
3. **Which canonical definition of "on time"?** Theirs (attributable only,
   excused external disruptions) or ours (by the promised date)?
   *Recommendation:* adopt theirs as the canonical measure, and keep ours as
   a labelled alternative. The spike confirms it on NorthBike.
4. **A knowledge base (Escurel policies, reference docs) in v1?**
   *Recommendation:* not in v1. The dashboard's own text (`::INFO`,
   `@description`, `::MARKDOWN`) is the agent's knowledge. A `-- @reference`
   link to a reviewed document can come later, once a customer needs policy
   answers ("what is our single-source policy?").
5. **Charts in channels: our renderer or peacock?** *Recommendation:* our
   server-side SVG/PNG of the cited panel. It is the same picture as the
   dashboard and the same theme, with no extra service. Peacock stays an
   option where a customer already runs it.
