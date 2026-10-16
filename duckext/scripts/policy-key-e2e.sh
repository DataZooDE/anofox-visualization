#!/usr/bin/env bash
# End-to-end check of the access policy's `hash` mask key (#49, #105): start
# a locked server with a known key (policy_key_env) and look for the key on
# every text surface a viewer or an operator can reach — panel results, a
# failing panel's error, ::PAGED profiles, the dashboard page, /healthz, the
# server's stderr and the request log — and check that the dashboard SQL that
# could show it (duckdb_views(), current_query(), EXPLAIN,
# json_serialize_plan) is refused at start. Also checks the hash is
# HMAC-SHA256(key, column id ‖ 0 ‖ value) and that `auth` without a policy
# refuses to start.
#
#   duckext/scripts/policy-key-e2e.sh <anofox_visualization.duckdb_extension> [port]
#
# Needs the pinned duckdb CLI (DUCKDB, default `duckdb`), curl and python3.
set -euo pipefail
EXT=$(realpath "$1")
PORT=${2:-8880}
DUCKDB=${DUCKDB:-duckdb}
W=$(mktemp -d)
PIDS=()
cleanup() {
    for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
    rm -rf "$W"
}
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

KEY="keyprobe$(python3 -c 'import secrets; print(secrets.token_hex(20))')"
SEC="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
mkdir -p "$W/dash" "$W/bad"
cat > "$W/data.sql" <<'EOF'
CREATE TABLE emp AS SELECT * FROM (VALUES (1, 'Ann', 9000), (2, 'Ben', 7000)) t(id, name, salary);
EOF
cat > "$W/policy.json" <<'EOF'
{"tables": {"emp": {"rows": true, "columns": {"name": "hash", "salary": "deny"}}}}
EOF
cp "$W/policy.json" "$W/dash/"
cat > "$W/dash/d.sql" <<'EOF'
SELECT id::TABLE, name FROM emp ORDER BY id;
SELECT id::TABLE, salary FROM emp;
SELECT name::TABLE, CAST(name AS INTEGER) AS boom FROM emp;
EOF
AUTH='"auth": {"mode": "proxy", "shared_secret_env": "SEC", "trusted_proxies": ["127.0.0.1"]}'
OPTS="{$AUTH, \"policy_key_env\": \"KEY\", \"log\": \"$W/requests.jsonl\"}"

# 1. SQL that could show the key is refused at start.
i=0
for q in "SELECT sql::TABLE FROM duckdb_views()" "SELECT current_query()::TABLE" \
    "SELECT json_serialize_plan('SELECT * FROM temp.main.__anofox_rls_0')::TABLE" \
    "SELECT name::TABLE FROM emp WHERE current_setting('threads') IS NOT NULL"; do
    i=$((i + 1)); mkdir -p "$W/bad/$i"; cp "$W/policy.json" "$W/bad/$i/"
    echo "$q;" > "$W/bad/$i/x.sql"
    out=$(KEY=$KEY SEC=$SEC "$DUCKDB" -unsigned -c "$(cat "$W/data.sql") LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/bad/$i', $PORT, '$OPTS');" 2>&1 || true)
    grep -q "refused by the access policy" <<<"$out" || fail "not refused: $q → $out"
done
mkdir -p "$W/nopol"; echo "SELECT 1::TABLE;" > "$W/nopol/x.sql"
out=$(SEC=$SEC "$DUCKDB" -unsigned -c "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/nopol', $PORT, '{$AUTH}');" 2>&1 || true)
grep -q "no access policy was found" <<<"$out" || fail "auth without a policy started: $out"

# 2. A running server: the key is on no surface.
mkfifo "$W/in"
sleep 120 > "$W/in" & PIDS+=($!)
{ cat "$W/data.sql"; echo "LOAD '$EXT'; SELECT anofox_serve_dashboards('$W/dash', $PORT, '$OPTS');"; } > "$W/serve.sql"
KEY=$KEY SEC=$SEC "$DUCKDB" -unsigned -init "$W/serve.sql" < "$W/in" > "$W/stderr.log" 2>&1 & PIDS+=($!)
for _ in $(seq 50); do curl -s -m 1 "localhost:$PORT/health" > /dev/null && break; sleep 0.2; done
H=(-H "X-Anofox-Proxy-Secret: $SEC" -H "X-Forwarded-User: alice")
: > "$W/surfaces.txt"
for b in '{"dashboard":"d","panel":0}' '{"dashboard":"d","panel":1}' '{"dashboard":"d","panel":2}' \
    '{"dashboard":"d","panel":0,"page":{"profile":["name"]}}' '{"dashboard":"d","panel":0,"page":{"distinct":"name"}}'; do
    curl -s -X POST "localhost:$PORT/api/panel" "${H[@]}" -H 'Content-Type: application/json' -d "$b" >> "$W/surfaces.txt"
    echo >> "$W/surfaces.txt"
done
curl -s "localhost:$PORT/d/d" "${H[@]}" >> "$W/surfaces.txt"
curl -s "localhost:$PORT/healthz" "${H[@]}" >> "$W/surfaces.txt"
curl -s "localhost:$PORT/" "${H[@]}" >> "$W/surfaces.txt"
sleep 0.5
grep -q "could not be shown" "$W/surfaces.txt" || fail "a failing panel did not get the generic error"
for f in surfaces.txt stderr.log requests.jsonl; do
    ! grep -q "$KEY" "$W/$f" || fail "the key appears in $f"
done
# The served hash is HMAC-SHA256(key, column id ‖ 0 ‖ value), first 8 bytes.
want=$(KEY=$KEY python3 -c 'import hmac, hashlib, os
print(hmac.new(os.environ["KEY"].encode(), b"[\"main\",\"emp\",\"name\"]\x00Ann", hashlib.sha256).hexdigest()[:16])')
grep -q "\"name\":\"$want\"" "$W/surfaces.txt" || fail "hash of Ann is not $want: $(head -c 300 "$W/surfaces.txt")"
echo "OK: key on no surface; HMAC as specified; key-revealing SQL and auth without a policy refused"
