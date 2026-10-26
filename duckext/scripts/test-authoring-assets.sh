#!/usr/bin/env bash
# End-to-end checks of #522: `anofox_serve(port, {assets: '<dir>'})`'s new,
# opt-in `GET`/`HEAD /assets/<rest>` route on the authoring backend. The
# authoring builder has no dashboards folder at all (plan-514 decision 5),
# so unlike the locked server's `assets/` subfolder this names a root
# directly, and is off unless given.
#
# Covers:
#   * a valid image is served with the locked route's own headers
#     (Content-Type sniffed, nosniff, CSP sandbox, Cache-Control, CORP).
#   * HEAD matches GET's Content-Length without reading the body.
#   * containment: a raw `..` (curl --path-as-is: curl itself normalizes a
#     plain `..` before sending otherwise, which would never actually test
#     the server), percent-encoded `..`, an absolute-looking path, a real
#     symlink escape (a symlinked subdirectory *and* a symlinked leaf file,
#     both reaching an actual image on the other side — review of #604 S3:
#     a target with no image extension is already refused at the
#     extension check, before containment is even reached, so that alone
#     isn't a containment test), a non-image extension, a missing file —
#     all a plain 404.
#   * the same token/loopback auth every other authoring route needs.
#   * GET/HEAD only; POST is 405.
#   * the option off (default) is a plain 404, same as any unknown path.
#   * `options.assets` pointing at a missing path, a non-directory, or a
#     symlinked root (even named with a trailing slash — #604 S5) all
#     refuse to start (`anofox_serve` itself errors) — never silently
#     serve nothing.
#   * `options.assets` refuses to start when the live session has
#     `enable_external_access = false`, or when `allowed_directories` is
#     set and the root is outside every one of them (#604 S1).
#   * an unrecognised `options` key is still refused (the existing
#     `AppBrand` validation is untouched by pulling `assets` out first).
#
# usage: duckext/scripts/test-authoring-assets.sh <extension> [port]
#        (DUCKDB=… picks the CLI; default: duckdb on PATH)
set -euo pipefail
EXT="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
PORT="${2:-18235}"
DUCKDB="${DUCKDB:-duckdb}"
T="$(mktemp -d)"
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  rm -rf "$T"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
q() { printf "'%s'" "${1//\'/\'\'}"; }

ASSETS="$T/assets"
OUTSIDE="$T/outside"
mkdir -p "$ASSETS" "$OUTSIDE"
SVG='<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4"/></svg>'
printf '%s' "$SVG" > "$ASSETS/pic.svg"
printf 'not an image' > "$ASSETS/notes.txt"
printf '%s' "$SVG" > "$OUTSIDE/secret.svg"
# #604 S3: a real image reached through a symlinked subdirectory, and
# through a symlinked leaf file — both escape containment with an actual
# file on the other side, unlike a symlink to an extension-less target
# (refused earlier, at the extension check, regardless of containment).
ln -s "$OUTSIDE" "$ASSETS/sublink"
ln -s "$OUTSIDE/secret.svg" "$ASSETS/leaf.svg"

# One `anofox_serve` call, run to completion (success or a startup error),
# over a fresh duckdb process: `$1` = the options JSON (or "" for none).
# Prints the CLI's own stdout+stderr; the caller greps it.
run_serve() {
  local opts="$1" port="$2"
  if [ -n "$opts" ]; then
    echo "LOAD $(q "$EXT"); SELECT anofox_serve($port, $(q "$opts"));" | "$DUCKDB" -unsigned ":memory:" 2>&1
  else
    echo "LOAD $(q "$EXT"); SELECT anofox_serve($port);" | "$DUCKDB" -unsigned ":memory:" 2>&1
  fi
}

# --- startup refusals (never bind, never serve) -----------------------
OUT="$(run_serve "{\"assets\": \"$T/no-such-dir\"}" "$((PORT+1))" || true)"
echo "$OUT" | grep -qi "options.assets" || fail "missing --assets dir should refuse to start: $OUT"
echo "OK: options.assets refuses a missing directory at start"

OUT="$(run_serve "{\"assets\": \"$ASSETS/pic.svg\"}" "$((PORT+2))" || true)"
echo "$OUT" | grep -qi "not a directory" || fail "a non-directory options.assets should refuse to start: $OUT"
echo "OK: options.assets refuses a non-directory at start"

