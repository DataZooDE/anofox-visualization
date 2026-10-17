#!/usr/bin/env python3
"""Synthetic purchase-order history for the supplier lead-time dashboard.

Writes the tables web/examples/supplier-lead-times.sql reads, in the layouts
inventory-rs uses for lead-time calibration (DataZooDE/inventory-rs, private):

  po_history    one row per purchase-order line, contract v1 `po_history`
                (po_id, line_id, supplier_id, sku_id, node_id, ordered_at,
                ordered_qty, promised_at, status, cancelled_qty, uom)
  po_receipt    one row per (partial) receipt, contract v1 `po_receipt`
                (po_id, line_id, sku_id, receipt_id, received_at,
                received_qty, uom)
  items         per item: supplier, daily demand mean / std dev, unit cost,
                order quantity — the inputs of the safety-stock formula
  suppliers     per supplier: name, region, contracted lead time
  disruptions   known supply events (shutdowns, port congestion, capacity)
  lead_time_monitor
                every closed line's lead-time error against the lead time the
                planning system assumed at the time (inventory-monitor's
                `lead_time_error`), with a one-sided CUSUM and its state
  proposals     calibration proposals for `supplier.lead_time_mean` /
                `supplier.lead_time_std_dev` (inventory-calibration), with
                mode, status and the day they were decided
  provenance    source, generator, seed

Lead time follows inventory-calibration's default rule
(LeadTimeRule::WeightedMeanReceipt): the quantity-weighted mean receipt day
minus the order day. Nothing here is inventory-rs output or real data:
6 made-up suppliers and 15 made-up items, orders from Jan 2025 to Sep 2026.
Python standard library only; seeded, so every run writes the same bytes.

usage: python3 examples/supplier-lead-times/synthetic.py OUT_DIR [--seed 7]
"""

import csv
import math
import os
import random
import sys
from datetime import date, timedelta

SEED = 7
START, TODAY = date(2025, 1, 2), date(2026, 9, 30)

# supplier_id, name, region, contracted (promised) lead time in days
SUPPLIERS = [
    ("SUP-01", "Fasteners", "domestic", 7),
    ("SUP-02", "Packaging", "domestic", 10),
    ("SUP-03", "Valves", "EU", 18),
    ("SUP-04", "Castings", "EU", 14),
    ("SUP-05", "Motors", "Asia, sea freight", 42),
    ("SUP-06", "Seals", "domestic", 5),
]

# sku_id, supplier, description, demand/day, sd/day, unit cost (EUR), order every n days
ITEMS = [
    ("SKU-101", "SUP-01", "Hex bolt M8", 420.0, 130.0, 0.08, 7),
    ("SKU-102", "SUP-01", "Washer 8 mm", 610.0, 200.0, 0.02, 7),
    ("SKU-103", "SUP-01", "Lock nut M10", 180.0, 70.0, 0.15, 10),
    ("SKU-201", "SUP-02", "Carton 40x30", 95.0, 30.0, 0.9, 14),
    ("SKU-202", "SUP-02", "Pallet wrap", 22.0, 8.0, 6.5, 14),
    ("SKU-301", "SUP-03", "Ball valve DN25", 14.0, 5.0, 38.0, 21),
    ("SKU-302", "SUP-03", "Check valve DN40", 6.0, 2.6, 74.0, 21),
    ("SKU-401", "SUP-04", "Pump housing", 9.0, 3.0, 112.0, 14),
    ("SKU-402", "SUP-04", "Impeller cover", 12.0, 4.0, 46.0, 14),
    ("SKU-403", "SUP-04", "Bearing bracket", 18.0, 6.0, 21.0, 14),
    ("SKU-501", "SUP-05", "Motor 2.2 kW", 4.0, 1.6, 240.0, 14),
    ("SKU-502", "SUP-05", "Motor 5.5 kW", 2.5, 1.1, 410.0, 14),
    ("SKU-601", "SUP-06", "O-ring 32 mm", 260.0, 90.0, 0.35, 7),
    ("SKU-602", "SUP-06", "Shaft seal 25 mm", 40.0, 15.0, 4.2, 7),
    ("SKU-603", "SUP-06", "Gasket set", 15.0, 6.0, 9.8, 10),
]

# supplier_id, start, end, kind, what — goods in transit or in production
# then take longer (`extra` days, drawn per order)
DISRUPTIONS = [
    ("SUP-05", date(2025, 1, 20), date(2025, 2, 16), "shutdown", "Lunar New Year: factory closed", (6, 12)),
    ("SUP-02", date(2025, 8, 4), date(2025, 8, 22), "shutdown", "Summer shutdown", (3, 7)),
    ("SUP-05", date(2025, 10, 27), date(2025, 11, 30), "port congestion", "Port congestion at the origin port", (8, 18)),
    ("SUP-03", date(2025, 12, 15), date(2026, 1, 9), "shutdown", "Year-end shutdown", (4, 9)),
    ("SUP-05", date(2026, 2, 9), date(2026, 3, 1), "shutdown", "Lunar New Year: factory closed", (5, 11)),
    ("SUP-04", date(2026, 3, 16), date(2026, 7, 31), "capacity", "Furnace relining: one of two lines down", (0, 0)),
    ("SUP-05", date(2026, 8, 10), date(2026, 8, 24), "weather", "Typhoon: port closed for 5 days", (5, 10)),
]

