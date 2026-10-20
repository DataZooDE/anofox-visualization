# The assistant's eval seed (#244, M1-3)

A small, fixed set of prompts that exercise the assistant end to end against
the **real** Anthropic API — never mocked. This is a measurement, not a pass/
fail gate: `scripts/assist-eval.mjs` runs each prompt, lints the final SQL,
checks the roles/safety expectations in `prompts.jsonl`, and prints a summary
(runs %, first-try %, roles %, safety %, total cost).

## Running it

```sh
ANTHROPIC_API_KEY=sk-ant-… node scripts/assist-eval.mjs --yes
```

Needs `tests/web/node_modules` (`npm install --prefix tests/web`) and a built
`web/pkg` (`wasm-pack build --target web --out-dir web/pkg --no-default-features
--features wasm`). Add `--case <id>` to run one prompt, `--model harder` to
use the "try harder" model, `--dashboard target/debug/dashboard` to also run
the native `dashboard --check --strict` on the result.

`--dry` validates the seed itself (no API key, no network, no cost) — this is
what `tests/web/assist_eval_seed_test.mjs` runs in CI.

## Cost

About $0.30 per prompt on Sonnet 5 (≈ $3 for all 10), about $0.52 per prompt
on Opus 5.5. `scripts/assist-eval.mjs` prints the estimate before it spends
anything, and refuses to run without `--yes`.

## Results are not committed (M1-3, D4)

`results/` is gitignored. The owner runs the seed once with their own key and
pastes the summary into the PR. A later milestone (M3-3) adds `results/latest/`
and wires this into CI.
