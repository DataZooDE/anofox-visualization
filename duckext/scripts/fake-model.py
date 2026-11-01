#!/usr/bin/env python3
"""A fake Messages API for Ask's end-to-end tests (#195): it plays a model
that tries to read everything, records every request, and never calls out.

    fake-model.py <dir>   (writes <dir>/port, appends <dir>/requests.jsonl)

Turn 1 (no tool results yet): get_panel for panels 0..9, then panel 1 with
each value in $FAKE_TRY_VALUES for variable `region`. Turn 2: `answer`, its
text all the tool results it got (so a value it saw reaches the viewer), with
one citation. `x-api-key` must be $FAKE_KEY.
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DIR = sys.argv[1]
KEY = os.environ.get("FAKE_KEY", "")
TRY = [v for v in os.environ.get("FAKE_TRY_VALUES", "").split(",") if v]


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = self.rfile.read(n)
        with open(os.path.join(DIR, "requests.jsonl"), "ab") as f:
            f.write(body.replace(b"\n", b" ") + b"\n")
        if self.headers.get("x-api-key") != KEY or self.path != "/v1/messages":
            return self.send(401, {"type": "error", "error": {"type": "authentication_error"}})
        req = json.loads(body)
        last = req["messages"][-1]
        results = [b for b in last.get("content", []) if isinstance(b, dict) and b.get("type") == "tool_result"]
        if not results:
            calls = [{"type": "tool_use", "id": f"p{i}", "name": "get_panel", "input": {"panel": i}} for i in range(10)]
            calls += [
                {"type": "tool_use", "id": f"v{j}", "name": "get_panel",
                 "input": {"panel": 1, "vars": [{"name": "region", "value": v}]}}
                for j, v in enumerate(TRY)
            ]
            return self.reply(calls)
        seen = " | ".join(str(r.get("content", "")) for r in results)
        return self.reply([{"type": "tool_use", "id": "a", "name": "answer", "input": {
            "text": seen[:1100],
            "citations": [{"panel": 1, "label": "EU", "column": "Revenue", "value": "10",
                           "row": [{"name": "Region", "value": "EU"}]}],
            "scope": "answered"}}])

    def reply(self, blocks):
        self.send(200, {"id": "msg_fake", "type": "message", "role": "assistant", "content": blocks,
                        "stop_reason": "tool_use",
                        "usage": {"input_tokens": 500, "output_tokens": 50}})

    def send(self, status, obj):
        b = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("request-id", "req_fake")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)


s = ThreadingHTTPServer(("127.0.0.1", 0), H)
with open(os.path.join(DIR, "port"), "w") as f:
    f.write(str(s.server_address[1]))
s.serve_forever()