LINK="$T/assets-link"
ln -s "$ASSETS" "$LINK"
OUT="$(run_serve "{\"assets\": \"$LINK\"}" "$((PORT+3))" || true)"
echo "$OUT" | grep -qi "symlink" || fail "a symlinked options.assets root should refuse to start: $OUT"
echo "OK: options.assets refuses a symlinked root at start"

# #604 S5: the same symlinked root, but named with a trailing slash — the
# shape a shell's own tab-completion produces. `symlink_metadata` on a
# path a trailing slash forces to resolve as a directory used to follow
# the link instead of stat-ing it, letting this one through.
OUT="$(run_serve "{\"assets\": \"$LINK/\"}" "$((PORT+6))" || true)"
echo "$OUT" | grep -qi "symlink" || fail "a symlinked options.assets root with a trailing slash should refuse to start: $OUT"
echo "OK: options.assets refuses a symlinked root at start even with a trailing slash"
rm -f "$LINK"

OUT="$(run_serve '{"nope_key": "x"}' "$((PORT+4))" || true)"
echo "$OUT" | grep -qi "unknown key" || fail "an unrecognised options key should still be refused: $OUT"
echo "OK: an unrecognised options key is still refused (AppBrand's own validation is untouched)"

# #604 S1: options.assets reads files with plain std::fs, bypassing the
# live session's own enable_external_access/allowed_directories entirely
# unless this is checked — refuse the option outright instead.
OUT="$(printf "LOAD %s;\nSET enable_external_access=false;\nSELECT anofox_serve(%s, %s);\n" \
  "$(q "$EXT")" "$((PORT+7))" "$(q "{\"assets\": \"$ASSETS\"}")" | "$DUCKDB" -unsigned ":memory:" 2>&1 || true)"
echo "$OUT" | grep -qi "enable_external_access" || fail "options.assets should refuse to start with enable_external_access=false: $OUT"
echo "OK: options.assets refuses to start when enable_external_access is false"

OUT="$(printf "LOAD %s;\nSET allowed_directories=['%s'];\nSELECT anofox_serve(%s, %s);\n" \
  "$(q "$EXT")" "$OUTSIDE" "$((PORT+8))" "$(q "{\"assets\": \"$ASSETS\"}")" | "$DUCKDB" -unsigned ":memory:" 2>&1 || true)"
echo "$OUT" | grep -qi "allowed_directories" || fail "options.assets outside every allowed_directories entry should refuse to start: $OUT"
echo "OK: options.assets refuses a root outside every allowed_directories entry"

OUT="$(printf "LOAD %s;\nSET allowed_directories=['%s'];\nSELECT anofox_serve(%s, %s);\n" \
  "$(q "$EXT")" "$ASSETS" "$((PORT+9))" "$(q "{\"assets\": \"$ASSETS\"}")" | "$DUCKDB" -unsigned ":memory:" 2>&1 || true)"
echo "$OUT" | grep -qi "anofox-visualization serving" || fail "options.assets inside an allowed_directories entry should start: $OUT"
echo "OK: options.assets starts when the root is inside an allowed_directories entry"

# --- a real, running server with the option on -------------------------
mkfifo "$T/in"
"$DUCKDB" -unsigned ":memory:" < "$T/in" > "$T/out" 2>&1 &
PIDS+=($!)
exec 3>"$T/in"
rm "$T/in"
printf "LOAD %s;\nSELECT anofox_serve(%s, %s);\n" "$(q "$EXT")" "$PORT" "$(q "{\"assets\": \"$ASSETS\"}")" >&3
TOKEN=""
for _ in $(seq 100); do
  TOKEN="$(grep -oE 'token=[0-9a-f]+' "$T/out" 2>/dev/null | head -1 | cut -d= -f2 || true)"
  if [ -n "$TOKEN" ]; then break; fi
  if grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$T/out" 2>/dev/null; then
    cat "$T/out" >&2
    fail "anofox_serve did not start"
  fi
  sleep 0.2
done
[ -n "$TOKEN" ] || { cat "$T/out" >&2; fail "could not find the session token"; }

hdr() { curl -s -D - -o /dev/null -H "X-Anofox-Token: $TOKEN" "http://127.0.0.1:$PORT$1"; }
# --path-as-is: curl itself collapses a literal `..` segment in the URL
# before ever sending the request otherwise (#604 S3) — the point of the
# `/assets/../a.duckdb` case below is what the *server* does with a raw
# `..` it actually receives.
code() { curl -s --path-as-is -o /dev/null -w '%{http_code}' -H "X-Anofox-Token: $TOKEN" "http://127.0.0.1:$PORT$1"; }

