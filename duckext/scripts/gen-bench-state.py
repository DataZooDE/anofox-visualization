#!/usr/bin/env python3
"""Generate a synthetic options.state store (JSONL) with N rows, matching
Record::to_json's on-disk shape, for bench-trainer-readback.sh (#600) —
bypasses N real HTTP submissions (and their own per-row fdatasync cost,
unrelated to what that benchmark measures) in favour of writing the file
directly.

`ts` must match src/host/access.rs::rfc3339's exact fixed format
(YYYY-MM-DDTHH:MM:SS.mmmZ) — the server's own parse_rfc3339_micros (#600)
only ever has to parse that one shape, not general RFC3339, so this script
must produce exactly it too."""
import json, sys, datetime

path = sys.argv[1]
n = int(sys.argv[2])
users = int(sys.argv[3]) if len(sys.argv) > 3 else 500


def rfc3339(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%S.") + f"{dt.microsecond // 1000:03d}Z"


with open(path, "w") as f:
    f.write(
        json.dumps(
            {"anofox_state": 1, "created": rfc3339(datetime.datetime.now(datetime.timezone.utc))}
        )
        + "\n"
    )
    for i in range(1, n + 1):
        rec = {
            "v": 1,
            "seq": i,
            "ts": rfc3339(datetime.datetime.now(datetime.timezone.utc)),
            "user": f"user{i % users}",
            "auth": "proxy",
            "dashboard": "quiz",
            "submit": "quiz_1",
            "id": f"{i:032x}",
            "vars": {"q1": "an answer text, not a single char", "q2": i},
        }
        f.write(json.dumps(rec) + "\n")
