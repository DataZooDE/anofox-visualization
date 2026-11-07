---
title: Design guide
description: What makes a dashboard good, not just correct — and which rules dashboard --check enforces.
---

The rules below come from Nielsen Norman Group, Tufte and Few, *Storytelling with Data* and Datawrapper. Rules marked **lint** are checked by `dashboard --check` (a `design/*` warning; `--strict` fails on it).

## Layout

- **Summary → trend → detail.** A KPI strip first, then the one chart that answers the main question (full width), then breakdowns two by two, then a detail table.
- **The most important thing top left.** Not a filter, not a logo.
- **At most ~7 panels per view**; split topics into `::TAB`s. **lint** `too-many-panels`
- **Group what belongs together** in a `::GROUP` box. KPI tiles in one strip. **lint** `ungrouped-kpis`
- **No raw row dumps.** A top-N or an aggregate; `::PAGED` for large remote data. **lint** `raw-table`

## Charts

| The question | Chart |
|---|---|
| rank or compare categories | a **sorted** [`::BARCHART`](../charts/barchart/) (`::FLIP` for long labels) |
| a value over time | [`::LINECHART`](../charts/linechart/), ≤ ~6 series |
| part of a whole | [`::BARCHART_STACKED`](../charts/barchart-stacked/); a [`::DONUTCHART`](../charts/donutchart/) only up to 5–6 slices |
| the distribution of a number | [`::HISTOGRAM`](../charts/histogram/), [`::BOXPLOT`](../charts/boxplot/) by group |
| two numbers | [`::SCATTER`](../charts/scatter/), a third as [`::BUBBLE`](../charts/bubble/) size |
| two categories and a measure | [`::HEATMAP`](../charts/heatmap/) |
| progress to a goal | [`::GAUGE`](../charts/gauge/), [`::BULLET`](../charts/bullet/), a KPI's `::GOAL` |
| geography | [`::REGION`](../charts/region/), [`::MAP`](../charts/map/), [points](../charts/lat/) |

- **Sort bars by value** unless x is time or an order. **lint** `unsorted-bars`
- **No pie past ~6 slices.** **lint** `pie-slices`
- **Title every chart with the takeaway**: `'Revenue up 12 % on last year'`, not `'Revenue'`. **lint** `untitled-chart`
- **At most ~7 colours**; keep the top few and bucket the rest as "Other", or use `::FACET`. **lint** `many-series`
- **Format the axis** (`'€'::YFORMAT`), **one accent colour**, and **never colour alone**: pair it with position or a label.

## Allowing an exception

When a chart breaks a rule on purpose, put `-- @lint_allow design/many-series` right above it: `--check` still notes it, `--strict` passes.

[The full guide, with layout recipes →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/dashboard-design.md)
