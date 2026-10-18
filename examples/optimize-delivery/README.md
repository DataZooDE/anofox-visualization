# Which van serves which stop tomorrow, so every customer gets its delivery on time?

A use-case dashboard for [anofox-optimize](https://github.com/DataZooDE/anofox-optimize):
a delivery plan for a small fleet. It's the "Which van serves which stop tomorrow?"
example of the [live demo](https://datazoode.github.io/anofox-visualization/)
(sidebar: anofox extensions → Optimize).

**Who asks:** the dispatcher of a made-up food wholesaler at 15:00 the day
before. 40 bakeries, cafés, restaurants, shops and a canteen around the depot,
each with a deadline (bakeries before 07:30, restaurants before lunch prep)
and 1–3 roll cages; a van takes 32 cages, leaves at 06:00 and drives 40 km/h.
The town is a street grid — a street every kilometre each way, the depot and
every stop on a crossing — so the distance between two places is the street
distance |dx| + |dy|: the solvers get it, every km on the dashboard is a
street km, and the routes are drawn along the streets (east-west, then
north-south). Today each driver has a fixed district and drives the order
list.

**The story (4 vans):** today's districts drive 516 km, 13 stops are late and
one van carries 33 cages for 32 places. The shortest routes drive 268 km but
6 stops are late; the deadline-aware plan drives 305 km and serves every stop
on time, back at the depot by 10:45 — 4 vans are enough, a fifth only adds cost.

## How it is computed

anofox-optimize has no vehicle-routing family, so the plan is "cluster first,
route second", the routing done by the extension:

1. **Assign** stops to vans: today by fixed districts; optimised by a
   capacity-balanced sweep around the depot, tried from 24 start angles
   (plain SQL).
2. **Order** each van's stops:
   * `opt_sequence_best_of` — the shortest route (setup matrix = driving
     minutes; the depot is pinned first and last with large constants),
   * `opt_schedule_exact` — the least weighted lateness against the deadlines
     (setup = driving minutes from the depot / the previous stop, processing =
     unloading time, due date = deadline, weight 3 for the canteen and key
     customers); `opt_schedule_best_of` for a van with more than 14 stops,
   * `opt_sequence_as_given` — today's order list.
   The deadline-aware plan takes the schedule order, or the shortest route
   where it is just as punctual.
3. **Keep** the best start angle per plan and van count (fewest km for the
   shortest route; least lateness, then km, for the deadline-aware plan).

Deadlines are "deliver by"; earliest-arrival windows aren't modelled.

## Stored results, not live

The browser demo doesn't run anofox_optimize: its DuckDB-Wasm build doesn't
load yet (see [../optimize-showcase/README.md](../optimize-showcase/README.md)).

| File | Role |
|---|---|
| `compute.sql` | Synthetic stops (hash-based noise, no files), the assignment sweep and the solver runs for 4, 5 and 6 vans. Tables `dr_*`. |
| `../../scripts/gen-optimize-cases.sh delivery` | Runs `compute.sql` natively and writes `dr_stops`, `dr_summary`, `dr_vans`, `dr_route`, `dr_legs` (routes as WKT lines) to `web/data/optimize-delivery/*.csv` (≈ 50 KB), plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/optimize-delivery.sql`, the dashboard. |

```sh
duckdb -unsigned -c "INSTALL anofox_optimize FROM 'https://get.erpl.io'"   # once
DUCKDB=duckdb scripts/gen-optimize-cases.sh delivery
cd web && ../target/release/dashboard --check --strict examples/optimize-delivery.sql
```

## Inputs and tabs

Above the tabs (the KPI row uses them): the new plan to compare with today's
(deadline-aware / shortest route) and the vans on the road (4–6). The van for
the stop list sits in the Stop list tab, the cost assumptions (€ per street
km, per driver hour, per van day, per late delivery) in the Decision tab — an
input placed after a `::TAB` renders in that tab. "Today's plan" is the stored
plan `today: districts, order list`. The KPI row compares the selected plan
with today's plan at the same van count: stops on time, street km, driver
hours and cost per day.

Every number the dashboard shows beyond the stored tables (first and last
arrival per van, minutes late, the legs, arrowheads and street backdrop of
the maps, the cost components) is derived in SQL from the stored tables.

1. **Map** — today's plan next to the new one (`::MAP` of WKT in km) over
   the street grid (`::BASEMAP`): routes along the streets with an arrowhead
   per leg pointing the way the van drives, the depot as a
   grey square, a red ring around every late stop (`::COLORS` pins the depot
   and late-stop colours; the vans keep the palette). Hovering a route, arrow
   or stop lights that van up on both maps; a click picks it out in the van
   table below (km, hours on the road, return time and late stops, today vs
   plan, the plan's first and last stop, and a link to the stop list).
2. **Vans** — cages per van against capacity, hours on the road, van-by-van
   table; a click on a van's bar opens its stop list.
3. **Stop list** — one van's route as a `::STORY` on the map (`::XAXIS` = stop
   number: ▶ Play steps leg by leg with arrival and deadline), the minutes to
   spare per stop, and the driver's list.
4. **Decision** — the cost assumptions; the cheapest stored plan that serves
   every stop on time against today's plan with as many vans, and what one
   more van would cost; cost per day by component (street km, driver time,
   van day rates, late penalties) for every plan × van count; late stops by
   van count; the recommendation as a share of today's plan; every plan priced.
