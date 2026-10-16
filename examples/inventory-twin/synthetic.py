#!/usr/bin/env python3
"""Synthetic stand-in for inventory-rs's `end_to_end_demo --export-dir`.

Writes the export tables the inventory twin dashboard reads
(web/examples/inventory-twin.sql), in exactly the shapes specified by the
inventory-rs issues that will deliver the real export:

  trajectory, projection                                   DataZooDE/inventory-rs#22
  alarms, parameter_changes, proposals, detector_status,
  replay_deviation                                         DataZooDE/inventory-rs#23
  detector_trace                                           DataZooDE/inventory-rs#26
  run_manifest, run_edges                                  DataZooDE/inventory-rs#28
  provenance                                               DataZooDE/inventory-rs#27

Nothing here comes from inventory-rs's code or data: 12 made-up SKUs shaped
like its demo fleet (classes A/B/C, smooth / erratic / intermittent / lumpy
demand, min/max policies, two suppliers), periods 60-179 of a daily calendar
anchored at 2024-01-01, then a 28-period projection. Python standard library
only; seeded, so every run writes the same bytes.

usage: python3 examples/inventory-twin/synthetic.py OUT_DIR [--seed 42]
"""

import csv
import hashlib
import math
import os
import random
import sys
from datetime import date, timedelta

SEED = 42
ANCHOR = date(2024, 1, 1)
FIRST, LAST = 60, 179          # the streamed periods (the twin is built at 60)
HORIZON = 28                   # projection periods after LAST
PATHS = 400                    # Monte-Carlo paths per projection
LEVELS = [(0.5, 0.25, 0.75), (0.8, 0.10, 0.90), (0.9, 0.05, 0.95)]
SCHEDULE = (90, 120, 150)      # scheduled calibration runs (every 30 periods)
BASELINE = 14                  # periods before a detector has a baseline

# sku, class, shape, mean demand/period, sd (or nonzero size), P(demand>0),
# min, max, supplier, stated lead time, sd, order multiple, review every n,
# on hand at period 60
SKUS = [
    ("SKU-001", "A", "smooth", 53.0, 11.0, 1.0, 300, 1196, "SUP-1", 5, 1.5, 10, 1, 640),
    ("SKU-002", "A", "smooth", 59.0, 12.0, 1.0, 330, 980, "SUP-1", 5, 1.5, 12, 1, 520),
    ("SKU-003", "A", "smooth", 59.5, 10.0, 1.0, 767, 1793, "SUP-2", 10, 3.0, 10, 1, 1210),
    ("SKU-004", "A", "smooth", 48.0, 9.0, 1.0, 380, 1182, "SUP-1", 5, 1.5, 12, 1, 700),
    ("SKU-012", "A", "erratic", 48.9, 24.0, 1.0, 623, 1246, "SUP-2", 10, 3.0, 12, 1, 940),
    ("SKU-013", "B", "smooth", 5.6, 1.6, 1.0, 42, 90, "SUP-1", 5, 1.5, 6, 2, 60),
    ("SKU-019", "B", "erratic", 2.9, 2.6, 0.9, 26, 52, "SUP-1", 5, 1.5, 6, 2, 36),
    ("SKU-021", "B", "erratic", 4.2, 3.4, 0.9, 50, 110, "SUP-2", 10, 3.0, 6, 2, 52),
    ("SKU-028", "C", "intermittent", 0.0, 4.0, 0.22, 16, 34, "SUP-1", 5, 1.5, 1, 7, 18),
    ("SKU-033", "C", "lumpy", 0.0, 9.0, 0.13, 30, 55, "SUP-2", 10, 3.0, 1, 7, 22),
    ("SKU-037", "C", "intermittent", 0.0, 2.0, 0.12, 6, 12, "SUP-1", 5, 1.5, 1, 7, 4),
    ("SKU-045", "C", "lumpy", 0.0, 7.0, 0.18, 28, 48, "SUP-2", 10, 3.0, 1, 7, 15),
]
MODE = {"A": "guarded_auto", "B": "guarded_auto", "C": "propose_only"}

