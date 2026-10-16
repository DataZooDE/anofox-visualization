#!/usr/bin/env bash
# Demo data for the served example: a made-up chain of 12 bakeries in
# the North, the Midlands and the South of England.
#
#   generate.sh init           store list, products, daily sales of the 16 weeks
#                              up to last Sunday and two forecast runs
#   generate.sh week           the next week of sales (a new file in incoming/)
#   generate.sh run [model]    a forecast run from the sales so far
#                              (model: seasonal-naive | moving-average | trend)
#
# Writes under $WORK (default: examples/served/work, git-ignored):
#   incoming/      stores, products and one sales file per week. The serving
#                  session loads these into tables; viewers see them through
#                  the snapshot (refresh to see a new week).
#   data/runs/<run_id>/{run,forecast}.parquet
#                  the live data folder: the server reads it in place, so a
#                  new run is readable at once.
# Deterministic: noise is a hash of (store, product, day), no RNG state.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${WORK:-$HERE/work}"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
IN="$WORK/incoming"
RUNS="$WORK/data/runs"
TMP="$WORK/tmp"
mkdir -p "$IN" "$RUNS" "$TMP"

sql() { "$DUCKDB" -batch -noheader -list :memory: "$@"; }

# Uniform(0,1) and a standard normal from the md5 of a key.
MACROS="
CREATE MACRO unif(k) AS ((hash(md5(k)) >> 11)::DOUBLE + 0.5) / 9007199254740992.0;
CREATE MACRO rnorm(k) AS sqrt(-2.0 * ln(unif(k || ':a'))) * cos(2.0 * pi() * unif(k || ':b'));
"

reference() {
  sql <<SQL
COPY (SELECT * FROM (VALUES
  ('Leeds Kirkgate',    'North',    'Leeds',      1450,  0.0006),
  ('York Shambles',     'North',    'York',       1200,  0.0000),
  ('Ancoats',           'North',    'Manchester',  900,  0.0004),
  ('Newcastle Quayside','North',    'Newcastle',  1050,  0.0030),
  ('Jewellery Quarter', 'Midlands', 'Birmingham', 1600,  0.0002),
  ('Hockley',           'Midlands', 'Nottingham', 1150,  0.0005),
  ('Leicester Market',  'Midlands', 'Leicester',   850,  0.0000),
  ('Derby Cathedral',   'Midlands', 'Derby',       780,  0.0010),
  ('Brighton Lanes',    'South',    'Brighton',   1300,  0.0008),
  ('Clifton',           'South',    'Bristol',    1250,  0.0000),
  ('Bath Abbey',        'South',    'Bath',        820,  0.0003),
  ('Jericho',           'South',    'Oxford',      950, -0.0035)
) t(store, region, city, base_units, trend)) TO '$IN/stores.parquet' (FORMAT parquet);
COPY (SELECT * FROM (VALUES
  ('Sourdough loaf', 'Bread',  4.20, 0.12),
  ('Wholemeal loaf', 'Bread',  3.10, 0.08),
  ('Bread roll',     'Rolls',  0.60, 0.22),
  ('Sausage roll',   'Rolls',  2.10, 0.16),
  ('Croissant',      'Pastry', 1.60, 0.14),
  ('Cinnamon bun',   'Pastry', 2.80, 0.07),
  ('Bakewell tart',  'Pastry', 3.20, 0.05),
  ('Coffee to go',   'Drinks', 2.90, 0.16)
) t(product, category, price, share)) TO '$IN/products.parquet' (FORMAT parquet);
SQL
}

# sales_week <monday> <first monday>: one week of daily sales per store and
# product.
sales_week() {
  local mon="$1" start="$2"
  sql <<SQL
$MACROS
COPY (
  WITH d AS (SELECT (DATE '$mon' + INTERVAL (i) DAY)::DATE AS day FROM range(7) r(i)),
  x AS (
    SELECT d.day, s.store, p.product, p.price,
           s.base_units * p.share
             * CASE isodow(d.day) WHEN 6 THEN 1.25 WHEN 7 THEN 0.55 ELSE 1.0 END
             -- a slow start to the week, the Friday bump
             * CASE isodow(d.day) WHEN 1 THEN 0.92 WHEN 5 THEN 1.08 ELSE 1.0 END
             -- a new competitor near one store, growth at others
             * greatest(0.5, 1 + s.trend * (d.day - DATE '$start'))
             -- warm months: less bread, more coffee and pastry
             * CASE WHEN p.category = 'Bread' THEN 1 - 0.10 * sin(2 * pi() * (dayofyear(d.day) - 110) / 365.0)
                    ELSE 1 + 0.06 * sin(2 * pi() * (dayofyear(d.day) - 110) / 365.0) END
             * exp(0.12 * rnorm(s.store || '|' || p.product || '|' || d.day::VARCHAR)) AS mu
    FROM d, read_parquet('$IN/stores.parquet') s, read_parquet('$IN/products.parquet') p)
  SELECT day, store, product, round(mu)::INTEGER AS units,
         round(round(mu) * price, 2) AS revenue,
         now() AS loaded_at   -- when the till export landed
  FROM x ORDER BY day, store, product
) TO '$TMP/sales_$mon.parquet' (FORMAT parquet);
SQL
  mv "$TMP/sales_$mon.parquet" "$IN/sales_$mon.parquet"
}

