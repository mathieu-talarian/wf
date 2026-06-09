#!/usr/bin/env python3
"""Phase 6 parity harness: diff the TS vs Rust backend JSON responses per §14
endpoint, using the same Supabase JWT against the same database.

The Rust port is meant to be byte-for-byte compatible with the TS server's JSON
contract, so this drives both servers and reports per-endpoint matches/diffs.

Usage:
    # start the TS server (../workflow/apps/server) and the Rust server, then:
    JWT="$SUPABASE_ACCESS_TOKEN" \
    RUST_BASE=http://localhost:3000/api \
    TS_BASE=http://localhost:3001/api \
    python3 scripts/parity.py [--include-writes]

Exit code is non-zero if any compared endpoint diverges. Read-only GET endpoints
run by default; mutating endpoints are skipped unless --include-writes is given
(they have side effects, so point at a throwaway account).

Volatile fields that legitimately differ between two live calls (timestamps,
the health clock) are normalized out before comparison — see VOLATILE_KEYS.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request

# Fields that change between two independent live calls and so are not part of
# the contract being verified. Compared structurally, not by value.
VOLATILE_KEYS = {
    "time",            # /health clock
    "lastUsedAt",      # bumped on every data read
    "lastValidatedAt",
    "updatedAt",
    "connectedAt",
    "createdAt",
}

# (method, path, query, body, requires_connection)
# `requires_connection` endpoints only produce comparable data when the account
# actually has the integration connected; otherwise both servers should still
# agree (e.g. both return the disconnected/empty shape or the same 4xx).
READ_ENDPOINTS = [
    ("GET", "/health", None, None),
    ("GET", "/hello/parity", None, None),
    ("GET", "/me", None, None),
    # GitHub
    ("GET", "/me/github", None, None),
    ("GET", "/me/github/dashboard", {"tab": "assigned"}, None),
    ("GET", "/me/github/queue", {"key": "assigned"}, None),
    ("GET", "/me/github/repos", None, None),
    ("GET", "/me/github/branches", None, None),
    ("GET", "/me/github/workflows", None, None),
    ("GET", "/me/github/favorites", None, None),
    # Jira
    ("GET", "/me/jira", None, None),
    ("GET", "/me/jira/dashboard", None, None),
    ("GET", "/me/jira/projects", None, None),
    ("GET", "/me/jira/boards", None, None),
]

# Mutating endpoints — only with --include-writes (side effects!).
WRITE_ENDPOINTS = [
    ("POST", "/me/github/token/validate", None, None),
    ("POST", "/me/jira/token/validate", None, None),
]


def normalize(value):
    """Recursively sort dict keys and blank out volatile fields so two live
    responses can be compared for structural + value parity."""
    if isinstance(value, dict):
        return {
            k: ("<volatile>" if k in VOLATILE_KEYS else normalize(v))
            for k, v in sorted(value.items())
        }
    if isinstance(value, list):
        return [normalize(v) for v in value]
    return value


def request(base: str, method: str, path: str, query, body, jwt: str):
    url = base.rstrip("/") + path
    if query:
        from urllib.parse import urlencode

        url += "?" + urlencode(query)
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("accept", "application/json")
    if data is not None:
        req.add_header("content-type", "application/json")
    if jwt:
        req.add_header("authorization", f"Bearer {jwt}")
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read().decode()
            status = resp.status
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        status = e.code
    except Exception as e:  # noqa: BLE001 - report transport failures inline
        return None, {"_transport_error": str(e)}
    try:
        parsed = json.loads(raw) if raw else None
    except json.JSONDecodeError:
        parsed = {"_non_json": raw}
    return status, parsed


def diff_lines(a, b) -> list[str]:
    import difflib

    ta = json.dumps(a, indent=2, sort_keys=True).splitlines()
    tb = json.dumps(b, indent=2, sort_keys=True).splitlines()
    return list(difflib.unified_diff(ta, tb, "ts", "rust", lineterm=""))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--include-writes", action="store_true")
    args = ap.parse_args()

    jwt = os.environ.get("JWT", "")
    ts_base = os.environ.get("TS_BASE", "http://localhost:3001/api")
    rust_base = os.environ.get("RUST_BASE", "http://localhost:3000/api")
    if not jwt:
        print("warning: JWT not set — authed endpoints will compare 401s only\n")

    endpoints = list(READ_ENDPOINTS)
    if args.include_writes:
        endpoints += WRITE_ENDPOINTS

    failures = 0
    for method, path, query, body in endpoints:
        ts_status, ts_body = request(ts_base, method, path, query, body, jwt)
        rust_status, rust_body = request(rust_base, method, path, query, body, jwt)
        label = f"{method} {path}"

        if ts_status != rust_status:
            print(f"FAIL {label}: status ts={ts_status} rust={rust_status}")
            failures += 1
            continue

        na, nb = normalize(ts_body), normalize(rust_body)
        if na == nb:
            print(f"PASS {label}  ({ts_status})")
        else:
            print(f"FAIL {label}: body differs ({ts_status})")
            for line in diff_lines(na, nb)[:40]:
                print("    " + line)
            failures += 1

    print(f"\n{len(endpoints) - failures}/{len(endpoints)} endpoints match.")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
