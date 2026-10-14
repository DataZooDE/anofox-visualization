#!/usr/bin/env python3
"""Drop a duplicated trailing `duckdb_signature` custom section from a DuckDB
wasm extension, in place.

The get.erpl.io v1.5.6 wasm builds of anofox_statistics / anofox_forecast carry
the signature section twice (DataZooDE/anofox-statistics#192,
DataZooDE/anofox-forecast#272), and DuckDB-Wasm's LOAD then fails with
"Unknown ABI type". The second copy is removed only when there are exactly two
and the last one ends the file, so a fixed upstream build passes through
untouched. Exits non-zero unless the result has exactly one signature section.

usage: strip_wasm_signature.py <file.wasm> [more.wasm …]
"""
import sys


def leb(b, i):
    r = s = 0
    while True:
        x = b[i]
        i += 1
        r |= (x & 0x7F) << s
        s += 7
        if x < 0x80:
            return r, i


def signature_sections(b):
    if b[:4] != b"\0asm":
        raise ValueError("not a wasm module")
    i, out = 8, []
    while i < len(b):
        start, sid = i, b[i]
        size, i = leb(b, i + 1)
        end = i + size
        if sid == 0:
            nl, j = leb(b, i)
            if b[j : j + nl] == b"duckdb_signature":
                out.append((start, end))
        i = end
    return out


def fix(path):
    b = open(path, "rb").read()
    sig = signature_sections(b)
    if len(sig) == 2 and sig[-1][1] == len(b):
        b = b[: sig[-1][0]]
        open(path, "wb").write(b)
        print(f"{path}: dropped the duplicate duckdb_signature section")
    n = len(signature_sections(b))
    if n != 1:
        sys.exit(f"{path}: expected 1 duckdb_signature section, found {n}")
    print(f"{path}: ok ({len(b)} bytes, 1 signature section)")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for p in sys.argv[1:]:
        fix(p)