# What happens in the run (the story the dashboard tells):
#  * SKU-004: demand rises 35 % from period 126; the order placed after the
#    first alarms is delivered 5 days late. The detectors fire, the stock runs out, the
#    alarm-triggered calibration's bias change is escalated (too large for
#    guarded_auto) and approved at 141, which raises the reorder point.
#  * SKU-003: demand +22 % from 124 (alarms at ~130), fixed by the 150 run.
#  * SKU-021: demand +60 % from 138 (B, erratic).
#  * SKU-002: supplier SUP-1 gets slower for it (lead time 5 -> 7) from 100.
DRIFT = {"SKU-004": (126, 1.35), "SKU-003": (124, 1.22), "SKU-021": (138, 1.6)}
LT_SHIFT = {"SKU-002": (100, 2.0)}
LATE = {"SKU-004": (128, 136, 5)}  # the first order in 128..136 comes 5 days late

# Detector thresholds (warning, alarm), as in the demo's report.
THRESH = {
    "cusum": (9.18, 13.77),
    "page_hinkley": (8.14, 12.21),
    "occurrence_cusum": (4.0, 6.0),
    "mae": (2.4, 2.999),
    "psi": (1.2, 1.817),
    "ks": (0.55, 0.712),
}


def ds(period):
    return (ANCHOR + timedelta(days=period)).isoformat()


def eid(*parts):
    return hashlib.sha256("|".join(map(str, parts)).encode()).hexdigest()[:16]


def r3(x):
    return None if x is None else round(x, 3)


def demand_draw(rng, s, mult=1.0):
    _, _, shape, mu, sd, p, *_ = s
    if shape in ("smooth", "erratic") and p >= 1.0:
        if shape == "erratic":
            # log-normal: variable sizes, frequent demand
            cv = sd / mu
            sigma = math.sqrt(math.log(1 + cv * cv))
            return max(0, round(mu * mult * math.exp(rng.gauss(-sigma * sigma / 2, sigma))))
        return max(0, round(rng.gauss(mu * mult, sd * math.sqrt(mult))))
    if rng.random() > p * (1 if mult == 1 else min(1.0, mult)):
        return 0
    if shape == "lumpy":
        return 1 + round(rng.expovariate(1 / sd) * mult)
    if shape == "erratic":
        return 1 + round(rng.expovariate(1 / sd) * mult)
    return 1 + round(rng.gauss(sd, 1.0) * mult) if sd > 1 else 1


def expected_demand(s):
    _, _, shape, mu, sd, p, *_ = s
    if mu > 0 and p >= 1.0:
        return mu
    return p * (1 + sd)


def simulate(rng, s, start, on_hand, pipeline, rop_delta, lt_mean, lt_sd, horizon,
             bias=0.0, late=None, drift=None, lt_shift=None, record=None):
    """Steps one twin; returns the period records (lost-sales mode)."""
    sku, cls, shape, mu, sd, p, mn, mx, sup, lt0, ltsd0, mult, review, _ = s
    out = []
    pipeline = dict(pipeline)
    for period in range(start, start + horizon):
        on_hand_start = on_hand
        receipts = pipeline.pop(period, 0)
        avail = on_hand + receipts
        m = 1.0
        if drift and period >= drift[0]:
            m = drift[1]
        elif bias:
            m = 1.0 + bias
        d = demand_draw(rng, s, m)
        fulfilled = min(d, avail)
        lost = d - fulfilled
        on_hand = avail - fulfilled
        on_order = sum(pipeline.values())
        reviewed = (period % review) == 0
        order_qty, arrival = 0, None
        rop = mn + (rop_delta(period) if callable(rop_delta) else rop_delta)
        top = mx + (rop_delta(period) if callable(rop_delta) else rop_delta)
        if reviewed and on_hand + on_order <= rop:
            need = top - (on_hand + on_order)
            order_qty = int(math.ceil(need / mult) * mult)
            lt_m = lt_mean(period) if callable(lt_mean) else lt_mean
            if lt_shift and period >= lt_shift[0]:
                lt_m += lt_shift[1]
            lt = max(1, round(rng.gauss(lt_m, lt_sd)))
            if late and sku in late and late[sku][0] <= period <= late[sku][1]:
                lt += late.pop(sku)[2]
            arrival = period + 1 + lt
            pipeline[arrival] = pipeline.get(arrival, 0) + order_qty
            on_order += order_qty
            if record is not None:
                record.append((sku, period, lt))
        out.append({
            "period": period, "on_hand_start": on_hand_start, "receipts": receipts,
            "demand": d, "fulfilled": fulfilled, "lost": lost,
            "on_hand_end": on_hand, "on_order_end": on_order,
            "position_end": on_hand + on_order, "reviewed": reviewed,
            "order_qty": order_qty, "order_arrival_period": arrival,
        })
    return out, on_hand, pipeline


