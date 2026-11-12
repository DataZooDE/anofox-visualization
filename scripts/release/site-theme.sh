#!/usr/bin/env bash
# Owner 2026-11-12: the online demo wears the website's colours. In each given
# index.html this sets
# - the builder's default dashboard theme (window.__anofoxTheme, as `serve
#   --theme` does) to <theme.json> (website/showcase/site-theme.json, which
#   the site's own pictures use); a dashboard's own ::THEME still wins;
# - the builder chrome's --ui-* tokens (accent, background, ink, muted, line)
#   to the same theme's colours, light and dark.
# The product's default look stays graphite + blue; only the Pages demo
# changes.
#
#   scripts/release/site-theme.sh <theme.json> <index.html>...
set -euo pipefail
theme="$1"
shift
for f in "$@"; do
  [ -f "$f" ] || continue
  grep -q "window.__anofoxTheme=" "$f" && continue
  python3 - "$theme" "$f" <<'PY'
import json, sys
theme, page = json.load(open(sys.argv[1])), sys.argv[2]
spec = json.dumps(theme, separators=(",", ":")).replace("<", "\\u003c")
def tokens(c):
    return (f"--ui-accent:{c['accent']};--ui-accent2:{c['accent']};--ui-bg:{c['bg']};"
            f"--ui-ink:{c['ink']};--ui-muted:{c['muted']};--ui-line:{c['line']};")
css = f":root{{{tokens(theme['colors'])}}}"
if "dark" in theme:
    css += f"body.dark{{{tokens(theme['dark']['colors'])}}}"
s = open(page).read()
head = f"<style>{css}</style><script>window.__anofoxTheme={spec};</script>"
open(page, "w").write(s.replace("</head>", head + "</head>", 1))
PY
  echo "site theme → $f"
done
