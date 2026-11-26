#!/usr/bin/env bash
# The agent skills of this release (#1389): the Claude Code plugin folder
# plugins/anofox-visualization/ (.claude-plugin/plugin.json + skills/*/SKILL.md)
# as one asset, so a release pins the skills that match it. Fails when
# plugin.json or a SKILL.md's `version:` is not this release's version (the
# release PR bumps them with the rest). package-cli.sh puts the same folder
# into every CLI archive.
#   output: $DIST/anofox-visualization-skills-<tag>.zip
set -euo pipefail
# shellcheck source=scripts/release/common.sh
. "$(dirname "$0")/common.sh"

PLUGIN="$ROOT/plugins/anofox-visualization"
want="${TAG#v}"
got="$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$PLUGIN/.claude-plugin/plugin.json")"
[ "$got" = "$want" ] || { echo "plugin.json version $got != $want (the release tag)" >&2; exit 1; }
n=0
for f in "$PLUGIN"/skills/*/SKILL.md; do
  v="$(sed -n 's/^version: *//p' "$f" | head -1 | tr -d '\r')"
  [ "$v" = "$want" ] || { echo "${f#"$ROOT"/}: version '$v' != $want (the release tag)" >&2; exit 1; }
  n=$((n + 1))
done
[ "$n" -gt 0 ] || { echo "no skills in $PLUGIN/skills" >&2; exit 1; }

mkdir -p "$DIST"
OUT="$DIST/anofox-visualization-skills-$TAG.zip"
rm -f "$OUT"
# the folder as is (anofox-visualization/.claude-plugin/…, …/skills/…)
(cd "$ROOT/plugins" && "$PYTHON" -m zipfile -c "$OUT" anofox-visualization)
echo "== done → $OUT ($n skills) =="
