#!/usr/bin/env bash
# End-to-end test of the built-in login (#50) in a real browser: a local
# OIDC/JWKS test IdP (idp.mjs, HTTPS with a throwaway CA), a locked server
# with every P3 method (oidc, jwt, embed, api_keys) and p3.mjs (Playwright).
#
#   NODE_PATH=<dir with playwright>/node_modules scripts/auth-e2e/run.sh <extension>
#
# <extension>: a native build (duckext/scripts/build-native.sh). Needs the
# `serve` binary (cargo build --features serve --bin serve), node, openssl and
# DuckDB 1.5 (DUCKDB, default `duckdb`). Ports: PORT (8950), IDP_PORT (8953),
# BIN_PORT (8954, the `serve` binary).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
EXT="$(cd "$(dirname "${1:?usage: run.sh <extension>}")" && pwd)/$(basename "$1")"
case "$(basename "$EXT")" in
  anofox_visualization.duckdb_extension|anofox_visualization.*.duckdb_extension) ;;
  *) echo "$EXT: LOAD takes the entry point from the file name, so it must be named anofox_visualization.duckdb_extension (or anofox_visualization.<x>.duckdb_extension). Rename it, or: ln -s '$EXT' /tmp/anofox_visualization.duckdb_extension" >&2; exit 2 ;;
esac
DUCKDB="${DUCKDB:-duckdb}"
SERVE="${SERVE:-$REPO/target/debug/serve}"
export PORT="${PORT:-8950}" IDP_PORT="${IDP_PORT:-8953}"
export WORK="$(mktemp -d "${TMPDIR:-/tmp}/anofox-auth-e2e.XXXXXX")"
export OIDC_SECRET="$(openssl rand -hex 16)" EMBED_SECRET="$(openssl rand -hex 32)" SESSION_SECRET="$(openssl rand -hex 32)"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do pkill -P "$p" 2>/dev/null || true; kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
cd "$WORK"

# A throwaway CA and the IdP's certificate (127.0.0.1 / localhost).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj "/CN=auth-e2e test CA" \
  -keyout ca.key -out ca.pem -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "/CN=localhost" -keyout idp.key -out idp.csr 2>/dev/null
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' > ext.cnf
openssl x509 -req -in idp.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 1 -extfile ext.cnf -out idp.pem 2>/dev/null

ISS="https://127.0.0.1:$IDP_PORT"
ISSUER="$ISS" IDP_KEY=idp.key IDP_CERT=idp.pem node "$HERE/idp.mjs" > idp.log 2>&1 &
PIDS+=($!)

# Two API keys; the second is revoked straight away.
"$SERVE" api-key new --file keys.json --user svc-etl --dashboards rls --audit audit.jsonl > apikey.txt
"$SERVE" api-key new --file keys.json --user revoked-svc --audit audit.jsonl > apikey2.txt
"$SERVE" api-key revoke --file keys.json --id "$(cut -c5-16 apikey2.txt)" --audit audit.jsonl > /dev/null

OPTS=$(tr -d '\n' <<J
{"auth":[
 {"mode":"oidc","issuer":"$ISS","client_id":"dash","client_secret_env":"OIDC_SECRET","redirect_url":"http://localhost:$PORT/auth/callback","groups_claim":"groups","ca_file":"$WORK/ca.pem","session_secret_env":"SESSION_SECRET","revocation_file":"$WORK/revoked.jsonl"},
 {"mode":"jwt","issuer":"$ISS","audience":"dash-api","groups_claim":"groups","ca_file":"$WORK/ca.pem"},
 {"mode":"embed","secret_env":"EMBED_SECRET","claims":["tenant"]},
 {"mode":"api_keys","file":"$WORK/keys.json"}],
 "policy":"none","audit":"$WORK/audit.jsonl","log":"$WORK/requests.jsonl"}
J
)
# The CLI keeps serving while its stdin stays open (the feeder's sleep).
( echo "$BASHPID" > feeder.pid
  printf "LOAD '%s';\n.read %s\nSELECT anofox_serve_dashboards('%s', %s, '%s');\n" \
    "$EXT" "$HERE/setup.sql" "$HERE/dash" "$PORT" "$OPTS"; exec sleep 3600 ) 2>/dev/null |
  "$DUCKDB" -unsigned > srv.out 2> srv.err &
PIDS+=($!)
sleep 0.2
PIDS+=("$(cat feeder.pid)")
for _ in $(seq 60); do curl -fsS "http://127.0.0.1:$PORT/healthz" > /dev/null 2>&1 && break; sleep 0.5; done

node "$HERE/p3.mjs"

# #514 PR B: `GET /d/<id>/assets/<rest>` sits behind the exact same auth and
# scope as the dashboard page itself — no code path reaches it before the
# login/scope checks above, so this is really checking that construction,
# not re-implementing auth for the new route. `dash/assets/rls-logo.svg` is
# a committed fixture; `svc-etl`'s api_keys credential (apikey.txt) is
# scoped to `rls` only (see the `api-key new --dashboards rls` above).
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }
APIKEY="$(cat apikey.txt)"
[ "$(code "http://127.0.0.1:$PORT/d/rls/assets/rls-logo.svg")" = 401 ] \
  || fail "anonymous asset request should need login, like the page"
[ "$(code -H "Authorization: Bearer $APIKEY" "http://127.0.0.1:$PORT/d/rls/assets/rls-logo.svg")" = 200 ] \
  || fail "rls-scoped key should read rls's own asset"
[ "$(code -H "Authorization: Bearer $APIKEY" "http://127.0.0.1:$PORT/d/other/assets/rls-logo.svg")" = 403 ] \
  || fail "rls-scoped key must not read another dashboard's asset (scope, not file existence)"

# serve --dashboards (no script): the embed flow with a one-time link.
export BIN_PORT="${BIN_PORT:-8954}"
"$SERVE" --dashboards "$HERE/bindash" --db "$WORK/bin.duckdb" --init /dev/null --port "$BIN_PORT" --no-open \
  --auth '{"mode":"embed","secret_env":"EMBED_SECRET","one_time":true}' --log "$WORK/bin-req.jsonl" > bin.out 2>&1 &
PIDS+=($!)
for _ in $(seq 40); do curl -s -o /dev/null "http://127.0.0.1:$BIN_PORT/d/p" && break; sleep 0.25; done
node "$HERE/bin.mjs"
echo "auth e2e ok — logs and screenshots in $WORK"
