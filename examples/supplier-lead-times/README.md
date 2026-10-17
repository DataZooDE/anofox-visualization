# How reliable are our suppliers, and what does it cost us in stock?

A use-case dashboard on supplier lead times, built on the concepts of
[inventory-rs](https://github.com/DataZooDE/inventory-rs) (private), the
inventory core of DataZoo's digital twins: purchase-order history and
receipts, lead time as inventory-calibration measures it, the lead-time
drift monitor, calibration of the planned lead time and King's safety-stock
formula. It sits next to the
[inventory twin](../inventory-twin/) in the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Inventory).

**Who asks:** the purchasing lead of a distributor at the end of Q3 2026,
buying 15 items from 6 suppliers. Which suppliers keep their promised lead
time, is any of them getting slower, and how much safety stock (and fill
rate) does their unreliability cost?

**The story (supplier Castings):** a furnace relining from 16 Mar 2026 takes
one of its two lines down and its lead time drifts from about 13.5 to 20
days. The lead-time CUSUM alarms on 28 Apr; the alarm-triggered calibration
moves the planned lead time only a small step (13.8 → 14.6 days, applied on
its own). A second alarm in late May proposes 14.6 → 18.1 days, too large to
apply on its own; the planner approves it on 3 Jun and the reorder points and
safety stock rise with it. The *Over time* chart steps through it
(`::STORY`).

## Tabs

Inputs: the supplier (`::DROPDOWN`, its region and promise as a hint) and the
service level (`::BUTTONS` 95 / 90 / 98 / 99 %).

| Tab | Shows |
|---|---|
| header | the question and its answer; on-time share, days late when late, lead-time variability (CV) and the safety stock held for lead-time variability, each with thresholds and the change against the previous quarter |
| Reliability | days late or early against the promise per supplier (`::BOXPLOT`), promised vs actual lead time (median and 10–90 % range), every supplier with a monthly sparkline; a click on a supplier opens its orders (`::LINK`) |
| Over time | the selected supplier's weekly lead-time median with nested 95 / 80 / 50 % bands (`::BAND_LOWER` / `::BAND_UPPER`), the lead time planning assumed, the drift story (`::STORY`), strips (`::TRACK`) for late lines, disruptions and the drift monitor; the CUSUM with its warning and alarm lines; the calibration proposals |
| Safety stock | King's formula in TeX, $SS = z\sqrt{\bar L\sigma_D^2+\bar D^2\sigma_L^2}$, with the current values; safety-stock value per supplier split into what demand variability and what lead-time variability needs, and the sales value not served from stock with each plan — both move (`'tween'::TRANSITION`) when the service level changes; every item |
| Orders | the selected supplier's lines by order day, what is on order now (overdue against the promise), every closed line |

## Definitions

- **Lead time** of a line: the quantity-weighted mean receipt day minus the
  order day — inventory-calibration's default rule
  (`LeadTimeRule::WeightedMeanReceipt`). Lines with several receipts count
  once.
- **On time:** the line's last receipt arrives by `promised_at`.
- **Variability (CV):** standard deviation / mean of a supplier's lead time.
- **Safety stock:** King's formula (inventory-analytics
  `safety_stock_kings`) with $\bar L$, $\sigma_L$ from the lines closed in
  the quarter and $\bar D$, $\sigma_D$ per item; against $z\sigma_D\sqrt{\bar L}$,
  the stock a fixed lead time would need.
- **Fill rate** per order cycle of one review period: $1 - \sigma\,G(k)/Q$,
  with $\sigma$ the spread of demand over the real lead time, $k$ the safety
  stock in units of $\sigma$ and $G$ the standard normal loss function.
- **Drift monitor:** a one-sided CUSUM of (actual − assumed lead time) /
  assumed spread, slack 0.5, warning at 3, alarm at 5 — the
  `lead_time_error` stream of inventory-monitor.

## Data and provenance: synthetic

`synthetic.py` (Python standard library, seed 7, deterministic) writes the
tables to `web/data/supplier-lead-times/` (about 200 KB):

| Table | Layout |
|---|---|
| `po_history` | inventory-rs contract v1 `po_history`: `po_id, line_id, supplier_id, sku_id, node_id, ordered_at, ordered_qty, promised_at, status, cancelled_qty, uom` |
| `po_receipt` | contract v1 `po_receipt`: `po_id, line_id, sku_id, receipt_id, received_at, received_qty, uom` |
| `items` | `sku_id, supplier_id, description, demand_mean, demand_std_dev, unit_cost, review_days` |
| `suppliers` | `supplier_id, name, region, promised_days` |
| `disruptions` | `supplier_id, start_date, end_date, kind, what` |
| `lead_time_monitor` | per closed line: `lead_time_days, assumed_lead_time, assumed_std_dev, lead_time_error, cusum, warning_threshold, alarm_threshold, state` |
| `proposals` | calibration of `supplier.lead_time_mean` / `supplier.lead_time_std_dev`: `old_num, new_num, sample_size, confidence, mode, status, decided_at, trigger, reason` |
| `provenance` | `source, seed, generator, schema` |

Six made-up suppliers (a reliable domestic one, an optimistic one that is
usually a day late, a right-skewed EU one, a sea-freight one hit by shutdowns,
port congestion and a typhoon, and the drifting one) and 15 made-up items,
orders from Jan 2025 to Sep 2026. Nothing in it is inventory-rs output or
real data. inventory-rs has no export of lead-time history yet; the input
tables are what a customer would feed it, so a real export would replace
`po_history` / `po_receipt` and the dashboard SQL stays the same.

Regenerate:

```sh
python3 examples/supplier-lead-times/synthetic.py web/data/supplier-lead-times
```

Check natively, from `web/`:

```sh
dashboard --check --strict ../examples/supplier-lead-times/showcase.sql
```
