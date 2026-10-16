# Did the new checkout lift conversion? (A/B test readout)

**Who asks:** the head of e-commerce, before the Q4 release freeze: ship the
one-page checkout (B) or keep the three-step checkout (A)? A live
anofox_statistics use case; in the [live demo](https://datazoode.github.io/anofox-visualization/)
it is "A/B test readout".

`showcase.sql` is a link to `web/examples/statistics-ab-test.sql`, the single
source the browser demo loads. The setup builds 96,000 synthetic visitors
(1–21 Sep 2026, hash-based, no files) with a planted lift on mobile and tablet,
a novelty bump in the first days and a set of guardrail metrics. The dashboard
computes everything live:

| Tab | What it answers | Functions |
|---|---|---|
| Readout | lift + 95% CI, cumulative lift (peeking), revenue per visitor, verdict | `prop_test_two_agg`, `prop_test_one_agg`, `t_test_agg`, `chisq_gof_agg` |
| Segments | where the lift comes from, and whether it really differs | `prop_test_two_agg` → `anofox_plot_tests`, `logistic_tidy_by` → `anofox_plot_terms`, `eb_shrink_by` |
| Guardrails | did B break anything (pass / watch / fail) | `tost_t_test_agg`, `prop_test_two_agg`, `t_test_agg`, `anofox_plot_compare` |
| Power & sample size | was the test big enough | plain SQL (normal approximation; the extension has no power function) |
| Decision | rollout impact, checklist, recommendation | the above |

The inputs (device, the lift the test was sized for, the page-load tolerance)
re-run every test. Lint it natively from the repo root with a `duckdb` on
`PATH` that can load the unsigned extension (`INSTALL anofox_statistics FROM
'https://get.erpl.io'`, a wrapper running `duckdb -unsigned "$@"`):

```sh
cargo run --bin dashboard -- --check examples/statistics-ab-test/showcase.sql
```
