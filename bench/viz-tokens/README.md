# Talk to data: anofox vs Python vs Vega-Lite (#1044)

Does an AI that answers data questions with charts need fewer tokens per
correct answer through anofox (SQL + `::ROLE` casts) than through Python
(pandas + matplotlib) or a Vega-Lite spec, and does it get to a good chart in
fewer steps?

## Design

One agent loop (`harness.py`) for every arm, with the same model and settings
(`claude-sonnet-5`, adaptive thinking, effort `medium`, prompt caching). Each
arm gets the same data (one read-only DuckDB file per dataset), the same
question, the same exploration tools (`list_tables`, `describe`, `query` ≤ 50
rows) and the same ending (`answer`). Only the chart tool and the reference
differ:

| arm | chart tool | reference (system prompt, cached) |
|---|---|---|
| anofox | `render(sql)`: annotated SQL, drawn by `dashboard --svg` | the builder assistant's role reference + guide (`anofox-reference.mjs`) |
| python | `run_python(code)`: pandas / matplotlib / seaborn, saves `chart.png` | the `matplotlib` skill + its style reference |
| vegalite | `render(sql, spec)`: the query's rows become the spec's data, drawn by vl-convert | the `altair` skill + its style reference (Altair is Vega-Lite's Python API) |

- Every chart tool answers in text only (ok, or the error). No arm sees its
  picture. Since #1095 an "ok" also says what was drawn, in the same words
  for every arm: per layer and series the marks, the x range, first / last y,
  y min / max and where. anofox: `dashboard --check --digest` (ggplot-rs's
  build); vegalite: the plotted rows by the spec's x / y / colour fields;
  python: the figure's own lines, points and bars. `--no-digest` turns it
  off (recorded in meta.json).
- `--see-chart on` (#1118): every drawn chart comes back to the model as an image
  (all arms alike), with an instruction to check it against the request and its
  answer before answering. Off by default, so runs stay comparable; meta.json records it.
- The claim check (#1116, `claims.py`): every number of an answer must appear in
  a tool's output, the model's own SQL or the request, within its rounding, or
  follow from two numbers of one row or column (difference, ratio, share, %
  change) or a column's total, mean, median, min or max. `--claim-check shadow`
  (the default) only logs what it would flag; report.md compares that with the
  judge. `on` sends unsupported numbers back once. Tool outputs are kept in
  runs.jsonl (4,000 characters each) for that comparison.
- Model-written code runs in bubblewrap: read-only filesystem, a private
  `/tmp`, no network, 60 s, and a 4 GB cgroup cap. `query` runs with
  `enable_external_access = false`.
- Every chart is trimmed of its empty border alike before it is judged.
- Arms are interleaved per task and repeat, so drift over time hits all
  three alike.

**Tasks** (`tasks.jsonl`, written by `make-tasks.py`, which checks every truth
query): 20 questions over five datasets of `tests/assist-eval/datasets/`. Each
has a ground-truth SQL and one follow-up edit with its own truth. 5 are marked
`pilot` (one per dataset).

**Judge** (`judge.py`, blind to the arm): `claude-sonnet-5` sees the request,
the truth rows, the written answer and the chart image. It grades
`numbers_correct`, `chart_answers` and `readable` (1–5); `pass` = both true.
Each answer is judged `--votes 3` times. The majority counts, and an answer
whose calls split on pass is marked `unsure` (#1121: one call flipped 13 % of
verdicts on a re-judge). The report gives the judge's self-agreement. `unsure.py
export <results> <data>` writes the unsure answers, blind, for a second look by
a grader that recomputes from the data. `unsure.py apply <results>
<grades.jsonl>` writes its verdicts back (`resolved_by`).

**Asking back** (#1142). Customers ask vaguely, so the questions keep their
vague wording. A coverage task that can be meant more than one way lists its
**readings** (`make-coverage.py` `READINGS`: lead time to the first or the last
receipt, the best channel by revenue or by units, …), each with its truth SQL
and the user's reply that picks it. `ambiguity.py` labels every task phase from
how far the readings' results diverge: `clear` (one reading), `harmless` (same
rows, same order, numbers within 10 %) or `ambiguous` (another ranking, another
winner, numbers further apart). Every arm has an `ask_user` tool. The simulated
user answers with the default reading's reply, or with a go-ahead on a task
without readings. The judge grades against the reading the conversation settled
on, or else the one the answer states (the default if none). The report adds
**detection** (asked on ambiguous), **false asks** (asked on clear or
harmless) and the pass rate on ambiguous phases.

**Report** (`report.py`):
- per arm and phase: pass with a 95 % CI (see below), first-try chart, median and p90 tokens, output
  tokens, cost, tokens and cost per pass, calls and repairs;
- paired by task (anofox ÷ other): the geometric mean ratio with a bootstrap
  CI, Wilcoxon, McNemar for pass, and the pass-rate difference with a 95 %
  CI and a verdict;
- where the output went: exploration, chart, repair, answer, and the
  reference read per run.

**Intervals** (`stats.py`, #1122). Repeats of one task are correlated, so
the unit is the task. A pass rate's 95 % CI is a bootstrap that resamples
tasks with all their repeats ("boot", the one to read); a Wilson interval
over all runs is shown for reference ("Wilson": it counts each run as
independent and is too narrow when repeats agree). A difference (two arms in
one run, or a run against its baseline in `compare.md`) is paired by task:
each task's pass rate over its repeats, now minus then, averaged over the
tasks both have, with a bootstrap CI over those tasks. The verdict is
**better** or **worse** only when the whole CI lies above or below 0, else
**within noise**; `compare.py` counts "worse" as a regression. Claims of
progress need `--reps 3`: at 50 tasks a CI is still about ±10 points.

A fourth arm, `anofox-lean`, is anofox with a lean reference
(`anofox-reference.mjs --lean`, rules fixed before any results). It keeps one
role source, the guide's Roles table without its dashboard-only rows, plus the
guide's sections on design, tables, heatmaps and gotchas: ≈29 k characters,
the size of the Vega-Lite reference. Since #1100 it keeps the inputs too (the
Inputs rows and section, ≈8 k more): the dashboard questions ask for a filter,
and without them the 'All' default never reached the model.

## The reference sizes differ

The anofox reference is about 4× the others (≈104 k characters against 22 k
and 29 k): the product's guide covers whole dashboards, not just one chart.
It is what the product sends, so the main run uses it. Its cost shows apart
from the rest ("reference read") and is mostly cache reads at a tenth of the
input price. A leaner reference is a separate question.

## Running it

```sh
python -m venv .venv && .venv/bin/pip install -r bench/viz-tokens/requirements.txt
.venv/bin/python bench/viz-tokens/make-data.py ../scratch-viz-bench/data
cargo build --release --features serve --bin dashboard     # the anofox arm's renderer
# a plumbing check, no API:
.venv/bin/python bench/viz-tokens/harness.py --dry --data ../scratch-viz-bench/data --out /tmp/viz-dry
# the pilot (5 tasks × 3 arms × 3 repeats, with follow-ups):
ANTHROPIC_API_KEY=… .venv/bin/python bench/viz-tokens/harness.py --data ../scratch-viz-bench/data --out ../scratch-viz-bench/results/pilot
ANTHROPIC_API_KEY=… .venv/bin/python bench/viz-tokens/judge.py --data ../scratch-viz-bench/data ../scratch-viz-bench/results/pilot
.venv/bin/python bench/viz-tokens/report.py ../scratch-viz-bench/results/pilot
```

`--tasks all` runs all 20, and `--python` names the interpreter for the
sandboxed code (default: the one running the harness). `--python-ref` /
`--vega-ref` point at the skill folders (default `~/.claude/skills/matplotlib`
and `…/altair`, which are not in this repository), and `--pkg` is the wasm
build for the role reference (default `web/pkg`). Needs `bwrap`,
`systemd-run --user`, `rsvg-convert` and `node`.

The harness counts each arm's prefix tokens and asks before it spends
(`--yes` skips the question).

## One command (#1074)

```sh
ANTHROPIC_API_KEY=… BENCH_PYTHON=.venv/bin/python bench/viz-tokens/run.sh --set smoke   # ≈ 18 questions × 1, ≈ $0.65
ANTHROPIC_API_KEY=… BENCH_PYTHON=.venv/bin/python bench/viz-tokens/run.sh --set full    # 50 × 3, ≈ $10
bench/viz-tokens/run.sh --dry                                                           # the plumbing, no API
```

It builds the release CLI and `web/pkg` (unless `$BENCH_DASHBOARD` / an existing
build), the data (unless `$BENCH_DATA`), runs the anofox-lean arm, judges, writes
`report.md` and `gaps.md`, then `compare.md`: pass rate per phase, family and task
against `--baseline` (default: the previous run of the same set), with the
regressions — a task down from ≥ 2 passes to ≤ 1, a phase more than 10 points
lower, or a phase worse beyond noise (its difference's CI below 0). Each phase
gets its CI and a verdict line: better / worse / within noise. One row per run goes to `$BENCH_RESULTS/history.csv` (default
`target/bench/`). Exit status 2 on a regression. The sets are in `sets.json`;
`compare.py` also runs on its own (`compare.py <run> --baseline <run>`).
