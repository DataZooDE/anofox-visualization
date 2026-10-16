# Decide use case: which claims can we fast-track, and which go to fraud review?

**Asked by** the head of claims operations and the fraud team (SIU) lead of a
made-up insurer. **The decision:** which new claims are paid at once (fast
track), which go to an adjuster and which to fraud review — and what reading
the claimant's own description adds to the four intake rules.

[anofox-decide](https://github.com/DataZooDE/anofox-decide) answers
natural-language questions about text inside DuckDB. Here it reads 320
synthetic first notices of loss (motor, home, travel; English and German,
eight weeks), each with its outcome after investigation. It's the "Stored
results: insurance claim routing" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Decide).

## The story

1. **Overview** — where the claims go under *rules only* and *rules +
   description*, fraud reviews per week against the team's capacity, and the
   cost per claim by fraud-review threshold (cheapest marked, yours dashed).
2. **Fraud signals** — how well each signal separates fraud on its own
   (AUROC), the ROC of both scores, P(fraud) per claim and the model's raw
   answers scored with `decide_accuracy`, `decide_brier_score`, `decide_ece`.
   A concrete question (*"Does the claimant provide evidence?"*) beats a vague
   one (*"Is this claim suspicious?"*), and raw answers need calibration.
3. **Fraud review queue** — the queue at your thresholds with each claim's
   reasons (weights in log-odds points), capacity badges and the back-tested
   outcome.
4. **Fast track** — share fast-tracked per line of business, why a claim is
   not fast-tracked, and the fraud that is paid without a review.
5. **All claims** — the last four weeks by queue (subtotals, totals).
6. **Decision** — a recommendation (the cheapest threshold the fraud team can
   handle, the yearly saving against the rules alone) and the options side by
   side.

## How the score works

Four intake rules (policy younger than 90 days, reported more than 14 days
late, two or more prior claims, amount above 8,000 €) and three answers of the
model about the description (*no evidence*, *new items or raised cover*,
*pushes for a fast payout*) each add a weight of evidence (log-odds points) to
the base fraud rate: the rules from their hit rates, the answers through a
Platt fit with `decide_fit_calibration`. The sum is P(fraud); the reasons on
the page are its largest terms. The weights are fitted on the same 320
investigated claims the page back-tests on — a demo shortcut; in production
fit them on last quarter and test on this one.

## Controls

- `siu_at_pct` — fraud review from this P(fraud);
- `fast_below_pct`, `fast_limit_eur` — fast track below this P(fraud) and up
  to this amount (claims with an injury never go fast);
- `siu_capacity` — fraud reviews the team can do per week;
- `lob` — lines of business.

Costs per claim (in the SQL): handling 40 € fast track, 220 € adjuster, 900 €
fraud review; an adjuster catches 30 % of fraud, fraud review 90 %, the fast
track none (undetected fraud is paid out); a genuine claimant held in fraud
review costs 300 € of goodwill.

## Stored results, not live

The answers need a decision model. This example uses the local
**laya-multilingual** model (ONNX Runtime on the CPU, a 0.68 GB download,
offline afterwards). Neither the model nor ONNX Runtime runs in DuckDB-Wasm,
and anofox_decide has no DuckDB-Wasm build, so the answers are computed
natively and the dashboard reads them:

| File | Role |
|---|---|
| `compute.sql` | The claim generator (fragments, structured intake fields, the investigated outcome), one `decide_answers` call per claim (four questions in one request), the signal weights (`decide_fit_calibration`) and `cl_metrics` (`decide_accuracy`, `decide_brier_score`, `decide_ece`). |
| `../../scripts/gen-decide-cases.sh claims` | Runs `compute.sql` natively and writes `cl_scored`, `cl_signals`, `cl_metrics` to `web/data/decide-claims/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/decide-claims.sql`, the single source of the dashboard. Routing, queues, costs and metrics are computed in SQL, live, from the stored answers. |

Regenerate (DuckDB v1.5.6, `-unsigned`, from the repository root; about a
minute on 20 cores):

```sh
duckdb -unsigned -c "INSTALL anofox_decide FROM 'https://get.erpl.io'"                  # once
duckdb -unsigned -c "LOAD anofox_decide; CALL decide_download('laya-multilingual')"     # once
DUCKDB=duckdb scripts/gen-decide-cases.sh claims
cd web && ../target/release/dashboard --check ../examples/decide-claims/showcase.sql
```
