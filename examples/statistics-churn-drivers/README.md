# Why do customers cancel, and which lever pays off? (drivers of churn)

**Who asks:** the head of customer success of a SaaS company, planning the Q4
retention budget after five quarters of rising churn. A live anofox_statistics
use case; in the [live demo](https://datazoode.github.io/anofox-visualization/)
it is "Why do customers cancel?".

`showcase.sql` is a link to `web/examples/statistics-churn-drivers.sql`, the
single source the browser demo loads. The setup builds 5,000 synthetic
customers (hash-based, no files) with planted drivers — contract, failed
payments, support tickets, usage drop, onboarding, tenure — and four
attributes that play no part (mobile app, newsletter, region, price). The
dashboard computes everything live:

| Tab | What it answers | Functions |
|---|---|---|
| Overview | churn trend, the drivers as odds ratios (n.s. terms fade) | `logistic_tidy_by` → `anofox_plot_terms(reference := 1)` |
| Drivers | how big each effect is, as odds and probabilities | `logistic_tidy_by`, `logistic_effects_by` → `anofox_plot_prediction` |
| Model check | can the score be trusted on unseen customers | `logistic_fit_predict_by` (hold-out), `roc_curve_by`, `calibration_curve_by` → `anofox_plot_curve`, `auc_agg`, `brier_agg`, `logistic_glance_by`, `vif_agg` |
| What-if | what four retention levers are worth (net of the annual discount) | the fitted coefficients, re-scored in SQL from the inputs |
| Action list | risk bands and the 25 accounts to call first, recommendation | the above |

The what-if reads the model's coefficients as if they were causal; the
dashboard says so and treats the numbers as an upper bound. Lint it natively
from the repo root (a `duckdb` on `PATH` that can load the unsigned
extension, see the A/B test README):

```sh
cargo run --bin dashboard -- --check examples/statistics-churn-drivers/showcase.sql
```
