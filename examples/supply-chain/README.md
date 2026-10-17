# Where does our supply chain amplify demand, and what would stabilise it?

A supply-chain simulation dashboard: four stages pass goods down to consumers
and orders back up, and small swings in consumer sales grow into large swings
in orders further up the chain (the **bullwhip effect**, as in the beer game).
It is the "Where does our supply chain amplify demand, and what would
stabilise it?" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Inventory).

**Who asks:** the supply-chain lead of a consumer-goods maker who sells through
one retailer and one distribution centre (DC), and who buys a key component
from one supplier. Orders at the factory and the supplier swing far more than
consumer sales do. Where does the swing grow, what does it cost, and would
sharing sales data or smoothing orders calm it down?

## The model

The model runs over 52 weekly periods. Consumers buy from the **retailer**,
the retailer orders from the **DC**, the DC orders from the **factory**, and the
factory buys components from the **supplier**. The supplier buys raw
material, which never runs short.

| Stage | Lead time (weeks) | Ordering | Holding / backlog cost ($ per unit and week) |
|---|---|---|---|
| Retailer | 1 | base stock | 1.00 / 4.00 (a consumer waiting) |
| Distribution centre | 2 | base stock | 0.60 / 1.50 |
| Factory | 2 (1 component transport + 1 production), **capacity 150 units/week** | base stock | 0.40 / 1.50 |
| Supplier | 2 | base stock, raw material bought in lots of 50 | 0.20 / 1.50 |

The timing within a week follows inventory-rs's multi-echelon network twin
(`inventory-sim/src/network.rs`):

1. Shipments due this week arrive.
2. Stage by stage, starting at the retailer and moving up, each stage:
   - serves its backlog and then this week's incoming order from stock on hand;
   - ships what it can at once;
   - places its own order, which the stage above sees in the same week.

Unserved orders stay open as backlog at every stage, consumers included.

**Ordering.** Every stage follows an order-up-to (base-stock) policy with
proportional smoothing (POUT):

$$O_t = \hat D_t + \frac{1}{T_s}\Big(SS + \hat D_t\,(L-1) - IP_t\Big)$$

In this formula:

- $\hat D_t$ is the stage's demand forecast, from exponential smoothing with
  $\alpha = 0.25$.
- $SS = 1.65 \cdot 15 \sqrt{L+1}$ is the safety stock.
- $IP_t$ is the inventory position: on hand − backlog + pipeline.
- $T_s$ is the order smoothing: 1 (none), 2 or 4 weeks.

**Information policy (the "Who sees consumer sales" input):**

- **Own forecasts.** Each stage forecasts from the orders it receives. It also
  counts only 70 % of what is already on the way (Sterman's *supply-line
  underweighting*, the main cause of over-ordering in the beer game). The
  supplier buys raw material in full lots of 50 units.
- **Shared sales data (VMI).** Every stage plans on consumer sales (point-of-sale
  data, as in vendor-managed inventory) and sees its whole pipeline. The
  supplier is replenished continuously.

**Scenarios.** Every scenario uses the same random numbers: consumer noise
σ = 15 units/week around 100, and 6 % of the weeks with late component
deliveries. Only the shock differs:

- **Promotion:** demand rises 60 % in weeks 13–16, then falls 15 % below normal
  in weeks 17–18 because consumers bought ahead.
- **Supplier outage:** in weeks 19–26 the supplier can ship at most 35
  components a week.
- **Normal:** no shock.

**Bullwhip ratio** of stage $i$: $B_i = \operatorname{Var}(O_i) / \operatorname{Var}(D)$
over the 52 weeks, where $O_i$ is the order it places and $D$ is consumer
demand. The step factor $B_i / B_{i-1}$ is how much a stage amplifies the
orders it receives.

### Results (bullwhip ratio by stage, consumer fill rate, chain cost)

| Scenario | Policy | Smoothing | Retailer | DC | Factory | Supplier | Fill rate | Cost |
|---|---|---|---|---|---|---|---|---|
| Promotion | Own forecasts | none | 1.9 | 4.7 | 10.6 | **24.7** | 96.6 % | $10,698 |
| Promotion | Own forecasts | 4 weeks | 0.7 | 0.9 | 1.4 | 2.7 | 92.9 % | $9,343 |
| Promotion | Shared sales data | none | 1.5 | 3.0 | 4.7 | 6.5 | 94.9 % | $8,281 |
| Promotion | Shared sales data | 2 weeks | 1.0 | 1.3 | 1.5 | 1.7 | 94.1 % | $8,158 |
| Supplier outage | Own forecasts | none | 3.5 | 17.1 | 54.3 | **85.0** | 66.2 % | $66,890 |
| Supplier outage | Shared sales data | none | 1.6 | 3.2 | 5.5 | 8.3 | 66.8 % | $46,602 |
| Normal | Own forecasts | none | 1.6 | 4.5 | 14.4 | 40.4 | 99.8 % | $7,057 |
| Normal | Shared sales data | none | 1.6 | 3.2 | 5.5 | 8.3 | 99.8 % | $5,050 |

- **Sharing sales data** removes most of the amplification.
- **Smoothing** removes the rest, but it reacts later to a real change in
  demand: in the promotion, the consumer fill rate falls a little.
- **During a supplier outage**, own-forecast stages place phantom orders: they
  re-order what is already on the way. The supplier's ratio doubles against a
  normal year (85 vs 40), and the chain cost
  rises by about 40 % over what the same outage costs with shared data. No
  ordering policy can protect consumers from an eight-week outage; only the
  cost differs.

## Dashboard

The inputs are three `::BUTTONS` groups: the scenario, who sees consumer
sales, and order smoothing. Together they pick one of the 18 stored runs, and
charts with `'tween'::TRANSITION` move to the new numbers.

| Tab | Shows |
|---|---|
| header | KPIs for the selected run: bullwhip at the supplier, consumer fill rate, weeks consumers waited and chain cost, each with `::THRESHOLDS`, `::INFO` and a `::DELTA` against "own forecasts, no smoothing" |
| Where it amplifies | **The chain as a `::DIAGRAM`.** Goods flow as solid `'flow-volume'` edges, as wide as the volume shipped and coloured by how often the shipping stage fell short. Orders and shared sales data flow as dashed edges. Each box shows its bullwhip ratio (with `::THRESHOLDS`) and its weekly orders as a sparkline. A `::STORY` walks the shock up the chain, and `'stage'::SELECT_AS` sets the stage the next tab shows. Below the diagram: the bullwhip ratio per stage with its step factor, orders vs consumer sales per stage, a `::TRACK` strip of events (promotion, outage, factory at capacity, late deliveries) and the formulas in LaTeX |
| Stock and service | **The clicked stage** (the retailer by default): fill rate, weeks with a backlog, peak backlog, average stock and cost. Also on the tab: stock on hand and backlog with `::MARKAREA` for the stockout spells and the event strip, where the chain's cost lands (clicking a bar selects that stage too), and the fill rate per stage |
| What would stabilise it | Every policy in the scenario: the supplier's bullwhip ratio and the chain cost per option, and a table with thresholds and the cheapest option highlighted |

## Files

| File | Role |
|---|---|
| `simulate.py` | The simulation: Python standard library, seed 7. It writes `web/data/supply-chain/weeks.csv` (3,744 rows: 18 runs × 4 stages × 52 weeks, about 440 KB) and prints the bullwhip ratios. |
| `showcase.sql` | A link to `web/examples/supply-chain.sql`, the dashboard. |

```sh
python3 examples/supply-chain/simulate.py
cd web && ../target/release/dashboard --check --strict examples/supply-chain.sql
```
