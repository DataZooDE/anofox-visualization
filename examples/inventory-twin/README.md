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
28 Jun 2024. inventory-rs's demo run keeps a twin for each of 50 items (value
classes A / B / C) and exports the day-by-day detail —
trajectory, projection, detector trace — for 12 of them (SKU-004, SKU-008, …
SKU-048); fleet tables (alarms, detectors, calibration, replay) cover all 50.
Which items will run out in the next four weeks, and is the twin behind that
answer still right?

**The story (item SKU-004, class A):** demand steps up in May; from 5 May the
MAE, CUSUM and Page-Hinkley detectors on the forecast error and KS / PSI on the
demand warn and then alarm (10 in all); the item is out of stock on 6 days.
Calibration re-estimates the twin every 30 days and after alarms: 7 changes
are in effect, and the large demand-bias and spread updates are escalated to
the planner instead of applied on their own. The stock chart steps through it
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
drift detector is latched in alarm now, *review* when a stock count did not
match the twin or a recalibration needs the planner (an escalated proposal, or
any waiting for a class A / B item), else *ok*. Class C proposals always wait
for the planner by design; they are listed as routine and do not make an item
*review*. The risk map colours each item by status with
`'Critical:bad; Watch:warn; OK:good'::COLORS` (the theme's status colours). In this run
SKU-040, SKU-044, SKU-032, SKU-028, SKU-048, SKU-008, SKU-036 and SKU-020 are
Critical, four items (SKU-024, SKU-004, SKU-016, SKU-012) are on Watch, and 2
twins are in alarm.

## Tabs and source tables

Inputs: the item (`::DROPDOWN`, with its class and demand shape as a hint) and
the plan level (`::BUTTONS` 90 / 80 / 50 %, the projection's quantile band).

| Tab | Shows | Export table | inventory-rs issue |
|---|---|---|---|
| header | the answer (the critical items) and fleet KPIs with thresholds | all | |
| Overview (opens first) | every item in one view: a risk map (days until it can run out × chance out in 4 weeks, coloured by status, threshold lines at 20 / 50 %, a click selects the item), when each item runs out (`::HEATMAP` item × next 28 days of cumulative stockout chance), the watch list most critical first (status, chance, by when, units short, plan trust, days of cover, projected stock `::SPARKLINE`, a link to the item) | `projection`, `trajectory`, `detector_status`, `parameter_changes`, `proposals`, `replay_deviation` | [#22](https://github.com/DataZooDE/inventory-rs/issues/22), [#23](https://github.com/DataZooDE/inventory-rs/issues/23) |
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

## Provenance

The tables are the real export of one inventory-rs run:
`end_to_end_demo --export-dir` (inventory-rs 0.2.0, export version 1, seed 42;
[inventory-rs#22–#28](https://github.com/DataZooDE/inventory-rs/issues/27)),
periods 60–179 of a daily calendar anchored at 2024-01-01.
`web/data/inventory-twin/provenance.csv` records it (`source, crate_version,
git_sha, schema_version, export_version, seed, generator, float_decimals,
per_period_skus`); the dashboard page does not show it. The export has no
demand shape (smooth / erratic / …) yet, so the dashboard shows the value
class alone. Without an inventory-rs checkout the script falls back to
`synthetic.py`, a documented stand-in with the same schemas (seed 42; its
provenance says `synthetic`).

| File | Role |
|---|---|
| `../../scripts/gen-inventory-cases.sh` | Runs inventory-rs's `end_to_end_demo --export-dir` when the checkout (`INVENTORY_RS`, default `../../rust/inventory-rs` next to this repo) has it, else `synthetic.py`; writes the CSVs and `provenance.csv` to `web/data/inventory-twin/` (≈ 1.5 MB). |
| `synthetic.py` | The documented synthetic stand-in (no checkout). |
| `showcase.sql` | A link to `web/examples/inventory-twin.sql`, the dashboard. |

```sh
scripts/gen-inventory-cases.sh
cd web && ../target/release/dashboard --check --strict examples/inventory-twin.sql
```

A new inventory-rs run changes the numbers in this README's story; the SQL
reads every one of them from the tables.