# CUSUM on the standardised lead-time error: slack k, warning and alarm lines
K, WARN, ALARM = 0.5, 3.0, 5.0


def base_lead_time(rng, sup, ordered):
    """Lead time in days (before disruptions) of a line ordered that day."""
    if sup == "SUP-01":
        return 5.4 + rng.gammavariate(2.0, 0.45)
    if sup == "SUP-02":
        return 7.9 + rng.gammavariate(2.2, 0.5)
    if sup == "SUP-03":
        return 16.2 * math.exp(rng.gauss(0, 0.13))
    if sup == "SUP-04":
        # stable at ~13.5 d until the furnace relining; then a drift to ~20 d
        # with a wider spread, easing slightly after the line is back
        t0, t1 = date(2026, 3, 9), date(2026, 5, 4)
        if ordered < t0:
            return rng.gauss(13.4, 1.1)
        f = min(1.0, (ordered - t0).days / (t1 - t0).days)
        mean = 13.4 + 6.8 * f - (1.2 if ordered > date(2026, 8, 1) else 0)
        return rng.gauss(mean, 1.1 + 1.9 * f)
    if sup == "SUP-05":
        return rng.gauss(39.0, 2.2)
    if sup == "SUP-06":
        # quick, but the promise of 5 days is optimistic: usually a day late
        lam, k = 1.3, 0
        p, u = math.exp(-lam), rng.random()
        s = p
        while u > s:
            k += 1
            p *= lam / k
            s += p
        return 4.6 + k + rng.random() * 0.4
    raise ValueError(sup)


