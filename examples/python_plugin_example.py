#!/usr/bin/env python3
"""
Example runway-selector plugin written in Python (stdlib only — no pip needed).

es_runway_selector will run this automatically via mise when plugin_binary ends
in .py.  Configure in config.toml:
    plugin_binary = "/path/to/python_plugin_example.py"

mise installs Python automatically if it is not already present, so neither
Python nor any package manager needs to be installed by the user.

The plugin receives two ports:
  --port N          listen here for POST /runway-selections and GET /health
  --helpers-port M  call http://127.0.0.1:M/helpers/* for selection helpers
"""

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.error import URLError
from urllib.request import Request, urlopen

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--helpers-port", type=int, required=True, dest="helpers_port")
args = parser.parse_args()

HELPERS = f"http://127.0.0.1:{args.helpers_port}/helpers"


# ---------------------------------------------------------------------------
# Thin wrappers around the helpers server
# ---------------------------------------------------------------------------

def _post(path: str, payload: dict) -> dict:
    body = json.dumps(payload).encode()
    req = Request(
        f"{HELPERS}{path}",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    with urlopen(req, timeout=5) as resp:
        return json.loads(resp.read())


def prefer_unless_tailwind(runways: list, preferred_id: str, max_tailwind_kt: int) -> str | None:
    """Use preferred_id unless its tailwind exceeds max_tailwind_kt."""
    return _post("/prefer-unless-tailwind", {
        "runways": runways,
        "preferred_id": preferred_id,
        "max_tailwind_kt": max_tailwind_kt,
    }).get("runway")


def prefer_unless_crosswind(runways: list, preferred_id: str, max_crosswind_kt: int) -> str | None:
    """Use preferred_id unless its crosswind exceeds max_crosswind_kt."""
    return _post("/prefer-unless-crosswind", {
        "runways": runways,
        "preferred_id": preferred_id,
        "max_crosswind_kt": max_crosswind_kt,
    }).get("runway")


def best_headwind(runways: list, advantage_threshold_kt: int = 2) -> str | None:
    """Runway with the greatest headwind (must beat runner-up by > threshold)."""
    return _post("/best-headwind", {
        "runways": runways,
        "advantage_threshold_kt": advantage_threshold_kt,
    }).get("runway")


# ---------------------------------------------------------------------------
# Per-airport selection logic — add cases for the airports you care about
# ---------------------------------------------------------------------------

def select_airport(airport: dict) -> dict:
    """
    Return an AirportSelectionResult.

    - Return {"icao": ..., "handled": False} to let es_runway_selector use its
      own generic wind/default logic.
    - Return {"icao": ..., "handled": True, "runway_uses": [...]} to own it.
    """
    icao = airport["icao"]
    runways = airport.get("runways", [])

    # Example: ENXX prefers runway 18 but switches to 36 when tailwind > 5 kt.
    if icao == "ENXX":
        rwy = prefer_unless_tailwind(runways, "18", max_tailwind_kt=5) or "18"
        return {"icao": icao, "handled": True,
                "runway_uses": [{"runway": rwy, "use": "Both"}]}

    # Example: ENYZ uses runway 27 unless crosswind > 15 kt, then tries 09.
    if icao == "ENYZ":
        rwy = prefer_unless_crosswind(runways, "27", max_crosswind_kt=15) or "27"
        return {"icao": icao, "handled": True,
                "runway_uses": [{"runway": rwy, "use": "Both"}]}

    return {"icao": icao, "handled": False}


# ---------------------------------------------------------------------------
# HTTP server
# ---------------------------------------------------------------------------

class _Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):  # silence default access log
        pass

    def do_GET(self):
        if self.path == "/health":
            self._ok(b"ok")
        else:
            self._send(404, b"not found")

    def do_POST(self):
        if self.path == "/runway-selections":
            length = int(self.headers.get("Content-Length", 0))
            body = json.loads(self.rfile.read(length))
            results = [select_airport(ap) for ap in body.get("airports", [])]
            self._ok(json.dumps({"results": results}).encode(), "application/json")
        else:
            self._send(404, b"not found")

    def _ok(self, body: bytes, ct: str = "text/plain"):
        self._send(200, body, ct)

    def _send(self, code: int, body: bytes, ct: str = "text/plain"):
        self.send_response(code)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    server = HTTPServer(("127.0.0.1", args.port), _Handler)
    print(f"plugin listening on 127.0.0.1:{args.port}", file=sys.stderr)
    server.serve_forever()
