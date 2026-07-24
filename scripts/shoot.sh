#!/usr/bin/env bash
# shoot.sh — render a folder of dashboards to PNG for visual review.
#
# `dashboard --check` validates a dashboard's SQL/structure but never looks at
# the result. This serves the dashboards headless and screenshots each page (and
# each ::TAB) so a reviewer — a person or a vision agent — can grade the actual
# rendered image against docs/dashboard-visual-review.md.
#
#   scripts/shoot.sh --dash examples/sp500-dashboard/single \
#       --init examples/sp500-dashboard/init.sql \
#       --workdir examples/sp500-dashboard \
#       --tabs "Market overview,Sector explorer,Valuation & screens" \
#       --out /tmp/shots
#
# Flags:
#   --dash DIR      folder of *.sql dashboards to serve (required)
#   --init FILE     read-write setup run before serving (builds tables/views)
#   --workdir DIR   cd here before serving (so --init's relative paths resolve)
#   --db FILE       serve against a prebuilt DuckDB instead of --init
#   --ext FILE      extension path (default /tmp/anofox_visualization.duckdb_extension)
#   --duckdb BIN    duckdb binary (default: from PATH)
#   --port N        (default 8130)
#   --tabs "A,B,C"  also screenshot each named tab (?tab=) of every dashboard
#   --out DIR       output dir for PNGs (default /tmp/anofox-shots)
#   --width N       viewport width (default 1440)
#   --height N      viewport height (default 2200)
set -u

DASH="" INIT="" WORKDIR="" DB="" PORT=8130 TABS="" OUT="/tmp/anofox-shots"
EXT="${EXT:-/tmp/anofox_visualization.duckdb_extension}"
DUCKDB="${DUCKDB:-$(command -v duckdb || echo "$HOME/.local/bin/duckdb")}"
WIDTH=1440 HEIGHT=2200
while [ $# -gt 0 ]; do
  case "$1" in
    --dash) DASH="$2"; shift 2;;
    --init) INIT="$2"; shift 2;;
    --workdir) WORKDIR="$2"; shift 2;;
    --db) DB="$2"; shift 2;;
    --ext) EXT="$2"; shift 2;;
    --duckdb) DUCKDB="$2"; shift 2;;
    --port) PORT="$2"; shift 2;;
    --tabs) TABS="$2"; shift 2;;
    --out) OUT="$2"; shift 2;;
    --width) WIDTH="$2"; shift 2;;
    --height) HEIGHT="$2"; shift 2;;
    *) echo "unknown flag: $1" >&2; exit 2;;
  esac
done
[ -n "$DASH" ] || { echo "usage: shoot.sh --dash DIR [--init FILE] [--workdir DIR] [--tabs 'A,B'] [--out DIR]" >&2; exit 2; }
[ -f "$EXT" ] || { echo "extension not found at $EXT (build it, or pass --ext)" >&2; exit 2; }
CHROME="$(command -v google-chrome-stable || command -v google-chrome || command -v chromium || true)"
[ -n "$CHROME" ] || { echo "no chrome/chromium on PATH" >&2; exit 2; }

mkdir -p "$OUT"
LOG="$(mktemp)"
# Resolve the dashboard dir and output dir to absolute paths before we cd into
# --workdir, so relative --out / --dash still land in the right place.
DASH_ABS="$(cd "$DASH" && pwd)"
OUT="$(cd "$OUT" && pwd)"
[ -n "$WORKDIR" ] && cd "$WORKDIR"

# Build the serve script: load ext, optional init, serve read-only.
serve_sql() {
  echo "LOAD '$EXT';"
  [ -n "$INIT" ] && echo ".read $INIT"
  echo "SELECT anofox_serve_dashboards('$DASH_ABS', $PORT);"
}
# Serve read-only, keeping the session alive (anofox_serve_dashboards returns
# immediately). Against a prebuilt --db we attach it read-only; otherwise --init
# builds the tables in an in-memory session.
pkill -x duckdb 2>/dev/null; sleep 0.3
if [ -n "$DB" ]; then
  ( serve_sql; tail -f /dev/null ) | "$DUCKDB" -readonly "$DB" -unsigned >"$LOG" 2>&1 &
else
  ( serve_sql; tail -f /dev/null ) | "$DUCKDB" -unsigned >"$LOG" 2>&1 &
fi

# Wait for the server to report it's up.
for _ in $(seq 1 40); do grep -q 'serving .* locked' "$LOG" && break; sleep 0.25; done
if ! grep -q 'serving .* locked' "$LOG"; then echo "server didn't start:" >&2; cat "$LOG" >&2; pkill -x duckdb; exit 1; fi
echo "serving $DASH_ABS on :$PORT"

urlencode() { python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "$1"; }
shoot_url() { # name url
  local name="$1" url="$2" prof; prof="$(mktemp -d)"
  "$CHROME" --headless=new --disable-gpu --no-sandbox --hide-scrollbars \
    --user-data-dir="$prof" --window-size="${WIDTH},${HEIGHT}" --virtual-time-budget=15000 \
    --screenshot="$OUT/$name.png" "$url" >/dev/null 2>&1
  rm -rf "$prof"
  echo "  $name.png $([ -s "$OUT/$name.png" ] && echo ok || echo FAIL)"
}

# One PNG per dashboard (+ per tab if --tabs given).
for f in "$DASH_ABS"/*.sql; do
  [ -e "$f" ] || continue
  id="$(basename "$f" .sql)"
  shoot_url "$id" "http://127.0.0.1:$PORT/d/$id"
  if [ -n "$TABS" ]; then
    IFS=',' read -ra TS <<< "$TABS"
    for t in "${TS[@]}"; do
      slug="$(echo "$t" | tr ' /&' '___' | tr -cd 'A-Za-z0-9_')"
      shoot_url "${id}__${slug}" "http://127.0.0.1:$PORT/d/$id?tab=$(urlencode "$t")"
    done
  fi
done

pkill -x duckdb 2>/dev/null
echo "→ PNGs in $OUT"
