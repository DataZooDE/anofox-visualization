#!/usr/bin/env bash
# Compile csrc/anofox_visualization_extension.cpp with stray DuckDB headers on the
# include path, the way a machine with a system DuckDB package (/usr/include/duckdb)
# sees it. Ported from anofox-forecast (DataZooDE/anofox-forecast#312).
#
# A version probe such as __has_include("duckdb/parser/token_iterator.hpp") asks
# whether a header exists ANYWHERE on the include path. When the bundled DuckDB
# lacks it (1.5.6 has no token_iterator.hpp), the compiler keeps searching and finds
# a copy from another DuckDB version in the system include directories, e.g. a
# system DuckDB 2.0. This script plants a stub that fails with #error for every
# header that either (a) another supported DuckDB line has and the bundled one
# lacks, or (b) the sources probe with __has_include and the bundled one lacks. The
# stubs go on -isystem, which is searched after the bundled tree (-I) and before the
# real system directories, so a stub also wins over a system DuckDB on a developer
# machine. A header that is found there fails the compile.
#
# Usage: scripts/check_compat_stray_headers.sh   (CXX and DUCKDB_INCLUDE override)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
duckdb_include=${DUCKDB_INCLUDE:-$root/duckdb/src/include}
cxx=${CXX:-c++}

if [ ! -f "$duckdb_include/duckdb.hpp" ]; then
	echo "no DuckDB headers at $duckdb_include (git submodule update --init duckdb)" >&2
	exit 2
fi
if [ ! -f "$root/datazoo-banner/include/datazoo_banner_duckdb.hpp" ]; then
	echo "no banner headers (git submodule update --init datazoo-banner)" >&2
	exit 2
fi

stray=$(mktemp -d)
trap 'rm -rf "$stray"' EXIT

# (a) Headers of DuckDB 2.0 that 1.5.x lacks and that the extension once probed for.
known="duckdb/parser/token_iterator.hpp"
# (b) Every header the sources probe for.
probed=$(grep -rhoE '__has_include[[:space:]]*\([[:space:]]*["<]duckdb[^">]*[">]' "$root/csrc" "$root/src/include" |
	sed -E 's/.*["<](duckdb[^">]*)[">]$/\1/' || true)

planted=0
for header in $(printf '%s\n' $known $probed | sort -u); do
	if [ ! -e "$duckdb_include/$header" ]; then
		mkdir -p "$stray/$(dirname "$header")"
		printf '#error "stray DuckDB header %s: not in the bundled DuckDB, found on a later include path"\n' \
			"$header" >"$stray/$header"
		echo "planted stray $header"
		planted=$((planted + 1))
	fi
done

if [ "$planted" -eq 0 ]; then
	echo "warning: no stray headers planted; the check below tests nothing" >&2
fi

"$cxx" -std=c++17 -fsyntax-only -I"$duckdb_include" -I"$root/src/include" -I"$root/datazoo-banner/include" \
	-isystem "$stray" "$root/csrc/anofox_visualization_extension.cpp"
echo "anofox_visualization_extension.cpp compiles with $planted stray header(s) on the include path"
