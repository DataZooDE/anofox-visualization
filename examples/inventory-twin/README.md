# Where will stock run short, and can we trust the plan?

A use-case dashboard for [inventory-rs](https://github.com/DataZooDE/inventory-rs)
(private), the inventory core of DataZoo's digital twins: it ingests sales,
stock and purchase orders, builds one twin per item, steps the stock with
every event, projects it, watches its own forecast for drift, recalibrates
itself and replays history against the stock counts. The dashboard shows the
results of one such run. It is the "Where will stock run short, and can we
trust the plan?" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Inventory).

**Who asks:** the replenishment planner of a wholesaler on the evening of
28 Jun 2024, with twins for 12 items (classes A / B / C; smooth, erratic,
intermittent and lumpy demand; two suppliers). Which items will run out in the
next four weeks, and is the twin behind that answer still right?

**The story (item SKU-004):** demand rises by a third in mid-May; the order
placed then arrives five days late. Page-Hinkley and CUSUM on the forecast
error warn and alarm, the stock runs out for 7 days, the alarm-triggered
calibration proposes a +32 % demand bias, which is too large for guarded
auto and is escalated; the planner approves it and the raised reorder point
keeps the item in stock from then on. The stock chart steps through it
(`::STORY`).

## How critical is an item?

One rule, the macro `it_crit()` in the SQL, at the plan level chosen:

- **Critical**: a chance of 50 % or more of at least one day out of stock in
  the next four weeks, and the item can run out within 7 days (the plan band's
  lower edge reaches zero) or it is a class A item. The export has no prices,
  so the class (A = top sellers) stands for value.
- **Watch**: a chance of 20 % or more.
- **OK**: below 20 %.

Most critical first: Critical, Watch, OK; within each, the earliest day the
item can run out, then the chance. Separately, **plan trust**: *alarm* when a
drift detector is latched in alarm now, *review* when a recalibration waits for
the planner or a stock count did not match the twin, else *ok*. In this run
SKU-033, SKU-021, SKU-019, SKU-003 and SKU-002 are Critical (SKU-033's twin is
in alarm), six items are on Watch and SKU-004 is OK.

## Tabs and source tables

Inputs: the item (`::DROPDOWN`, with its class and demand shape as a hint) and
the plan level (`::BUTTONS` 90 / 80 / 50 %, the projection's quantile band).

| Tab | Shows | Export table | inventory-rs issue |
|---|---|---|---|
| header | the answer (the critical items) and fleet KPIs with thresholds | all | |
| Overview (opens first) | every item in one view: a risk map (days until it can run out × chance out in 4 weeks, threshold lines at 20 / 50 %, a click selects the item), when each item runs out (`::HEATMAP` item × next 28 days of cumulative stockout chance), the watch list most critical first (status, chance, by when, units short, plan trust, days of cover, projected stock `::SPARKLINE`, a link to the item) | `projection`, `trajectory`, `detector_status`, `parameter_changes`, `proposals`, `replay_deviation` | [#22](https://github.com/DataZooDE/inventory-rs/issues/22), [#23](https://github.com/DataZooDE/inventory-rs/issues/23) |
| Stock & pipeline | on hand / on order (`::STEP`), stockout spells (`::MARKAREA`), the incident story, strips (`::TRACK`) for alarms, orders and deliveries, parameter changes; served vs lost demand | `trajectory`, `alarms`, `parameter_changes` | [#22](https://github.com/DataZooDE/inventory-rs/issues/22), [#23](https://github.com/DataZooDE/inventory-rs/issues/23) |
| Risk | nested quantile fan (three `::BAND_LOWER` / `::BAND_UPPER` pairs), daily and cumulative stockout probability, every item's risk at the plan level (a link opens its fan) | `projection` | [#22](https://github.com/DataZooDE/inventory-rs/issues/22) |
| Monitoring | alarms by detector and class · shape (`::HEATMAP`), per week; CUSUM and Page-Hinkley paths with their thresholds (`::REFLINE`), shaded while in alarm; every detector's status | `alarms`, `detector_trace`, `detector_status` | [#23](https://github.com/DataZooDE/inventory-rs/issues/23), [#26](https://github.com/DataZooDE/inventory-rs/issues/26) |
| Calibration | the reorder point as calibration moved it; every proposal before / after with its lineage (independent vs twin-influenced evidence); alarms and changes on one timeline (`anofox_plot_events`); what waits for a decision | `proposals`, `parameter_changes`, `alarms` | [#23](https://github.com/DataZooDE/inventory-rs/issues/23) |
| Replay | twin vs counted stock (`anofox_plot_prediction`), matches per item | `replay_deviation`, `trajectory` | [#23](https://github.com/DataZooDE/inventory-rs/issues/23) |
| Pipeline | the run as a diagram (`::DIAGRAM`) and its manifest | `run_manifest`, `run_edges` | [#28](https://github.com/DataZooDE/inventory-rs/issues/28) |

The column layout of every table is the one the issues specify (e.g.
`projection`: `model_id, period, ds, yhat, level, yhat_lower, yhat_upper,
mean_on_hand, mean_backlog, stockout_probability,
cumulative_stockout_probability, expected_unmet, order_probability, split,
measure`). `detector_status.state` is the latched `Level` of `TestStatus`.

## Provenance: synthetic until the export lands

`end_to_end_demo --export-dir` ([inventory-rs#27](https://github.com/DataZooDE/inventory-rs/issues/27))
does not exist yet. Until it does, the tables are **synthetic**:
`synthetic.py` (Python standard library, seed 42) writes them in exactly the
specified schemas — 12 made-up items, periods 60–179 of a daily calendar
anchored at 2024-01-01 (as the demo's), a min/max lost-sales twin, CUSUM /
Page-Hinkley / occurrence-CUSUM / PSI / KS / MAE detectors with the demo's
thresholds, calibration every 30 periods and after alarms (guarded auto for
classes A and B, propose only for C), weekly stock counts and a 400-path
Monte-Carlo projection. Nothing in it is inventory-rs output.

`web/data/inventory-twin/provenance.csv` records the source: `source,
crate_version, git_sha, schema_version, export_version, seed, generator` —
today `synthetic`, seed 42; from the real export, the inventory-rs version,
git sha, `SCHEMA_VERSION`, `export_version` and seed. The dashboard page does
not show it.

| File | Role |
|---|---|
| `../../scripts/gen-inventory-cases.sh` | Runs inventory-rs's `end_to_end_demo --export-dir` when the checkout (`INVENTORY_RS`, default `../../rust/inventory-rs` next to this repo) has it, else `synthetic.py`; writes the CSVs and `provenance.csv` to `web/data/inventory-twin/` (≈ 470 KB). |
| `synthetic.py` | The documented synthetic stand-in. |
| `showcase.sql` | A link to `web/examples/inventory-twin.sql`, the dashboard. |

```sh
scripts/gen-inventory-cases.sh
cd web && ../target/release/dashboard --check --strict examples/inventory-twin.sql
```

When the real export replaces the synthetic tables, the SQL stays the same;
only the numbers in this README's story may change.
