#!/usr/bin/env -S uv run
"""
Python runway-selector plugin using generated Pydantic models.

es_runway_selector runs this automatically via mise (uv run):
  uv run plugin.py --port N --helpers-port M

Configure in config.toml:
  plugin_binary = "/path/to/examples/python/plugin.py"

Regenerate models after an openapi.yaml change:
  mise run generate
"""

import argparse
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

# Make the generated package importable when run as a script.
sys.path.insert(0, str(Path(__file__).parent))

import httpx
from generated.models import (
    AirportSelectionRequest,
    AirportSelectionResult,
    RunwaySelectionsRequest,
    RunwaySelectionsResponse,
    RunwayUse,
    RunwayUseEntry,
)

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--helpers-port", type=int, required=True, dest="helpers_port")
args = parser.parse_args()

# ---------------------------------------------------------------------------
# Typed helpers client
# ---------------------------------------------------------------------------

_helpers = httpx.Client(
    base_url=f"http://127.0.0.1:{args.helpers_port}/helpers",
    timeout=5,
)


def _call_helper(path: str, body: dict) -> dict:
    resp = _helpers.post(path, json=body)
    resp.raise_for_status()
    return resp.json()


def prefer_unless_tailwind(
    request: AirportSelectionRequest, preferred_id: str, max_tailwind_kt: int
) -> str | None:
    runways = [r.model_dump() for r in request.runways]
    return _call_helper(
        "/prefer-unless-tailwind",
        {"runways": runways, "preferred_id": preferred_id, "max_tailwind_kt": max_tailwind_kt},
    )["runway"]


def prefer_unless_crosswind(
    request: AirportSelectionRequest, preferred_id: str, max_crosswind_kt: int
) -> str | None:
    runways = [r.model_dump() for r in request.runways]
    return _call_helper(
        "/prefer-unless-crosswind",
        {"runways": runways, "preferred_id": preferred_id, "max_crosswind_kt": max_crosswind_kt},
    )["runway"]


def best_headwind(
    request: AirportSelectionRequest, advantage_threshold_kt: int = 2
) -> str | None:
    runways = [r.model_dump() for r in request.runways]
    return _call_helper(
        "/best-headwind",
        {"runways": runways, "advantage_threshold_kt": advantage_threshold_kt},
    )["runway"]


# ---------------------------------------------------------------------------
# Per-airport selection logic
# ---------------------------------------------------------------------------


def select_airport(airport: AirportSelectionRequest) -> AirportSelectionResult:
    """
    Return an AirportSelectionResult.

    - Return handled=False to let es_runway_selector use its own logic.
    - Return handled=True with runway_uses to take ownership of this airport.
    """
    icao = airport.icao

    # Example: ENXX prefers runway 18 but switches to 36 when tailwind > 5 kt.
    if icao == "ENXX":
        rwy = prefer_unless_tailwind(airport, "18", max_tailwind_kt=5) or "18"
        return AirportSelectionResult(
            icao=icao,
            handled=True,
            runway_uses=[RunwayUseEntry(runway=rwy, use=RunwayUse.Both)],
        )

    # Example: ENYZ uses runway 27 unless crosswind > 15 kt, then tries best headwind.
    if icao == "ENYZ":
        rwy = prefer_unless_crosswind(airport, "27", max_crosswind_kt=15) or "27"
        return AirportSelectionResult(
            icao=icao,
            handled=True,
            runway_uses=[RunwayUseEntry(runway=rwy, use=RunwayUse.Both)],
        )

    return AirportSelectionResult(icao=icao, handled=False)


# ---------------------------------------------------------------------------
# HTTP server
# ---------------------------------------------------------------------------


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):  # silence default access log
        pass

    def do_GET(self):
        if self.path == "/health":
            self._respond(200, b"ok")
        else:
            self._respond(404, b"not found")

    def do_POST(self):
        if self.path == "/runway-selections":
            length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(length)
            try:
                req = RunwaySelectionsRequest.model_validate_json(body)
                results = [select_airport(a) for a in req.airports]
                resp = RunwaySelectionsResponse(results=results)
                payload = resp.model_dump_json(
                    exclude_none=True,
                    by_alias=True,
                ).encode()
                self._respond(200, payload, "application/json")
            except Exception as exc:
                self._respond(500, str(exc).encode())
        else:
            self._respond(404, b"not found")

    def _respond(self, code: int, body: bytes, ct: str = "text/plain"):
        self.send_response(code)
        self.send_header("Content-Type", ct)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    print(f"plugin listening on 127.0.0.1:{args.port}", file=sys.stderr)
    HTTPServer(("127.0.0.1", args.port), _Handler).serve_forever()
