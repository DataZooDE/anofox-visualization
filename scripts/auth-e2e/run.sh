#!/usr/bin/env bash
# End-to-end test of the built-in login (#50) in a real browser: a local
# OIDC/JWKS test IdP (idp.mjs, HTTPS with a throwaway CA), a locked server
# with every P3 method (oidc, jwt, embed, api_keys) and p3.mjs (Playwright).
#
#   NODE_PATH=<dir with playwright>/node_modules scripts/auth-e2e/run.sh <extension>
#
# <extension>: a native build (duckext/scripts/build-native.sh). Needs the
# `serve` binary (cargo build --features serve --bin serve), node, openssl and
# DuckDB 1.5 (DUCKDB, default `duckdb`). Ports: PORT (8950), IDP_PORT (8953).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
EXT="$(cd "$(dirname "${1:?usage: run.sh <extension>}")" && pwd)/$(basename "$1")"
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
 {"mode":"oidc","issuer":"$ISS","client_id":"dash","client_secret_env":"OIDC_SECRET","redirect_url":"http://localhost:$PORT/auth/callback","groups_claim":"groups","ca_file":"$WORK/ca.pem","session_secret_env":"SESSION_SECRET"},
 {"mode":"jwt","issuer":"$ISS","audience":"dash-api","groups_claim":"groups","ca_file":"$WORK/ca.pem"},
 {"mode":"embed","secret_env":"EMBED_SECRET","claims":["tenant"]},
 {"mode":"api_keys","file":"$WORK/keys.json"}],
 "audit":"$WORK/audit.jsonl","log":"$WORK/requests.jsonl"}
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
echo "auth e2e ok — logs and screenshots in $WORK"
