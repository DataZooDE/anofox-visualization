#!/usr/bin/env python3
"""Synthetic customers and deliveries for the delivery-coverage map dashboard.

Writes the tables web/examples/delivery-coverage.sql reads:

  districts    per German district (the 400 Kreise of the built-in
               boundaries, web/data/boundaries/de-districts.csv): AGS, name,
               state (ISO 3166-2), a made-up population, its centre, the
               nearest distribution centre and the road distance to it
  dcs          the three distribution centres: id, name, lat, lon
  customers    one row per customer: id, AGS, lat/lon (inside its
               district), segment, serving DC
  deliveries   per week (12 weeks), district and DC: deliveries made and
               how many arrived on time
  provenance   source, generator, seed

A made-up wholesaler serves shops and restaurants from three DCs (Hamburg,
Dortmund, Nürnberg). Customers cluster where people live and near the DCs;
districts far from every DC — the north-east and parts of the east — get
few customers and later deliveries. Nothing here is real data; the
district shapes are BKG VG2500 (dl-de/by-2-0), as in the built-in pack.
Python standard library only; seeded, so every run writes the same bytes.

usage: python3 examples/delivery-coverage/synthetic.py OUT_DIR [--seed 11]
"""

import csv
import math
import os
import random
import re
import sys

SEED = 11
WEEKS = 12
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BOUNDARIES = os.path.join(ROOT, "web", "data", "boundaries", "de-districts.csv")
STATES = {
    "01": "DE-SH", "02": "DE-HH", "03": "DE-NI", "04": "DE-HB", "05": "DE-NW", "06": "DE-HE",
    "07": "DE-RP", "08": "DE-BW", "09": "DE-BY", "10": "DE-SL", "11": "DE-BE", "12": "DE-BB",
    "13": "DE-MV", "14": "DE-SN", "15": "DE-ST", "16": "DE-TH",
}
# id, name, lat, lon
DCS = [
    ("DC-N", "Hamburg", 53.551, 9.993),
    ("DC-W", "Dortmund", 51.514, 7.466),
    ("DC-S", "Nürnberg", 49.452, 11.077),
]
SEGMENTS = [("Restaurant", 0.45), ("Shop", 0.4), ("Canteen", 0.15)]


def rings(wkt):
    """The polygons of a (MULTI)POLYGON WKT as lists of (lon, lat) rings."""
    polys = []
    for poly in re.findall(r"\(\(([^()]*(?:\)\s*,\s*\([^()]*)*)\)\)", wkt):
        rs = []
        for ring in re.split(r"\)\s*,\s*\(", poly):
            rs.append([tuple(map(float, p.split())) for p in ring.split(",")])
        polys.append(rs)
    return polys


def inside(pt, polys):
    x, y = pt
    hit = False
    for rs in polys:
        for r in rs:
            for i in range(len(r)):
                (x1, y1), (x2, y2) = r[i], r[i - 1]
                if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
                    hit = not hit
    return hit


def area_km2(polys, lat):
    """Shoelace area of the outer rings, in km² near `lat`."""
    kx, ky = 111.32 * math.cos(math.radians(lat)), 110.57
    a = 0.0
    for rs in polys:
        r = rs[0]
        a += abs(sum(r[i - 1][0] * r[i][1] - r[i][0] * r[i - 1][1] for i in range(len(r)))) / 2
    return a * kx * ky


def km(lat1, lon1, lat2, lon2):
    p = math.pi / 180
    a = (math.sin((lat2 - lat1) * p / 2) ** 2
         + math.cos(lat1 * p) * math.cos(lat2 * p) * math.sin((lon2 - lon1) * p / 2) ** 2)
    return 2 * 6371 * math.asin(math.sqrt(a))


