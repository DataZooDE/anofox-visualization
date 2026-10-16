# Decide use case: who should sales call first this week?

**Asked by** the head of sales development of a made-up warehouse-software
vendor, "Stockline", every Monday. **The decision:** which inbound messages go
to the sales team, in which order the reps call them, and what to open each
call with. Today a coordinator sorts the shared inbox by hand and the reps
call sales enquiries first come, first served.

[anofox-decide](https://github.com/DataZooDE/anofox-decide) answers
natural-language questions about text inside DuckDB. Here two local decision
models read 300 synthetic inbound messages (web form, e-mail, chat; six
weeks) and answer four questions each: which team should handle it (a
`choice` with described options), is a budget approved, is the writer a
senior decision maker, do they want to decide this quarter (`binary`). The
CRM holds what the reps found out later and which messages became
opportunities. It's the "Stored results: lead routing" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Decide).

## The story

1. **Overview** — opportunities found by calls a week for the lead score,
   today's arrival order and a hindsight upper bound; the deal rate by score
   band; a funnel from inbox to opportunity at your settings; opportunities
   at your capacity per model (the model matters:
   laya-typed-decisions beats arrival order, laya-multilingual does not).
2. **Reasons** — the lead-score formula with your weights filled in, does each answer agree with the rep's finding (AUROC per
   reason and model), the chosen model's mean P(yes) split by the finding,
   and both models scored with `decide_accuracy`, `decide_brier_score`,
   `decide_ece`.
3. **Routing** — the rep's team against the model's choice, the sales queue
   by P(sales) threshold, and the lost leads (sales enquiries that miss it).
4. **This week's call list** — a recommendation and the top calls of the
   latest week, each with the reasons to open with and the back-tested
   outcome.
5. **All leads** — the last three weeks by queue (subtotals, totals).

## Controls

- `model` — laya-typed-decisions or laya-multilingual, a button pair (stored answers of
  both; the queries are the same, only the model differs);
- `sales_at_pct` — a message goes to sales when P(sales) reaches it;
- `calls_per_week` — rep capacity;
- `w_budget`, `w_authority`, `w_timeline` — the weights of the three reasons
  in the lead score (0–100, a weighted mean of the answers' probabilities).

## Stored results, not live

The answers need decision models. This example uses two local models,
**laya-typed-decisions** (English, 0.85 GB; the better router in the
extension's evaluation) and **laya-multilingual** (0.68 GB), both on ONNX
Runtime on the CPU and offline after the download. Neither the models nor
ONNX Runtime run in DuckDB-Wasm, and anofox_decide has no DuckDB-Wasm build,
so the answers are computed natively and the dashboard reads them:

| File | Role |
|---|---|
| `compute.sql` | The message generator (sentence fragments with known attributes, the CRM outcome), one `decide_answers` call per message and model (four questions in one request; P(sales) from the choice's `distribution`), and `ld_metrics` (`decide_accuracy`, `decide_brier_score`, `decide_ece` per reason and model). |
| `../../scripts/gen-decide-cases.sh leads` | Runs `compute.sql` natively and writes `ld_scored`, `ld_metrics` to `web/data/decide-leads/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/decide-leads.sql`, the single source of the dashboard. Routing, the lead score, the call list and every metric are computed in SQL, live, from the stored answers. |

Regenerate (DuckDB v1.5.6, `-unsigned`, from the repository root; about four
minutes on 20 cores):

```sh
duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"                        # once
duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"           # once
duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-typed-decisions')"        # once
DUCKDB=duckdb scripts/gen-decide-cases.sh leads
cd web && ../target/release/dashboard --check ../examples/decide-leads/showcase.sql
```
