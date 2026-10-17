#!/usr/bin/env python3
"""Simulate a four-stage supply chain for 52 weeks, every scenario x policy.

Retailer <- distribution centre <- factory <- component supplier <- raw
material (always available). Consumers buy from the retailer; every stage
serves the orders of the stage below it and orders from the stage above.

Deterministic: Python standard library only, one fixed seed, and the same
random numbers (consumer noise, supplier late weeks) in every run, so runs
differ only by the scenario and the policy. Writes
web/data/supply-chain/weeks.csv (one row per run, stage and week).

    python3 examples/supply-chain/simulate.py

Period timing (after inventory-rs's network twin): within week t,
  1. receipts: shipments due at t arrive at the start of the week;
  2. stage by stage from the retailer up: the stage serves its backlog, then
     this week's incoming order, from what is on hand; ships go out at once
     and arrive after the lane's lead time; then it reviews its inventory
     position and orders from the stage above, which sees the order the same
     week.
Unserved orders stay open (backlog) at every stage, customers included.
"""
import csv
import math
import os
import random

WEEKS = 52
SEED = 7
BASE = 100.0

STAGES = ["Retailer", "Distribution centre", "Factory", "Supplier"]
# Weeks from placing an order until it arrives, if the stage above ships at
# once. The factory's 2 = 1 week component transport + 1 week production.
LEAD = {"Retailer": 1, "Distribution centre": 2, "Factory": 2, "Supplier": 2}
HOLD = {"Retailer": 1.0, "Distribution centre": 0.6, "Factory": 0.4, "Supplier": 0.2}  # $ / unit-week
BACK = {"Retailer": 4.0, "Distribution centre": 1.5, "Factory": 1.5, "Supplier": 1.5}  # $ / unit-week
ALPHA = 0.25           # exponential smoothing of the demand forecast
Z_SIGMA = 1.65 * 15.0  # safety stock per sqrt(lead time + 1): 95 % on sigma = 15 units/week
CAPACITY = 150.0       # factory production per week
BATCH_STAGE = "Supplier"  # buys raw material in lots
LOT = 50.0
SUPPLY_LINE_WEIGHT = 0.7  # share of the pipeline a stage counts when ordering on its own

SCENARIOS = ["Normal", "Promotion", "Supplier outage"]
POLICIES = ["Own forecasts", "Shared sales data"]
SMOOTHING = {"None": 1.0, "2 weeks": 2.0, "4 weeks": 4.0}

PROMO = range(12, 16)       # weeks 13-16 (t is 0-based): +60 %
PROMO_DIP = range(16, 18)   # weeks 17-18: -15 % (demand bought ahead)
DISRUPTION = range(18, 26)  # weeks 19-26: the supplier ships at most 35 units/week
DISRUPTED_CAP = 35.0


def common_randoms():
    rng = random.Random(SEED)
    noise = [rng.gauss(0.0, 15.0) for _ in range(WEEKS)]
    late = [rng.random() < 0.06 for _ in range(WEEKS)]  # supplier lane one week slower
    return noise, late


def consumer_demand(scenario, noise):
    d = []
    for t in range(WEEKS):
        f = 1.0
        if scenario == "Promotion":
            if t in PROMO:
                f = 1.6
            elif t in PROMO_DIP:
                f = 0.85
        d.append(max(0.0, round(BASE * f + noise[t])))
    return d


def safety(stage):
    return Z_SIGMA * math.sqrt(LEAD[stage] + 1)


