#!/usr/bin/env bash
# Tests extension-repo.sh against a stub `gh` (no network, no real release):
# a release without a complete set of extensions must FAIL, never yield an
# empty repository that pages.yml would deploy (#1008).
#   usage: scripts/release/test-extension-repo.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/releases"

# The stub: $T/releases/<tag>/ holds a release's assets, $T/releases/LATEST
# names the latest one. A <tag>.pending/ dir is "still uploading": its files
# move into <tag>/ on the third asset listing.
cat > "$T/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
R="$FAKE_RELEASES"
[ "$1 $2" = "release view" ] || [ "$1 $2" = "release list" ] || [ "$1 $2" = "release download" ] || { echo "stub gh: $*" >&2; exit 2; }
cmd="$2"; shift 2
tag=""; case "${1:-}" in -*|"") ;; *) tag="$1"; shift ;; esac
case "$cmd" in
  list) cat "$R/LATEST" 2>/dev/null || true; exit 0 ;;
esac
[ -n "$tag" ] || { [ -f "$R/LATEST" ] || { echo "release not found" >&2; exit 1; }; tag="$(cat "$R/LATEST")"; }
[ -d "$R/$tag" ] || { echo "release not found" >&2; exit 1; }
case "$cmd" in
  view)
    case "$*" in
      *tagName*) echo "$tag" ;;
      *assets*)
        if [ -d "$R/$tag.pending" ]; then
          c=$(( $(cat "$R/$tag.count" 2>/dev/null || echo 0) + 1 )); echo "$c" > "$R/$tag.count"
          [ "$c" -lt 3 ] || { mv "$R/$tag.pending"/* "$R/$tag/"; rmdir "$R/$tag.pending"; }
        fi
        ls "$R/$tag" ;;
    esac ;;
  download)
    pat="" dir="" out=""
    while [ $# -gt 0 ]; do
      case "$1" in --pattern) pat="$2"; shift ;; --dir) dir="$2"; shift ;; --output) out="$2"; shift ;; esac; shift
    done
    hit=0
    for f in "$R/$tag"/*; do
      # shellcheck disable=SC2053  # $pat is a glob on purpose
      [[ "$(basename "$f")" == $pat ]] || continue
      if [ -n "$out" ]; then cp "$f" "$out"; else cp "$f" "$dir/"; fi; hit=1
    done
    [ "$hit" = 1 ] || { echo "no assets match the file pattern" >&2; exit 1; } ;;
esac
STUB
chmod +x "$T/bin/gh"
export PATH="$T/bin:$PATH" FAKE_RELEASES="$T/releases" EXT_REPO_POLL=0

ext() { echo "anofox_visualization.$1.duckdb-v1.5.6.$2.duckdb_extension"; }
# release <tag> <platform>... : a complete release (extensions + CLI + SHA256SUMS)
release() {
  local tag="$1"; shift; mkdir -p "$T/releases/$tag"
  for p in "$@"; do echo "$tag $p" > "$T/releases/$tag/$(ext "$tag" "$p")"; done
  echo cli > "$T/releases/$tag/anofox-visualization-cli-$tag.tar.gz"
  (cd "$T/releases/$tag" && sha256sum -- * > SHA256SUMS)
}

fails=0
# check <name> <expected exit> <expected .gz count> <args...>
check() {
  local name="$1" want="$2" want_n="$3"; shift 3
  local out="$T/out-$name" got=0 n
  "$HERE/extension-repo.sh" "$@" "$out" > "$T/log-$name" 2>&1 || got=$?
  n="$( (find "$out" -name '*.gz' 2>/dev/null || true) | wc -l)"
  if [ "$got" = "$want" ] && [ "$n" = "$want_n" ]; then
    echo "ok   $name (exit $got, $n extension(s))"
  else
    echo "FAIL $name: exit $got (want $want), $n extension(s) (want $want_n)"; sed 's/^/     /' "$T/log-$name"; fails=$((fails + 1))
  fi
}

# No published release at all: first bootstrap, the app still deploys.
check bootstrap 0 0 --release latest

release v1 linux_amd64 osx_arm64
echo v1 > "$T/releases/LATEST"
check complete-latest 0 2 --release latest
check complete-tag 0 2 --release v1
gunzip -c "$T/out-complete-tag/v1.5.6/linux_amd64/anofox_visualization.duckdb_extension.gz" | grep -qx "v1 linux_amd64" \
  || { echo "FAIL complete-tag: wrong layout/content"; fails=$((fails + 1)); }

# #1008: the latest release was just (re)created and has no assets yet.
mkdir -p "$T/releases/v2"; echo v2 > "$T/releases/LATEST"
check empty-latest 1 0 --release latest
check missing-tag 1 0 --release v0-nonexistent

# Assets but no extensions (like the wasm-ext-* pre-releases).
mkdir -p "$T/releases/wasm"; echo w > "$T/releases/wasm/wasm_eh-x.duckdb_extension.wasm"
check no-extensions 1 0 --release wasm

# Mid-upload: SHA256SUMS lists two extensions, one is not uploaded yet.
release v3 linux_amd64 osx_arm64; rm "$T/releases/v3/$(ext v3 osx_arm64)"
check partial 1 0 --release v3

# An extension that does not match SHA256SUMS.
release v4 linux_amd64; echo tampered > "$T/releases/v4/$(ext v4 linux_amd64)"
check checksum 1 0 --release v4

# Still uploading when the run starts, complete within EXT_REPO_WAIT.
release v5 linux_amd64 osx_arm64; mkdir "$T/releases/v5.pending"; mv "$T/releases/v5"/* "$T/releases/v5.pending/"
EXT_REPO_WAIT=60 check wait-then-complete 0 2 --release v5
grep -q "polling again" "$T/log-wait-then-complete" || { echo "FAIL wait-then-complete: did not poll"; fails=$((fails + 1)); }

# A local assets dir without extensions.
mkdir -p "$T/empty"
check empty-dir 1 0 "$T/empty"

[ "$fails" = 0 ] || { echo "$fails failure(s)"; exit 1; }
echo "== extension-repo.sh: all checks passed =="