def write(out_dir, name, cols, rows):
    with open(os.path.join(out_dir, name + ".csv"), "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(cols)
        for r in rows:
            w.writerow(["" if r.get(c) is None else
                        ("true" if r[c] is True else "false" if r[c] is False else r[c]) for c in cols])


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    out_dir = sys.argv[1]
    seed = int(sys.argv[sys.argv.index("--seed") + 1]) if "--seed" in sys.argv else SEED
    os.makedirs(out_dir, exist_ok=True)

    trajectory, projection, alarms, changes, proposals = [], [], [], [], []
    status_rows, replay, trace = [], [], []

    for idx, s in enumerate(SKUS):
        sku, cls, shape, mu, sd, p, mn, mx, sup, lt0, ltsd0, mult, review, oh0 = s
        rng = random.Random(seed * 1000 + idx)
        mean_d = expected_demand(s)
        # The twin's parameters, as calibration changes them over the run.
        params = {"supplier.lead_time_mean": float(lt0), "supplier.lead_time_std_dev": float(ltsd0),
                  "demand_calibration.sigma_multiplier": 1.0, "demand_calibration.bias": 0.0}
        rop_steps = []  # (from period, reorder-point delta)

        # The run is stepped in segments between calibration points so that an
        # applied change takes effect from the next period.
        def rop_delta(period):
            return sum(dl for pe, dl in rop_steps if period > pe)

        orders = []
        lt_hist = [max(1, round(random.Random(seed + idx * 7 + k).gauss(lt0, ltsd0))) for k in range(5)]
        rows, oh, pipe = [], oh0, {}
        # One open order at the start, so the first periods have a receipt.
        pipe[FIRST + 2 + idx % 3] = int(math.ceil((mx - mn) / 2 / mult) * mult)
        period = FIRST
        forecast_bias = 0.0
        stat = {"cusum_pos": 0.0, "cusum_neg": 0.0, "ph_sum": 0.0, "ph_min": 0.0, "ph_max": 0.0,
                "occ": 0.0, "n": 0, "err": [], "dem": []}
        tier_state = {}
        run_points = sorted(set(SCHEDULE))
        pending_trigger = set()
        last_alarm_run = -99
        late = dict(LATE)
        approved = {}
        while period <= LAST:
            seg, oh, pipe = simulate(rng, s, period, oh, pipe, rop_delta, params["supplier.lead_time_mean"],
                                     params["supplier.lead_time_std_dev"], 1, late=late,
                                     drift=DRIFT.get(sku), lt_shift=LT_SHIFT.get(sku), record=orders)
            r = seg[0]
            rows.append(r)
            # ── monitoring: standardized forecast error on the twin's forecast
            fc = mean_d * (1 + forecast_bias)
            scale = (sd if p >= 1.0 else math.sqrt(p * (1 + sd) ** 2)) * params["demand_calibration.sigma_multiplier"]
            e = (r["demand"] - fc) / max(scale, 0.5)
            stat["n"] += 1
            stat["err"].append(abs(r["demand"] - fc) / max(fc, 0.5))
            stat["dem"].append(r["demand"])
            n = stat["n"]
            tests = []
            if n > BASELINE:
                stat["cusum_pos"] = max(0.0, stat["cusum_pos"] + e - 0.5)
                stat["cusum_neg"] = max(0.0, stat["cusum_neg"] - e - 0.5)
                stat["ph_sum"] += e - 0.3
                stat["ph_min"] = min(stat["ph_min"], stat["ph_sum"])
                cus = max(stat["cusum_pos"], stat["cusum_neg"])
                ph = stat["ph_sum"] - stat["ph_min"]
                tests.append(("test", "cusum", "forecast_error", cus))
                tests.append(("test", "page_hinkley", "forecast_error", ph))
                if cls == "C":
                    occ_e = (1 if r["demand"] > 0 else 0) - p
                    stat["occ"] = max(0.0, stat["occ"] + occ_e / max(math.sqrt(p * (1 - p)), 0.1) - 0.6)
                    tests.append(("test", "occurrence_cusum", "demand", stat["occ"]))
                # rolling MAE (relative, last 14 periods), a metric alarm
                mae = sum(stat["err"][-14:]) / 14 * (3.0 if cls == "A" else 0.9)
                tests.append(("metric", "mae", None, mae))
                if cls != "C" and n % 7 == 0 and n >= 2 * BASELINE:
                    base = stat["dem"][:BASELINE]
                    cur = stat["dem"][-14:]
                    shift = abs(sum(cur) / len(cur) - sum(base) / len(base)) / max(scale, 0.5)
                    tests.append(("test", "psi", "demand", shift * 1.1))
                    tests.append(("test", "ks", "demand", min(1.0, shift * 0.42)))
            else:
                tests.append(("test", "cusum", "forecast_error", None))
                tests.append(("test", "page_hinkley", "forecast_error", None))
                if cls == "C":
                    tests.append(("test", "occurrence_cusum", "demand", None))
            for cat, name, stream, val in tests:
                warn, alarm = THRESH[name]
                key = (name, stream)
                if val is None:
                    level = "insufficient_history"
                else:
                    level = "alarm" if val >= alarm else "warning" if val >= warn else "ok"
                prev = tier_state.get(key, "ok")
                state = prev
                if level in ("warning", "alarm"):
                    rank = {"ok": 0, "warning": 1, "alarm": 2}
                    if rank[level] > rank[prev]:
                        state = level
                        alarms.append({
                            "sku": sku, "node": None, "cohort": cls, "shape": shape, "period": period,
                            "ds": ds(period), "tier": level, "category": cat, "name": name, "stream": stream,
                            "value": r3(val), "threshold": alarm if level == "alarm" else warn,
                            "baseline": "per_sku" if n > 2 * BASELINE else "cohort",
                            "event_id": eid(sku, period, name, stream, level),
                        })
                        if level == "alarm" and stream == "forecast_error":
                            pending_trigger.add(period)
                elif level == "ok" and val is not None and val < warn * 0.5:
                    state = "ok"  # re-armed
                tier_state[key] = state
                if name in ("cusum", "page_hinkley", "occurrence_cusum"):
                    trace.append({
                        "period": period, "sku": sku, "cohort": cls, "shape": shape, "test": name,
                        "stream": stream, "statistic": r3(val), "warning_threshold": warn,
                        "alarm_threshold": alarm, "state": "insufficient_history" if val is None else state,
                    })
                if period == LAST:
                    status_rows.append({
                        "sku": sku, "test": name, "stream": stream, "statistic": r3(val), "level": level,
                        "state": state, "warning_threshold": warn, "alarm_threshold": alarm,
                        "n_baseline": min(n, 2 * BASELINE) if n > BASELINE else 0,
                        "n_current": min(n, 14),
                    })

            # ── calibration runs: scheduled, or the period after an alarm
            trig = None
            if period in run_points:
                trig = "schedule"
            elif (period - 1) in pending_trigger and cls != "C" and period - last_alarm_run > 14:
                trig = "alarm"
                last_alarm_run = period
            if trig:
                # an alarm-triggered run looks at the periods since the change
                recent = rows[-30:] if trig == "schedule" else rows[-8:]
                seq = 0
                observed_lt = [lt for (k, pe, lt) in orders if k == sku and pe + lt + 1 <= period]
                lt_obs = lt_hist + observed_lt
                for parameter in ("supplier.lead_time_mean", "supplier.lead_time_std_dev",
                                  "demand_calibration.sigma_multiplier", "demand_calibration.bias"):
                    old = params[parameter]
                    if parameter.startswith("supplier."):
                        n_obs = len(lt_obs)
                        if n_obs < 5 or len(observed_lt) == 0:
                            continue
                        mean_lt = sum(lt_obs[-8:]) / len(lt_obs[-8:])
                        var = sum((x - mean_lt) ** 2 for x in lt_obs[-8:]) / max(1, len(lt_obs[-8:]) - 1)
                        new = mean_lt if parameter.endswith("mean") else max(0.5, math.sqrt(var))
                        twin_inf = min(n_obs, len(observed_lt))
                        indep = n_obs - twin_inf
                        rop = (new - old) * mean_d if parameter.endswith("mean") else (new - old) * 1.65 * sd * 0.6
                    else:
                        if cls == "C" and parameter.endswith("sigma_multiplier"):
                            continue
                        dem = [x["demand"] for x in recent]
                        mean_recent = sum(dem) / len(dem)
                        if parameter.endswith("bias"):
                            new = round(mean_recent / mean_d - 1, 3)
                        else:
                            resid = [x - mean_recent for x in dem]
                            sdr = math.sqrt(sum(v * v for v in resid) / max(1, len(resid) - 1))
                            new = round(max(0.5, min(1.8, sdr / max(scale / params[parameter], 0.5))), 3)
                        n_obs = len(dem)
                        twin_inf = sum(1 for x in recent if x["lost"] > 0)
                        indep = n_obs - twin_inf
                        rop = ((new - old) * mean_d * (lt0 + 1) if parameter.endswith("bias")
                               else (new - old) * 1.65 * scale * math.sqrt(lt0 + 1))
                    if abs(new - old) < (0.02 if not parameter.startswith("supplier.") else 0.05):
                        continue
                    conf = "high" if n_obs >= 20 else "medium" if n_obs >= 5 else "low"
                    big = (abs(new - old) / max(abs(old), 0.1) > 0.25) if parameter.startswith("supplier.") \
                        else abs(new - old) > (0.2 if parameter.endswith("bias") else 0.35)
                    mode = MODE[cls]
                    if mode == "propose_only":
                        status = "proposed"
                    elif big:
                        status = "escalated"
                    else:
                        status = "applied"
                    pid = f"{sku}:{period}:calibration:{seq}"
                    seq += 1
                    reason = {
                        "supplier.lead_time_mean": f"{n_obs} receipts: mean lead time {new:.2f} d (was {old:.2f})",
                        "supplier.lead_time_std_dev": f"{n_obs} receipts: lead-time spread {new:.2f} d (was {old:.2f})",
                        "demand_calibration.sigma_multiplier": f"last {n_obs} periods: forecast errors {new:.2f} x the modelled spread",
                        "demand_calibration.bias": f"last {n_obs} periods: demand {new * 100:+.0f} % against the forecast",
                    }[parameter]
                    if status == "escalated":
                        reason += "; change exceeds the guarded_auto limit"
                    proposals.append({
                        "id": pid, "sku": sku, "node": None,
                        "supplier": sup if parameter.startswith("supplier.") else None,
                        "period": period, "parameter": parameter, "old_num": r3(old), "new_num": r3(new),
                        "sample_size": n_obs, "confidence": conf, "expected_rop_delta": r3(rop),
                        "lineage_independent": indep, "lineage_twin_influenced": twin_inf,
                        "mode": mode, "status": status, "reason": reason,
                    })
                    if status == "applied":
                        apply_at, origin, st = period, "calibration", "applied"
                    elif status == "escalated" and sku == "SKU-004" and trig == "alarm" and parameter.endswith("bias"):
                        apply_at, origin, st = period + 4, "approval", "approved"
                    else:
                        apply_at = None
                    changes.append({
                        "sku": sku, "period": period, "ds": ds(period), "parameter": parameter,
                        "before_num": r3(old), "after_num": r3(new),
                        "before_json": repr(r3(old)), "after_json": repr(r3(new)),
                        "reason": reason, "origin": "calibration", "mode": mode, "status": status,
                        "proposal_id": pid,
                    })
                    if apply_at is not None:
                        if apply_at != period:
                            approved[apply_at] = (parameter, old, new, pid, rop)
                        else:
                            params[parameter] = new
                            rop_steps.append((period, rop))
                            if parameter.endswith("bias"):
                                forecast_bias = new
                                stat["cusum_pos"] = stat["cusum_neg"] = 0.0
                                stat["ph_sum"] = stat["ph_min"] = 0.0
            if period in approved:
                parameter, old, new, pid, rop = approved.pop(period)
                params[parameter] = new
                rop_steps.append((period, rop))
                if parameter.endswith("bias"):
                    forecast_bias = new
                    stat["cusum_pos"] = stat["cusum_neg"] = 0.0
                    stat["ph_sum"] = stat["ph_min"] = 0.0
                changes.append({
                    "sku": sku, "period": period, "ds": ds(period), "parameter": parameter,
                    "before_num": r3(old), "after_num": r3(new),
                    "before_json": repr(r3(old)), "after_json": repr(r3(new)),
                    "reason": "approved by the planner", "origin": "approval", "mode": MODE[cls],
                    "status": "approved", "proposal_id": pid,
                })
            period += 1

        for r in rows:
            trajectory.append({"sku": sku, "node": None, "ds": ds(r["period"]), "backorders_start": 0,
                               "backlog_served": 0, "backlogged": 0, "backorders_end": 0, **r})

        # ── replay: recorded counts every 7 periods, the twin's on-hand vs them
        crng = random.Random(seed * 77 + idx)
        for r in rows:
            if (r["period"] + idx) % 7 != 0:
                continue
            twin = r["on_hand_end"]
            # counting noise within the tolerance (max(1 unit, 2 % of the count))
            noise = round(crng.gauss(0, max(0.3, 0.005 * twin)))
            if sku == "SKU-033" and 120 <= r["period"] < 141:
                noise = -3  # three units on the shelf the twin does not know of (an unbooked return)
            recorded = max(0, twin - noise)
            dev = twin - recorded
            tol = max(1.0, 0.02 * recorded)
            replay.append({"sku": sku, "period": r["period"], "ds": ds(r["period"]), "twin_on_hand": twin,
                           "recorded_on_hand": recorded, "deviation": dev, "abs_deviation": abs(dev),
                           "rel_deviation": r3(abs(dev) / recorded) if recorded else None,
                           "within_tolerance": abs(dev) <= tol})

        # ── projection: Monte Carlo from the end state with the final parameters
        paths = []
        for k in range(PATHS):
            prng = random.Random(seed * 100000 + idx * 1000 + k)
            sim, _, _ = simulate(prng, s, LAST + 1, oh, pipe, rop_delta(LAST + 1),
                                 params["supplier.lead_time_mean"], params["supplier.lead_time_std_dev"],
                                 HORIZON, bias=forecast_bias)
            paths.append(sim)
        ever_out = [False] * PATHS
        for h in range(HORIZON):
            per = LAST + 1 + h
            vals = sorted(pth[h]["on_hand_end"] for pth in paths)
            out = [pth[h]["lost"] > 0 or pth[h]["on_hand_end"] == 0 for pth in paths]
            for k in range(PATHS):
                ever_out[k] = ever_out[k] or out[k]

            def q(a):
                return vals[min(PATHS - 1, int(a * PATHS))]

            for level, lo, hi in LEVELS:
                projection.append({
                    "model_id": sku, "period": per, "ds": ds(per), "yhat": q(0.5), "level": level,
                    "yhat_lower": q(lo), "yhat_upper": q(hi),
                    "mean_on_hand": r3(sum(vals) / PATHS), "mean_backlog": 0.0,
                    "stockout_probability": r3(sum(out) / PATHS),
                    "cumulative_stockout_probability": r3(sum(ever_out) / PATHS),
                    "expected_unmet": r3(sum(pth[h]["lost"] for pth in paths) / PATHS),
                    "order_probability": r3(sum(1 for pth in paths if pth[h]["order_qty"] > 0) / PATHS),
                    "split": "future", "measure": "on_hand",
                })

    write(out_dir, "trajectory",
          ["sku", "node", "period", "ds", "on_hand_start", "backorders_start", "receipts", "demand",
           "fulfilled", "backlog_served", "lost", "backlogged", "on_hand_end", "backorders_end",
           "on_order_end", "position_end", "reviewed", "order_qty", "order_arrival_period"], trajectory)
    write(out_dir, "projection",
          ["model_id", "period", "ds", "yhat", "level", "yhat_lower", "yhat_upper", "mean_on_hand",
           "mean_backlog", "stockout_probability", "cumulative_stockout_probability", "expected_unmet",
           "order_probability", "split", "measure"], projection)
    write(out_dir, "alarms",
          ["sku", "node", "cohort", "shape", "period", "ds", "tier", "category", "name", "stream", "value",
           "threshold", "baseline", "event_id"], alarms)
    write(out_dir, "parameter_changes",
          ["sku", "period", "ds", "parameter", "before_num", "after_num", "before_json", "after_json",
           "reason", "origin", "mode", "status", "proposal_id"], changes)
    write(out_dir, "proposals",
          ["id", "sku", "node", "supplier", "period", "parameter", "old_num", "new_num", "sample_size",
           "confidence", "expected_rop_delta", "lineage_independent", "lineage_twin_influenced", "mode",
           "status", "reason"], proposals)
    write(out_dir, "detector_status",
          ["sku", "test", "stream", "statistic", "level", "state", "warning_threshold", "alarm_threshold",
           "n_baseline", "n_current"], status_rows)
    write(out_dir, "detector_trace",
          ["period", "sku", "cohort", "shape", "test", "stream", "statistic", "warning_threshold",
           "alarm_threshold", "state"], trace)
    write(out_dir, "replay_deviation",
          ["sku", "period", "ds", "twin_on_hand", "recorded_on_hand", "deviation", "abs_deviation",
           "rel_deviation", "within_tolerance"], replay)

    # ── run manifest (inventory-rs#28): what each stage consumed and produced
    n_tp = len(trajectory)
    n_props = len(proposals)
    n_applied = sum(1 for c in changes if c["status"] in ("applied", "approved"))
    manifest = [
        ("ingest", 1, 0, FIRST - 1, 9133, 9128, 5, 0, 0, 0, 0, "demand, stock, purchase orders"),
        ("build_twin", 2, FIRST, FIRST, len(SKUS), len(SKUS), 0, len(SKUS), 0, 0, 0, "twins"),
        ("calibrate", 3, FIRST, LAST, n_tp, n_tp, 0, len(SKUS), 0, n_props, n_applied, "proposals"),
        ("stream", 4, FIRST, LAST, n_tp, n_tp, 0, len(SKUS), len(alarms), 0, 0, "trajectory, alarms"),
        ("replay_history", 5, FIRST, LAST, len(replay), len(replay), 0, len(SKUS), 0, 0, 0, "replay_deviation"),
        ("project", 6, LAST + 1, LAST + HORIZON, len(SKUS), len(SKUS), 0, len(SKUS), 0, 0, 0, "projection"),
    ]
    write(out_dir, "run_manifest",
          ["run_id", "stage", "seq", "started_period", "ended_period", "records_in", "records_accepted",
           "records_rejected", "twins", "alarms", "proposals", "proposals_applied", "artefact"],
          [dict(zip(["stage", "seq", "started_period", "ended_period", "records_in", "records_accepted",
                     "records_rejected", "twins", "alarms", "proposals", "proposals_applied", "artefact"], m),
                run_id="demo-" + str(seed)) for m in manifest])
    write(out_dir, "run_edges", ["from_stage", "to_stage", "count"], [
        {"from_stage": "ingest", "to_stage": "build_twin", "count": 9128},
        {"from_stage": "build_twin", "to_stage": "calibrate", "count": len(SKUS)},
        {"from_stage": "build_twin", "to_stage": "stream", "count": len(SKUS)},
        {"from_stage": "stream", "to_stage": "calibrate", "count": len({a["sku"] for a in alarms if a["tier"] == "alarm"})},
        {"from_stage": "calibrate", "to_stage": "stream", "count": n_applied},
        {"from_stage": "stream", "to_stage": "replay_history", "count": n_tp},
        {"from_stage": "stream", "to_stage": "project", "count": len(SKUS)},
    ])
    write(out_dir, "provenance",
          ["source", "crate_version", "git_sha", "schema_version", "export_version", "seed", "generator"],
          [{"source": "synthetic", "crate_version": None, "git_sha": None, "schema_version": None,
            "export_version": "draft (inventory-rs#22 #23 #26 #28)", "seed": seed,
            "generator": "examples/inventory-twin/synthetic.py"}])


if __name__ == "__main__":
    main()