H="$(hdr /assets/pic.svg)"
echo "$H" | grep -qi '^HTTP/1.1 200' || fail "GET /assets/pic.svg should be 200: $H"
echo "$H" | grep -qi 'content-type: image/svg+xml' || fail "wrong Content-Type: $H"
echo "$H" | grep -qi 'x-content-type-options: nosniff' || fail "missing nosniff: $H"
echo "$H" | grep -qi "content-security-policy: default-src 'none'; style-src 'unsafe-inline'; sandbox" || fail "missing/wrong CSP: $H"
echo "$H" | grep -qi 'cache-control: private, max-age=60' || fail "missing/wrong Cache-Control: $H"
echo "$H" | grep -qi 'cross-origin-resource-policy: same-origin' || fail "missing CORP: $H"
echo "OK: a valid image is served with the locked route's own headers"

GET_LEN="$(echo "$H" | grep -oiE '^content-length: [0-9]+' | grep -oE '[0-9]+')"
HEAD_H="$(curl -sI -H "X-Anofox-Token: $TOKEN" "http://127.0.0.1:$PORT/assets/pic.svg")"
echo "$HEAD_H" | grep -qi '^HTTP/1.1 200' || fail "HEAD should be 200: $HEAD_H"
HEAD_LEN="$(echo "$HEAD_H" | grep -oiE '^content-length: [0-9]+' | grep -oE '[0-9]+')"
[ "$GET_LEN" = "$HEAD_LEN" ] || fail "HEAD Content-Length ($HEAD_LEN) disagrees with GET's ($GET_LEN)"
echo "OK: HEAD agrees with GET's Content-Length without reading the body"

for path in "/assets/../a.duckdb" "/assets/%2e%2e/a.duckdb" "/assets/..%2fa.duckdb" "/assets//etc/passwd" "/assets/notes.txt" "/assets/nope.svg" "/assets/.hidden.svg" "/assets/sublink/secret.svg" "/assets/leaf.svg"; do
  C="$(code "$path")"
  [ "$C" = "404" ] || fail "$path should be 404, got $C"
done
echo "OK: a raw .., encoded traversal, absolute-looking paths, a non-image extension, a missing file, a dot-leading segment, and a real symlink escape (subdirectory and leaf) are all a plain 404"

C="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/assets/pic.svg")"
[ "$C" = "401" ] || fail "no token should be 401, got $C"
echo "OK: /assets/<rest> needs the same token as every other authoring route"

C="$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "X-Anofox-Token: $TOKEN" "http://127.0.0.1:$PORT/assets/pic.svg")"
[ "$C" = "405" ] || fail "POST should be 405, got $C"
echo "OK: /assets/<rest> is GET/HEAD only"

# --- a second server, the option left off (default) --------------------
mkfifo "$T/in2"
"$DUCKDB" -unsigned ":memory:" < "$T/in2" > "$T/out2" 2>&1 &
PIDS+=($!)
exec 4>"$T/in2"
rm "$T/in2"
OFF_PORT="$((PORT+5))"
printf "LOAD %s;\nSELECT anofox_serve(%s);\n" "$(q "$EXT")" "$OFF_PORT" >&4
OFF_TOKEN=""
for _ in $(seq 100); do
  OFF_TOKEN="$(grep -oE 'token=[0-9a-f]+' "$T/out2" 2>/dev/null | head -1 | cut -d= -f2 || true)"
  if [ -n "$OFF_TOKEN" ]; then break; fi
  if grep -qE '^([A-Za-z][A-Za-z ]* )?Error:' "$T/out2" 2>/dev/null; then
    cat "$T/out2" >&2
    fail "anofox_serve (no options) did not start"
  fi
  sleep 0.2
done
[ -n "$OFF_TOKEN" ] || { cat "$T/out2" >&2; fail "could not find the second session's token"; }
C="$(curl -s -o /dev/null -w '%{http_code}' -H "X-Anofox-Token: $OFF_TOKEN" "http://127.0.0.1:$OFF_PORT/assets/pic.svg")"
[ "$C" = "404" ] || fail "with options.assets left off, /assets/<rest> should be a plain 404 like any unknown path, got $C"
echo "OK: options.assets left off (default) is a plain 404, same as any unknown path"

echo "PASS: authoring-assets"
