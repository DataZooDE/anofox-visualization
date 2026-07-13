#!/usr/bin/env python3
"""Tiny static server for the anofox-visualization builder that disables caching, so edits
show up on a normal refresh. Usage: python3 serve_nocache.py [port]"""
import functools
import http.server
import os
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8000
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "web")


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store, must-revalidate")
        self.send_header("Pragma", "no-cache")
        super().end_headers()


http.server.HTTPServer(
    ("127.0.0.1", PORT),
    functools.partial(Handler, directory=ROOT),
).serve_forever()
