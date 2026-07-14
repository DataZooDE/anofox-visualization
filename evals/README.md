# Dashboard evals

A tiny scorecard for **AI-authored dashboards** — so you can tell whether a
prompt/skill change actually improves results instead of guessing.

Each case in `cases/` is a **self-contained** `.sql` dashboard (it generates its
own data, so it runs standalone). The runner scores each on two axes:

- **Correctness** — `dashboard --check` (any lint error → score 0, a hard fail).
- **Taste (rubric heuristics)** — starting from 100: penalties for no KPI tiles,
  no filter, untitled charts, and too few / too many panels. These are the same
  rules the `build-dashboard` skill's Design rubric asks for.

## Run

```sh
cargo build --bin dashboard          # needs the duckdb CLI on PATH
python3 evals/run.py                 # scores evals/cases/*.sql
python3 evals/run.py /path/to/dir    # or score a directory of candidates
```

Exit code is non-zero if any case has lint errors.

## Measure a change

1. Note the current mean score.
2. Change the prompt / skill / model, regenerate the candidate dashboards into a
   directory, and run `python3 evals/run.py <dir>`.
3. Compare. The three seeded cases show the spread the scorer detects — a
   well-composed overview (100), a decent KPI board (90, no filter), and a
   valid-but-mediocre answer that lints clean yet scores 44.

## Add a case

Drop a `<name>.sql` in `cases/` with a `-- request: "…"` comment describing what
was asked. Keep it self-contained (`CREATE TABLE … AS VALUES/range(...)`) so it
lints without external data.