def write(out_dir, name, cols, rows):
    with open(os.path.join(out_dir, name + ".csv"), "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(cols)
        for r in rows:
            w.writerow(["" if v is None else v for v in r])


def main():
    args = sys.argv[1:]
    seed = SEED
    if "--seed" in args:
        i = args.index("--seed")
        seed = int(args[i + 1])
        del args[i:i + 2]
    if len(args) != 1:
        sys.exit(__doc__)
    out_dir = args[0]
    os.makedirs(out_dir, exist_ok=True)
    rng = random.Random(seed)
    promised = {s[0]: s[3] for s in SUPPLIERS}

    history, receipts, lines = [], [], []
    po_no = 0
    for sku, sup, _desc, mu, _sd, _cost, every in ITEMS:
        day = START + timedelta(days=rng.randrange(every))
        while day <= TODAY - timedelta(days=3):
            po_no += 1
            po = f"PO-{25000 + po_no}"
            qty = int(round(mu * every * rng.uniform(0.85, 1.15) / 10.0)) * 10 or 10
            prom = day + timedelta(days=promised[sup])
            lt = max(1.0, base_lead_time(rng, sup, day))
            for d_sup, d0, d1, _kind, _what, (lo, hi) in DISRUPTIONS:
                arrive = day + timedelta(days=lt)
                if d_sup == sup and hi > 0 and day <= d1 and arrive >= d0:
                    lt += rng.uniform(lo, hi)
            if rng.random() < 0.012:
                history.append([po, "1", sup, sku, "DC-1", day.isoformat(), qty, prom.isoformat(),
                                "cancelled", qty, "EA"])
                day += timedelta(days=every + rng.choice([-1, 0, 0, 1]))
                continue
            # receipts: most lines arrive at once, some in two shipments
            if rng.random() < 0.16:
                first = int(round(qty * rng.uniform(0.55, 0.8)))
                d1_ = max(1, round(lt - rng.uniform(0.5, 2.0)))
                d2_ = d1_ + rng.randint(2, 6)
                parts = [(d1_, first), (d2_, qty - first)]
            else:
                parts = [(max(1, round(lt)), qty)]
            got = [(day + timedelta(days=d), q) for d, q in parts if day + timedelta(days=d) <= TODAY]
            if not got:
                status = "open"
            elif len(got) < len(parts):
                status = "partially_received"
            else:
                status = "received"
            history.append([po, "1", sup, sku, "DC-1", day.isoformat(), qty, prom.isoformat(),
                            status, None, "EA"])
            for n, (rd, q) in enumerate(got, 1):
                receipts.append([po, "1", sku, f"{po}-R{n}", rd.isoformat(), q, "EA"])
            if status == "received":
                wmean = sum((rd - day).days * q for rd, q in got) / qty
                lines.append((got[-1][0], po, sup, wmean))
            day += timedelta(days=every + rng.choice([-1, 0, 0, 1]))

    # Monitoring and calibration, per supplier, in the order lines close.
    # The planning system starts with the contracted lead time and a spread of
    # 1.5 d; calibration runs on the first of each month on the last 90 days
    # of closed lines (n >= 6), and the day after a CUSUM alarm on the last 45
    # days (the lines since the shift). A change of
    # the mean up to 10 % is applied on its own (guarded auto); larger ones
    # are escalated and approved by the planner a week later.
    monitor, proposals = [], []
    lines.sort(key=lambda r: (r[2], r[0], r[1]))
    for sup, _name, _region, prom_days in SUPPLIERS:
        mine = [r for r in lines if r[2] == sup]
        mean, sd = float(prom_days), 1.5
        pending = []  # (effective day, parameter, new value)
        cusum, state, seq = 0.0, "ok", 0
        runs = sorted({date(y, m, 1) for y in (2025, 2026) for m in range(1, 13)
                       if START < date(y, m, 1) <= TODAY})
        extra_runs = []
        i = 0

        def calibrate(day, trigger):
            nonlocal seq
            span = 90 if trigger == "schedule" else 45
            window = [r[3] for r in mine if day - timedelta(days=span) <= r[0] < day]
            if len(window) < 6:
                return
            m = sum(window) / len(window)
            s = math.sqrt(sum((x - m) ** 2 for x in window) / (len(window) - 1))
            for param, old, new in (("supplier.lead_time_mean", mean, m),
                                    ("supplier.lead_time_std_dev", sd, s)):
                if abs(new - old) < 0.25 or any(q[1] == param for q in pending):
                    continue
                seq += 1
                big = abs(new - old) > 0.10 * mean
                status = "escalated" if big else "applied"
                decided = day + timedelta(days=7) if big else day
                if decided > TODAY:
                    status, decided = "escalated", None
                elif big:
                    status = "approved"
                what = "mean lead time" if param.endswith("mean") else "lead-time spread"
                proposals.append([f"{sup}:{day.isoformat()}:{seq}", sup, day.isoformat(), param,
                                  round(old, 2), round(new, 2), len(window),
                                  "high" if len(window) >= 12 else "medium",
                                  "guarded_auto", status, decided and decided.isoformat(), trigger,
                                  f"{len(window)} lines: {what} {new:.1f} d (was {old:.1f})"])
                if decided:
                    pending.append((decided, param, new))

        while i < len(mine) or runs or extra_runs:
            nxt = min([r for r in runs[:1] + extra_runs[:1]] + ([mine[i][0]] if i < len(mine) else []))
            # parameter changes that take effect today
            for p in [p for p in pending if p[0] <= nxt]:
                if p[1].endswith("mean"):
                    mean = p[2]
                else:
                    sd = max(0.5, p[2])
                pending.remove(p)
                cusum = 0.0
            if runs and runs[0] == nxt:
                calibrate(runs.pop(0), "schedule")
                continue
            if extra_runs and extra_runs[0] == nxt:
                calibrate(extra_runs.pop(0), "alarm")
                continue
            closed, po, _s, lt = mine[i]
            i += 1
            err = lt - mean
            cusum = max(0.0, cusum + err / sd - K)
            new_state = "alarm" if cusum >= ALARM else "warning" if cusum >= WARN else "ok"
            if new_state == "alarm" and state != "alarm":
                extra_runs.append(closed + timedelta(days=1))
                extra_runs.sort()
            state = new_state
            monitor.append([sup, po, closed.isoformat(), round(lt, 2), round(mean, 2), round(sd, 2),
                            round(err, 2), round(cusum, 3), WARN, ALARM, state])

    history.sort(key=lambda r: (r[5], r[0]))
    receipts.sort(key=lambda r: (r[4], r[3]))
    proposals.sort(key=lambda r: (r[2], r[0]))
    write(out_dir, "po_history",
          ["po_id", "line_id", "supplier_id", "sku_id", "node_id", "ordered_at", "ordered_qty",
           "promised_at", "status", "cancelled_qty", "uom"], history)
    write(out_dir, "po_receipt",
          ["po_id", "line_id", "sku_id", "receipt_id", "received_at", "received_qty", "uom"], receipts)
    write(out_dir, "items",
          ["sku_id", "supplier_id", "description", "demand_mean", "demand_std_dev", "unit_cost",
           "review_days"], [list(r) for r in ITEMS])
    write(out_dir, "suppliers", ["supplier_id", "name", "region", "promised_days"],
          [list(s) for s in SUPPLIERS])
    write(out_dir, "disruptions", ["supplier_id", "start_date", "end_date", "kind", "what"],
          [[s, a.isoformat(), b.isoformat(), k, w] for s, a, b, k, w, _ in DISRUPTIONS])
    write(out_dir, "lead_time_monitor",
          ["supplier_id", "po_id", "closed_at", "lead_time_days", "assumed_lead_time",
           "assumed_std_dev", "lead_time_error", "cusum", "warning_threshold", "alarm_threshold",
           "state"], monitor)
    write(out_dir, "proposals",
          ["id", "supplier_id", "ds", "parameter", "old_num", "new_num", "sample_size", "confidence",
           "mode", "status", "decided_at", "trigger", "reason"], proposals)
    write(out_dir, "provenance", ["source", "seed", "generator", "schema"],
          [["synthetic", seed, "examples/supplier-lead-times/synthetic.py",
            "inventory-rs contract v1 po_history / po_receipt"]])


if __name__ == "__main__":
    main()
