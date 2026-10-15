# Decide showcase: can a decision model triage the support inbox?

[anofox-decide](https://github.com/DataZooDE/anofox-decide) answers
natural-language questions about text inside DuckDB. Here it answers three
questions about 92 synthetic support tickets in English, German and French —
*does the customer ask for money back?* (binary), *which team owns it?*
(choice) and *how frustrated is the writer?* (score) — and the dashboard
measures the answers against labels a support lead gave. It's the "Stored
results: support inbox triage" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Decide).

## Stored results, not live

Answers need a decision model. This example uses the local
**laya-multilingual** model (ONNX Runtime on the CPU, a 0.68 GB download,
offline afterwards). Neither the model nor ONNX Runtime runs in DuckDB-Wasm,
and anofox_decide has no DuckDB-Wasm build, so the answers are computed
natively and the dashboard reads them:

| File | Role |
|---|---|
| `compute.sql` | The ticket templates and labels, and one `decide_answers` call per ticket (all three questions in one request); `dc_scored` (one row per ticket) and `dc_metrics` (`decide_accuracy`, `decide_brier_score`, `decide_ece`). |
| `../../scripts/gen-showcase-data.sh decide` | Runs `compute.sql` natively and writes the tables to `web/data/decide/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/decide-showcase.sql`, the single source of the dashboard. It reads `dc_scored.csv` and computes every metric in SQL, live, for the controls on the page (the header says the answers are stored). |

Regenerate (DuckDB v1.5.6, `-unsigned`, from the repository root; about 30 s
on 20 cores):

```sh
duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"                  # once
duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"     # once
DUCKDB=duckdb scripts/gen-showcase-data.sh decide
cd web && ../target/release/dashboard --check ../examples/decide-showcase/showcase.sql
```

## Controls

The **Decision rules** box drives every panel, in SQL, over the stored answers:

- `refund_at_pct` — escalate a ticket as a refund when P(refund) reaches it;
- `missed_refund_min`, `false_alarm_min` — what each mistake costs in agent
  minutes; the cost curve marks the cheapest threshold next to yours;
- `autoroute_at_pct` — the confidence a team choice needs to be applied
  without a human;
- `language`, `team` — filter the tickets. A click on a team bar (Routing)
  lists that team's routing mistakes.

## Tabs

1. **Refund queue** — agent minutes lost by threshold (cheapest vs yours),
   the confusion at your threshold and every mistake.
2. **Calibration** — calibration and ROC (`anofox_plot_curve`), precision and
   recall by threshold, P(refund) per ticket and language, and Brier score /
   calibration error per language.
3. **Routing** — right team per labelled team, the confusion matrix, what a
   confidence gate buys (coverage drops, accuracy doesn't rise) and the
   routing mistakes.
4. **Frustration** — the score by labelled level (calm is separated from
   upset; annoyed and angry are not) and a call-back-first queue.
5. **All tickets** — every ticket with labels, answers and verdicts.
