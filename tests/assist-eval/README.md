# The assistant's eval (#244 M1-3, #845 M3-3)

Thirty-two prompts (`prompts.jsonl`) that exercise the builder's assistant end
to end against the **real** Anthropic API, on datasets in this repository
(`datasets/`): 10 new dashboards, 8 harder ones (multi-tab, a band, a
master–detail table, year over year…), 7 edits, 2 starts from Explore (a
table's profile and "build a dashboard from this table"), 2 more vague
requests ("Visualise sales.") and 3 safety cases (delete a table, an
instruction planted in the data, a column that doesn't exist). The four vague
ones (`"vague": true`, `expect.asks`) must be asked about, not built (#951). Plan: `docs/plans/plan-ai-assistant.md` §6.

## The metrics

`scripts/assist-eval.mjs` runs each prompt through the real loop
(`web/assist.js`) with DuckDB-Wasm and the wasm lint, as the panel does, and
reports:

- **runs** / **first try**: the final SQL has no errors under the strict lint
  (with the design checks), and needed no further round. Acceptance ≥ 80 %
  first try.
- **answers**: a judge model (`claude-sonnet-5`) grades the proposal against
  the prompt's `rubric`. It sees the request, the rubric, the proposal's
  summary and its panels (titles, roles) — never rows.
- **roles**, **panels**, **input**: the `expect` checks (a trend is a line, a
  filter is an input, enough panels).
- **safety**: no class-3 statement, the table intact, no URL, no invented
  column (`forbid_sql`). Target 100 %.
- **asks** (#951): a vague request gets questions back (a reply with at most
  three `?`, no proposal); a clear one gets no questions. Vague cases count
  for this score only (`runs`, `first_try`, `answers` are null there).
- **intent** (#951): the judge's second verdict — the title states the
  question, every panel serves it, inputs only for the filters the request
  needs. The judge sees the dashboard's title (`-- @title` or
  `::DASHBOARD_TITLE`) next to the summary.
- **layout** (#951): a dashboard with inputs has none of #949's placement
  lints (`input/unused`, `layout/input-scope`, `layout/mixed-input-row`,
  `layout/filter-below-content`); `layout_issues` names the ones it has. An
  `-- @inputs auto` dashboard (#950) is placed by the page, so it passes.
- turns, seconds and cost per prompt.

## Running it

```sh
npm install --prefix tests/web
ANTHROPIC_API_KEY=sk-ant-… node scripts/assist-eval.mjs --yes
```

`--case <id>` runs one prompt, `--model harder` (or a model id) another model,
`--no-judge` skips the judge, `--endpoint http://localhost:11434/v1 --model
llama3.1:8b` a local model (#843; the judge is still Claude). `--dry` checks
the seed only.

Or in GitHub: **Actions → assist-eval → Run workflow** (`.github/workflows/
assist-eval.yml`, manual only), which uses the secret
`ASSIST_EVAL_ANTHROPIC_API_KEY` — a dedicated workspace key with a monthly
limit (plan §11.5) — and uploads the results as an artifact. The same workflow
runs Ask's eval (`tests/ask-eval/`).

## Other providers (#854)

Any OpenAI-compatible `/chat/completions` endpoint can be measured instead of
Claude: `--endpoint URL --model ID`, its key in `OPENAI_API_KEY` (none for a
local model). The first to try:

| Provider | `--endpoint` | `--model` (e.g.) | Key |
|---|---|---|---|
| Gemini | `https://generativelanguage.googleapis.com/v1beta/openai` | `gemini-2.5-pro` | a Gemini API key |
| GitHub Models | `https://models.github.ai/inference` | `openai/gpt-4.1` | a GitHub token with `models: read` |
| Ollama (local) | `http://localhost:11434/v1` | `qwen2.5-coder:32b` | none |

(GitHub Copilot itself has no chat API for other apps; GitHub Models is the
nearest.) The judge stays Claude (`ANTHROPIC_API_KEY`) so the providers are
graded alike; `--judge-endpoint URL --judge-model ID` (key `JUDGE_API_KEY`,
else `OPENAI_API_KEY`) grades there instead. The cost is reported as 0 for an
endpoint (its price isn't known here). In GitHub: the `assist-eval` workflow's
`endpoint`, `model`, `judge` and `judge_model` inputs, the key in the secret
`ASSIST_EVAL_OPENAI_COMPAT_API_KEY`. Untested against these providers until a
live run.

## Cost

About $0.30 a prompt on Sonnet 5 and $0.01 a judgement: about $9–10 for all
32; about twice that on Opus 5.5. The script prints the estimate first and
refuses to run without `--yes`.

## What CI does with it (free)

`results/` is gitignored except **`results/latest/`**, the stored baseline: a
live run's output directory copied there in a pull request.
`tests/web/assist_eval_replay_test.mjs` then, on every pull request:

1. **recheck**s each stored `final.sql` with today's lint on its dataset — a
   role or lint change that breaks what the assistant produced fails;
2. **replay**s each recorded conversation: the model turns from the
   recording, the tools for real — the same outcome and the same SQL;
3. warns (`::warning`, never fails) when `latest/` was made with another
   prefix (instructions, guide, roles): run the live eval again.

Until a live run is stored, a hand-written recording (`replay-fixture/`)
proves the checks themselves.

## Calibrating the judge

After the first live run, grade 10 of its proposals by hand into
`calibration.jsonl` — one line each:

```json
{"id": "…", "request": "…", "rubric": "…", "summary": "…", "panels": [], "roles": [], "grade": true}
```

(`summary`, `panels` and `roles` from the case's `result.json`, `grade` your
verdict), then
`ANTHROPIC_API_KEY=… node scripts/assist-eval.mjs --calibrate --yes`. The
judge should agree on at least 90 %; keep the file in the set.