def simulate(scenario, policy, smooth_label, noise, late):
    ti = SMOOTHING[smooth_label]
    shared = policy.startswith("Shared")
    demand = consumer_demand(scenario, noise)
    n = len(STAGES)

    # Steady state at 100 units/week: pipelines full, safety stock on hand.
    on_hand = [safety(s) for s in STAGES]
    backlog = [0.0] * n          # owed to the stage below (customers for the retailer)
    open_orders = [0.0] * n      # ordered from the stage above, not yet shipped by it
    forecast = [BASE] * n
    arrivals = [dict() for _ in range(n)]  # week -> units arriving (finished goods)
    for i, s in enumerate(STAGES):
        if s != "Factory":  # orders of the last L weeks, still under way
            for k in range(LEAD[s]):
                arrivals[i][k] = BASE
    queue = 0.0                  # factory: components waiting for capacity
    wip = {0: BASE}              # factory: week -> units finishing production
    comp_arrivals = {0: BASE}    # factory: week -> components arriving

    rows = []
    for t in range(WEEKS):
        # 1. receipts
        received = [arrivals[i].pop(t, 0.0) for i in range(n)]
        f = STAGES.index("Factory")
        received[f] += wip.pop(t, 0.0)
        queue += comp_arrivals.pop(t, 0.0)
        produced = min(CAPACITY, queue)
        capacity_bound = queue > CAPACITY + 1e-9
        queue -= produced
        wip[t + 1] = wip.get(t + 1, 0.0) + produced
        for i in range(n):
            on_hand[i] += received[i]

        # 2. downstream to upstream
        incoming = demand[t]
        for i, s in enumerate(STAGES):
            due = backlog[i] + incoming
            cap = math.inf
            if s == "Supplier" and scenario == "Supplier outage" and t in DISRUPTION:
                cap = DISRUPTED_CAP
            shipped = min(on_hand[i], due, cap)
            on_time = max(0.0, min(incoming, shipped - backlog[i]))
            on_hand[i] -= shipped
            backlog[i] = due - shipped
            if i > 0:
                below = STAGES[i - 1]
                open_orders[i - 1] -= shipped
                if below == "Factory":  # components: 1 week transport (2 when late)
                    comp_arrivals[t + (2 if late[t] else 1)] = comp_arrivals.get(t + (2 if late[t] else 1), 0.0) + shipped
                else:
                    arrivals[i - 1][t + LEAD[below]] = arrivals[i - 1].get(t + LEAD[below], 0.0) + shipped
            late_ship = s == "Supplier" and late[t] and shipped > 0

            # forecast: own incoming orders, or consumer sales when shared
            signal = demand[t] if shared else incoming
            forecast[i] = ALPHA * signal + (1 - ALPHA) * forecast[i]
            fc = forecast[i]

            pipeline = sum(v for k, v in arrivals[i].items() if k > t) + open_orders[i]
            if s == "Factory":
                pipeline += queue + sum(v for k, v in wip.items() if k > t) + sum(v for k, v in comp_arrivals.items() if k > t)
            position = on_hand[i] - backlog[i] + pipeline

            # Order-up-to with proportional smoothing (POUT): close the gap to
            # the safety stock and to a full pipeline (the orders of the last
            # L - 1 weeks) over `ti` weeks. On its own, a planner counts only
            # part of what is already under way (Sterman's supply-line
            # underweighting); with shared data the whole pipeline is seen.
            L = LEAD[s]
            weight = 1.0 if shared else SUPPLY_LINE_WEIGHT
            gap = (safety(s) - (on_hand[i] - backlog[i])) + weight * (fc * (L - 1) - pipeline)
            order = max(0.0, fc + gap / ti)
            if s == BATCH_STAGE and not shared:
                # the supplier buys raw material in full lots; with shared
                # data it is replenished continuously
                order = LOT * math.ceil(order / LOT)
            order = max(0.0, round(order))
            if s == "Supplier":
                arrivals[i][t + L] = arrivals[i].get(t + L, 0.0) + order  # raw material never short
            else:
                open_orders[i] += order

            rows.append({
                "scenario": scenario, "policy": policy, "smoothing": smooth_label,
                "week": t + 1, "stage_no": i + 1, "stage": s,
                "consumer_demand": demand[t], "demand_in": incoming, "order_out": order,
                "shipped": round(shipped, 1), "on_time": round(on_time, 1), "received": round(received[i], 1),
                "on_hand": round(on_hand[i], 1), "backlog": round(backlog[i], 1),
                "position": round(position, 1), "forecast": round(fc, 1),
                "produced": round(produced, 1) if s == "Factory" else None,
                "capacity_bound": capacity_bound if s == "Factory" else False,
                "late_shipment": late_ship,
                "cost_holding": round(HOLD[s] * on_hand[i], 2),
                "cost_backlog": round(BACK[s] * backlog[i], 2),
            })
            incoming = order
    return rows


def variance(xs):
    m = sum(xs) / len(xs)
    return sum((x - m) ** 2 for x in xs) / (len(xs) - 1)


def main():
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    out_dir = os.path.join(root, "web", "data", "supply-chain")
    os.makedirs(out_dir, exist_ok=True)
    noise, late = common_randoms()
    rows = []
    for sc in SCENARIOS:
        for po in POLICIES:
            for sm in SMOOTHING:
                rows += simulate(sc, po, sm, noise, late)
    path = os.path.join(out_dir, "weeks.csv")
    with open(path, "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=list(rows[0].keys()), lineterminator="\n")
        w.writeheader()
        w.writerows(rows)
    print(f"{path}: {len(rows)} rows, {os.path.getsize(path) // 1024} KB")
    # Bullwhip ratio per stage: Var(orders placed) / Var(consumer demand).
    for sc in SCENARIOS:
        for po in POLICIES:
            for sm in SMOOTHING:
                run = [r for r in rows if (r["scenario"], r["policy"], r["smoothing"]) == (sc, po, sm)]
                vd = variance([r["consumer_demand"] for r in run if r["stage"] == "Retailer"])
                ratios = [variance([r["order_out"] for r in run if r["stage"] == s]) / vd for s in STAGES]
                print(f"{sc:20s} {po:26s} {sm:8s} " + " ".join(f"{x:6.2f}" for x in ratios))


if __name__ == "__main__":
    main()
