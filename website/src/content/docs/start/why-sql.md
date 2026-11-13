---
title: Why SQL for AI-made charts
description: When an AI answers a data question with a chart, annotated SQL against generated Python — where each one wins.
---

An AI assistant that answers "which region sells the most?" with a chart has to write *something*: pandas and matplotlib code, a Vega-Lite spec, or anofox-visualization's annotated SQL. Here is where annotated SQL wins, and where Python does.

A benchmark that measures the first two points of each list, with the same model, data and questions for every approach, is under way ([#1044](https://github.com/DataZooDE/anofox-visualization/issues/1044)). Its results will be added here.

## Where SQL with `::ROLE` wins

1. **Less to write, so fewer tokens.** One statement replaces 15–40 lines of pandas and matplotlib: reshaping the data, the plotting calls, axis formats, legend, title, saving. Output tokens cost several times more than input tokens with most model providers, and they are what the user waits for.
2. **Edits are cheap.** "Split that by channel" is one more column, `channel::CATEGORY`, and a stacked bar. A Python answer is usually rewritten as a whole.
3. **The design lives in the engine, not in every prompt.** Number formats, colours, label placement, empty states and themes come from [ggplot-rs](https://github.com/sipemu/ggplot-rs). A Python assistant has to restate a house style in every answer, and each answer can still look different. Annotated SQL looks like one product.
4. **No code execution.** It runs SQL in DuckDB, never a program. Running generated Python safely needs a sandbox for every answer, which is infrastructure and attack surface of its own.
5. **The governance applies.** The same SQL goes through the [locked server](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md)'s policies, row filters, query limits and cache, and Ask answers within what each viewer may see. A Python chart sits outside all of that.
6. **The result is reusable.** An annotated `SELECT` is a panel: it can join a dashboard, be served, be exported as PNG or JSON, go out as a scheduled report or alert, or be posted to Teams or Slack. A matplotlib script makes one picture.
7. **It re-runs and it can be checked.** The same SQL on new data draws the same chart, and `dashboard --check` finds errors before anything is drawn. Python errors show only when the script runs.
8. **The data stays in DuckDB.** The database aggregates; nothing is pulled into a data frame first, which matters for large tables.

## Where Python wins

1. **Models already know it.** Python and matplotlib are all over the training data; the role language is not, so the assistant needs a reference in its prompt. Prompt caching makes that reference cheap to repeat, and a lean, chart-only version of it is part of the benchmark.
2. **Range.** Any statistics, any custom chart, models and simulations. Annotated SQL draws what the roles express; the [anofox extensions](../../integrate/) cover much of the statistics and forecasting.
3. **Familiar to analysts.** People can read the code and change it themselves.
4. **No engine needed.** A Python chart runs wherever Python runs.

## In short

To ask a business question and get a correct, consistent, governed chart that can become part of a product, annotated SQL should win on cost, consistency, safety and reuse. For open-ended analysis, Python wins. The two work as a pair: annotated SQL for everyday answers, and statistics extensions or sandboxed Python for the rare question the roles can't express.