def main(out, seed):
    rnd = random.Random(seed)
    os.makedirs(out, exist_ok=True)
    csv.field_size_limit(10**9)
    with open(BOUNDARIES, newline="", encoding="utf-8") as f:
        rows = list(csv.DictReader(f))
    districts, customers, weekly = [], [], {}
    for r in rows:
        polys = rings(r["wkt"])
        pts = [p for rs in polys for p in rs[0]]
        lon0, lon1 = min(p[0] for p in pts), max(p[0] for p in pts)
        lat0, lat1 = min(p[1] for p in pts), max(p[1] for p in pts)
        lat, lon = (lat0 + lat1) / 2, (lon0 + lon1) / 2
        area = area_km2(polys, lat)
        city = "Kreisfreie Stadt" in r["alt"] or r["key"] in ("11000", "02000")
        # people per km²: cities dense, the north-east thin
        density = (rnd.uniform(900, 2600) if city else rnd.uniform(90, 260)) * (
            0.55 if lon > 11.5 and lat > 51.5 else 1.0)
        pop = int(round(min(area * density, 3_700_000), -2))
        dc, dist = min(((d, km(lat, lon, d[2], d[3]) * 1.3) for d in DCS), key=lambda x: x[1])
        districts.append((r["key"], r["name_de"], STATES[r["key"][:2]], pop, round(lat, 4),
                          round(lon, 4), dc[0], round(dist, 1)))
        # customers per 10k inhabitants fall off with the distance to the DC
        rate = 1.6 * math.exp(-dist / 170) * rnd.uniform(0.7, 1.3)
        n = int(pop / 10_000 * rate + rnd.random())
        if dist > 330 and rnd.random() < 0.6:
            n = 0  # not served at all
        for _ in range(n):
            for _try in range(50):
                p = (rnd.uniform(lon0, lon1), rnd.uniform(lat0, lat1))
                if inside(p, polys):
                    break
            seg = rnd.choices([s for s, _ in SEGMENTS], [w for _, w in SEGMENTS])[0]
            cid = f"C{len(customers) + 1:05d}"
            ckm = km(p[1], p[0], dc[2], dc[3]) * 1.3
            customers.append((cid, r["key"], round(p[1], 4), round(p[0], 4), seg, dc[0], round(ckm, 1)))
            per_week = {"Restaurant": 2, "Shop": 1, "Canteen": 3}[seg]
            p_late = min(0.45, 0.02 + (ckm / 420) ** 2)
            for w in range(1, WEEKS + 1):
                k = sum(rnd.random() < 0.9 for _ in range(per_week))
                late = sum(rnd.random() < p_late for _ in range(k))
                weekly[(w, r["key"], dc[0])] = [a + b for a, b in zip(weekly.get((w, r["key"], dc[0]), [0, 0]), [k, k - late])]

    def write(name, header, data):
        with open(os.path.join(out, name), "w", newline="", encoding="utf-8") as f:
            w = csv.writer(f, lineterminator="\n")
            w.writerow(header)
            w.writerows(data)

    write("districts.csv", ["ags", "district", "state", "population", "lat", "lon", "dc", "dc_km"], districts)
    write("dcs.csv", ["dc", "name", "lat", "lon"], DCS)
    write("customers.csv", ["customer", "ags", "lat", "lon", "segment", "dc", "km"], customers)
    deliveries = [(w, ags, dc, n, ok) for (w, ags, dc), (n, ok) in sorted(weekly.items())]
    write("deliveries.csv", ["week", "ags", "dc", "deliveries", "on_time"], deliveries)
    write("provenance.csv", ["source", "generator", "seed"],
          [("synthetic; district shapes BKG VG2500 (dl-de/by-2-0)",
            "examples/delivery-coverage/synthetic.py", seed)])
    print(f"{len(districts)} districts, {len(customers)} customers, {sum(d[3] for d in deliveries)} deliveries")


if __name__ == "__main__":
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    s = SEED
    if "--seed" in args:
        s = int(args[args.index("--seed") + 1])
    main(args[0], s)