next_week() {
  local next start
  if ls "$IN"/sales_*.parquet >/dev/null 2>&1; then
    read -r start next < <(sql -separator ' ' -c "SELECT min(day), (max(day) + 1)::DATE FROM read_parquet('$IN/sales_*.parquet');")
  else
    # 16 weeks up to last Sunday
    start="$(sql -c "SELECT (date_trunc('week', current_date) - INTERVAL 16 WEEK)::DATE;")"
    next="$start"
  fi
  sales_week "$next" "$start"
  echo "week of $next → $IN/sales_$next.parquet"
}

# run <model>: weekly revenue per store for the 4 weeks after the last full
# week of sales, with 50 % and 80 % intervals that widen with the horizon.
forecast_run() {
  local model="${1:-moving-average}"
  case "$model" in seasonal-naive|moving-average|trend) ;; *)
    echo "unknown model '$model' (seasonal-naive | moving-average | trend)" >&2; exit 2;;
  esac
  local cutoff id n=1
  cutoff="$(sql -c "SELECT max(day) FROM read_parquet('$IN/sales_*.parquet');")"
  id="${cutoff}_${model}"
  while [ -e "$RUNS/$id" ]; do n=$((n + 1)); id="${cutoff}_${model}-$n"; done
  mkdir -p "$TMP/$id"
  sql <<SQL
CREATE TABLE wk AS
SELECT store, date_trunc('week', day)::DATE AS week, sum(revenue) AS revenue
FROM read_parquet('$IN/sales_*.parquet') GROUP BY ALL;
CREATE TABLE fit AS
WITH recent AS (
  SELECT *, row_number() OVER (PARTITION BY store ORDER BY week DESC) AS age FROM wk)
SELECT store,
       max(week) AS last_week,
       max(revenue) FILTER (WHERE age = 1) AS last,
       avg(revenue) FILTER (WHERE age <= 4) AS ma4,
       regr_slope(revenue, epoch(week) / 604800.0) FILTER (WHERE age <= 8) AS slope,
       regr_intercept(revenue, epoch(week) / 604800.0) FILTER (WHERE age <= 8) AS icept,
       stddev_samp(revenue) FILTER (WHERE age <= 8) AS sd
FROM recent GROUP BY store;
COPY (
  SELECT '$id' AS run_id, f.store, s.region,
         (f.last_week + INTERVAL (7 * h) DAY)::DATE AS week, h AS horizon,
         round(yhat, 2) AS yhat,
         round(yhat - 0.674 * se, 2) AS lo50, round(yhat + 0.674 * se, 2) AS hi50,
         round(yhat - 1.2816 * se, 2) AS lo80, round(yhat + 1.2816 * se, 2) AS hi80
  FROM fit f JOIN read_parquet('$IN/stores.parquet') s USING (store),
       range(1, 5) r(h),
       LATERAL (SELECT CASE '$model'
                  WHEN 'seasonal-naive' THEN f.last
                  WHEN 'moving-average' THEN f.ma4
                  ELSE f.icept + f.slope * (epoch(f.last_week + INTERVAL (7 * h) DAY) / 604800.0) END AS yhat,
                f.sd * sqrt(h) * CASE '$model' WHEN 'seasonal-naive' THEN 1.3 WHEN 'trend' THEN 0.9 ELSE 1.0 END AS se)
  ORDER BY store, week
) TO '$TMP/$id/forecast.parquet' (FORMAT parquet);
COPY (SELECT '$id' AS run_id, '$model' AS model, DATE '$cutoff' AS cutoff, 4 AS horizon_weeks,
             now() AS created_at)
TO '$TMP/$id/run.parquet' (FORMAT parquet);
SQL
  # Appear in one step: the server never sees a half-written run.
  mv "$TMP/$id" "$RUNS/$id"
  echo "run $id → $RUNS/$id"
}

case "${1:-}" in
  init)
    reference
    for _ in $(seq 16); do next_week >/dev/null; done
    echo "16 weeks of sales → $IN"
    forecast_run seasonal-naive
    forecast_run moving-average
    ;;
  week) next_week ;;
  run) forecast_run "${2:-moving-average}" ;;
  *) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
