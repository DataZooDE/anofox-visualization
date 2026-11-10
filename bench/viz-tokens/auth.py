"""The bench's Claude API credentials (#1075): an API key, or Workload Identity
Federation (WIF) on a GitHub Actions runner — no stored secret.

- `ANTHROPIC_API_KEY` set: sent as `x-api-key` (local runs).
- Else `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID` and
  `ANTHROPIC_SERVICE_ACCOUNT_ID` set (+ `ANTHROPIC_WORKSPACE_ID` when the rule
  spans several workspaces): a GitHub OIDC token (audience
  https://api.anthropic.com, fetched fresh for every exchange — a token is
  single-use and lives about 5 minutes), exchanged at POST /v1/oauth/token
  (RFC 7523 jwt-bearer) for a short-lived access token, sent as
  `Authorization: Bearer` and re-exchanged 2 minutes before it expires. Outside
  Actions, `ANTHROPIC_IDENTITY_TOKEN_FILE` holds the JWT instead.
"""
import json
import os
import threading
import time
import urllib.parse
import urllib.request

BASE = "https://api.anthropic.com"
AUDIENCE = "https://api.anthropic.com"
_FED = ("ANTHROPIC_FEDERATION_RULE_ID", "ANTHROPIC_ORGANIZATION_ID", "ANTHROPIC_SERVICE_ACCOUNT_ID")
_lock = threading.Lock()
_cache = {"token": None, "until": 0.0}


def available():
    """Is there a credential to call the API with?"""
    return bool(os.environ.get("ANTHROPIC_API_KEY")) or all(os.environ.get(k) for k in _FED)


def _identity_token():
    url, bearer = os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL"), os.environ.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
    if url and bearer:
        sep = "&" if "?" in url else "?"
        req = urllib.request.Request(f"{url}{sep}audience={urllib.parse.quote(AUDIENCE, safe='')}",
                                     headers={"Authorization": f"Bearer {bearer}"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)["value"]
    path = os.environ.get("ANTHROPIC_IDENTITY_TOKEN_FILE")
    if path:
        return open(path).read().strip()
    raise SystemExit("WIF: no identity token (not on a GitHub Actions runner with id-token: write, "
                     "and ANTHROPIC_IDENTITY_TOKEN_FILE is unset)")


def _exchange():
    body = {"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer", "assertion": _identity_token(),
            "federation_rule_id": os.environ["ANTHROPIC_FEDERATION_RULE_ID"],
            "organization_id": os.environ["ANTHROPIC_ORGANIZATION_ID"],
            "service_account_id": os.environ["ANTHROPIC_SERVICE_ACCOUNT_ID"]}
    if os.environ.get("ANTHROPIC_WORKSPACE_ID"):
        body["workspace_id"] = os.environ["ANTHROPIC_WORKSPACE_ID"]
    req = urllib.request.Request(BASE + "/v1/oauth/token", data=json.dumps(body).encode(), method="POST",
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        t = json.load(r)
    return t["access_token"], time.time() + int(t.get("expires_in", 600)) - 120


def headers():
    """The auth header(s) for one request."""
    key = os.environ.get("ANTHROPIC_API_KEY")
    if key:
        return {"x-api-key": key}
    with _lock:
        if not _cache["token"] or time.time() >= _cache["until"]:
            _cache["token"], _cache["until"] = _exchange()
        return {"authorization": f"Bearer {_cache['token']}"}


def invalidate():
    """Drop the cached token (after a 401), so the next request re-exchanges."""
    with _lock:
        _cache["token"] = None
