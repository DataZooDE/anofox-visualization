#!/usr/bin/env bash
# End-to-end test of share links (#763) in a real browser: a locked server
# whose only login is `{"mode": "share"}` — one public dashboard, one behind
# a password — and share.mjs (Playwright); then the same against the `serve`
# binary's static pages with curl.
#
#   NODE_PATH=<dir with playwright>/node_modules scripts/share-e2e/run.sh <extension>
#
# <extension>: a native build (duckext/scripts/build-native.sh). Needs the
# `serve` binary (cargo build --features serve --bin serve), node and DuckDB
# 1.5 (DUCKDB, default `duckdb`). Ports: PORT (8956), BIN_PORT (8957).
set -euo pipefail
fail() { echo "FAIL: $*" >&2; exit 1; }
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
AUTH="$REPO/scripts/auth-e2e"
EXT="$(cd "$(dirname "${1:?usage: run.sh <extension>}")" && pwd)/$(basename "$1")"
DUCKDB="${DUCKDB:-duckdb}"
SERVE="$(realpath "${SERVE:-$REPO/target/debug/serve}")"
export PORT="${PORT:-8956}" BIN_PORT="${BIN_PORT:-8957}"
export WORK="$(mktemp -d "${TMPDIR:-/tmp}/anofox-share-e2e.XXXXXX")"
export SHARE_SESSION="$(openssl rand -hex 32)"
export PASSWORD="$("$SERVE" share-password --generate | sed -n 's/^password: \([^ ]*\).*/\1/p')"
HASH="$(printf '%s\n' "$PASSWORD" | "$SERVE" share-password | sed -n 's/^password_hash: //p')"
[ -n "$PASSWORD" ] && [ -n "$HASH" ] || fail "serve share-password"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT INT TERM
cd "$WORK"

OPTS=$(tr -d '\n' <<J
{"auth":{"mode":"share","session_secret_env":"SHARE_SESSION","dashboards":{
 "rls":{"access":"password","password_hash":"$HASH","claims":{"tenant":"acme"}},
 "other":{"access":"public"}}},
 "policy":"none","audit":"$WORK/audit.jsonl","log":"$WORK/requests.jsonl"}
J
)
printf "LOAD '%s';\n.read %s\nINSERT INTO entitlements VALUES ('share:rls', 'EU');\nSELECT anofox_serve_dashboards('%s', %s, '%s');\n" \
  "$EXT" "$AUTH/setup.sql" "$AUTH/dash" "$PORT" "$OPTS" > serve.sql
# The CLI keeps serving while its stdin stays open.
( echo "$BASHPID" > feeder.pid; cat serve.sql; exec sleep 3600 ) 2>/dev/null |
  "$DUCKDB" -unsigned > srv.out 2> srv.err &
PIDS+=($!)
sleep 0.2
PIDS+=("$(cat feeder.pid)")
for _ in $(seq 60); do curl -fsS "http://127.0.0.1:$PORT/healthz" > /dev/null 2>&1 && break; sleep 0.5; done
curl -fsS "http://127.0.0.1:$PORT/healthz" > /dev/null || { cat srv.err; fail "server did not start"; }

node "$HERE/share.mjs"

# serve --dashboards (plain pages, no script): the same two dashboards.
"$SERVE" --dashboards "$AUTH/bindash" --db "$WORK/bin.duckdb" --init /dev/null --port "$BIN_PORT" --no-open \
  --auth "{\"mode\":\"share\",\"dashboards\":{\"p\":{\"access\":\"password\",\"password_hash\":\"$HASH\"}}}" \
  --audit "$WORK/bin-audit.jsonl" > bin.out 2>&1 &
PIDS+=($!)
B="http://127.0.0.1:$BIN_PORT"
for _ in $(seq 40); do curl -s -o /dev/null "$B/d/p" && break; sleep 0.25; done
page=$(curl -s "$B/d/p")
grep -q 'action="/share/p"' <<<"$page" || fail "static server: no password page: $page"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H "Origin: $B" --data-urlencode "password=wrong" "$B/share/p")" = 401 ] \
  || fail "static server: a wrong password must be refused"
hdr=$(curl -s -o /dev/null -D - -H "Origin: $B" --data-urlencode "password=$PASSWORD" "$B/share/p")
grep -qi '^location: /d/p' <<<"$hdr" || fail "static server: login should go back to /d/p: $hdr"
cookie=$(grep -i '^set-cookie:' <<<"$hdr" | sed 's/^[Ss]et-[Cc]ookie: //; s/;.*//' | tr -d '\r')
[ -n "$cookie" ] || fail "static server: no cookie"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H "Cookie: $cookie" "$B/d/p")" = 200 ] \
  || fail "static server: the cookie should open /d/p"
curl -s -H "Cookie: $cookie" "$B/d/p" | grep -q 'action="/share/p"' && fail "static server: still the password page"
[ "$(curl -s -o /dev/null -w '%{http_code}' -H "Cookie: $cookie" "$B/")" = 401 ] \
  || fail "static server: the share cookie is no login for the rest"
grep -q "$PASSWORD" "$WORK"/*.jsonl && fail "a password reached a log"
echo "share e2e ok — logs and screenshots in $WORK"
