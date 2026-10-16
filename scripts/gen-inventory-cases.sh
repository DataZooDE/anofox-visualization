#!/usr/bin/env bash
# Regenerate the stored results of the inventory twin dashboard
# (web/examples/inventory-twin.sql): the export tables of one inventory-rs
# demo run, as CSV in web/data/inventory-twin/, plus provenance.csv.
#
# usage: scripts/gen-inventory-cases.sh
#   INVENTORY_RS=/path/to/inventory-rs (default: ../../rust/inventory-rs from
#   this repo's parent, i.e. a sibling checkout) — DataZooDE/inventory-rs is
#   private.
#
# With an inventory-rs whose end_to_end_demo has --export-dir
# (DataZooDE/inventory-rs#27, with the tables of #22, #23, #26 and #28) the
# real demo run is exported. Until then — or without a checkout — the
# documented synthetic stand-in examples/inventory-twin/synthetic.py writes
# the same files in the same schemas (seed 42; provenance.csv says
# source=synthetic). Either way the dashboard SQL is the same.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INVENTORY_RS="${INVENTORY_RS:-$ROOT/../../rust/inventory-rs}"
OUT="$ROOT/web/data/inventory-twin"
DEMO="$INVENTORY_RS/crates/inventory-api/examples/end_to_end_demo.rs"
rm -rf "$OUT"
mkdir -p "$OUT"
if [ -f "$DEMO" ] && grep -q -- '--export-dir' "$DEMO"; then
  echo "== inventory-rs end_to_end_demo --export-dir → $OUT"
  (cd "$INVENTORY_RS" && cargo run -q -p inventory-api --features csv,arrow --example end_to_end_demo -- --export-dir "$OUT")
else
  echo "== no end_to_end_demo --export-dir in $INVENTORY_RS (DataZooDE/inventory-rs#27): synthetic tables → $OUT"
  python3 "$ROOT/examples/inventory-twin/synthetic.py" "$OUT" --seed 42
fi
ls -l "$OUT"
du -sh "$OUT"
