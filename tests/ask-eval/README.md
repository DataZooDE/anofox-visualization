# The Ask eval (#195 M2-7, #846 M3-4)

Thirty questions about the example dashboards (`cases.jsonl`), asked of the
**real** Anthropic API — never mocked — on both places Ask runs:

- **server**: a real locked server (`anofox_serve_dashboards` with
  `options.ask`, proxy login), `POST /api/ask`, the server's own loop. Every
  request it sends upstream goes through a local relay that records it, so a
  canary can be looked for in what left the server, not just in the answer;
- **builder**: the builder page in a real browser (Playwright), View mode's
  ✦ Ask, the page's loop with your key; the browser's calls are recorded on
  the way through.

This is a measurement, not a gate: `scripts/ask-eval.mjs` prints each case and
a summary per target against the plan's targets (§12.9): facts ≥ 85 %,
citations ≥ 95 %, refusals 100 %, plus the judged share, cost, model calls and
seconds per question.

## Running it

```sh
npm install --prefix tests/web                 # DuckDB-Wasm + Playwright
duckext/scripts/build-native.sh                # the extension, for the server target
ANTHROPIC_API_KEY=sk-ant-… node scripts/ask-eval.mjs --yes \
  --ext /tmp/anofox_visualization.duckdb_extension
```

Needs a built `web/pkg` (`wasm-pack build --target web --out-dir web/pkg
--no-default-features --features wasm`) and `duckdb` on `PATH` (or
`--duckdb`). `--target server|builder` runs one side, `--case <id>` one case,
`--model claude-opus-5-5` another model, `--no-judge` skips the judge.

`--dry` checks the seed's shape. `--truth` also runs every truth query and
prints it (free). CI runs `tests/web/ask_eval_seed_test.mjs`: the shape, every
truth query on the example data, every named panel, the served copies.

## A case

| Field | Meaning |
|---|---|
| `dashboard` | the dashboard, from the repository root |
| `kind` | `fact`, `view` (the same with a tab or input values), `explain`, `meaning`, `refusal` |
| `view` | `{tab?, vars?}`: what the viewer has open; vars as URL seeds in the builder, `vars` in `/api/ask` on the server |
| `targets` | `["server"]` for a case only a server can pose (an access policy, a `-- @requires`-hidden tab); default both |
| `truth` | one `SELECT` over the dashboard's tables, run at eval time after its setup (DuckDB-Wasm, the browser's version): every value of its one row must be in the answer — text by name, a number within the precision the answer writes it with (or `tol`, relative), a date in any common spelling |
| `panels` | where the fact is: `[{name}]` (an `-- @name`) or `[{sql}]` (text in exactly one panel's SQL); a citation counts only when it is verified and points at one of them |
| `rubric` | for `explain` / `meaning`: what a judge model (`claude-sonnet-5`, sees the question, the rubric and the answer — no rows) needs to pass it |
| `scope` | the answer's scope must be one of these |
| `forbid` | text that must not be in the answer |
| `canary` | text that must not be in the answer nor in any request sent upstream |

The server serves a copy of each dashboard with `-- @ask on` and
`-- @vars <its inputs' variables>` on top and its setup made `TEMP` (a locked
server's data is read-only); `dashboards/orders.sql` gets a server with a data
folder (so its `-- @requires` hides a tab), `dashboards/staff.sql` one with an
access policy that nulls the salary column (`SERVER_FIXTURES` in the script).

## Cost

About $0.05 a question on Sonnet 5 and $0.01 a judged answer: about $3 for
both targets; less on `--model claude-haiku-4-5` (the cheaper model §12.10
evaluates — it runs without thinking). The script prints the estimate first and refuses to run without
`--yes`.

## Results are not committed

`results/` is gitignored. The owner runs the eval with their own key and pastes
the summary into the PR. Or run it in GitHub: **Actions → assist-eval** with
`eval: ask` (`.github/workflows/assist-eval.yml`, #845).
