"""Minimal Python area plugin — standard library only, no pip installs.

Run via mise (the host does this automatically):
    mise exec python -- python server.py

Environment provided by the host:
    RUNWAY_SELECTOR_PORT       — bind 127.0.0.1:$PORT
    RUNWAY_SELECTOR_AREA_DIR   — area directory (also cwd)

The contract is plain HTTP/JSON (see openapi.json in the repository root):
    GET  /health             → 200 once ready
    POST /runway-selections  → batch request in, selections out
    POST /shutdown           → 200, then exit gracefully

The host pre-computes per-runway wind components (headwind_kt, tailwind_kt,
crosswind_kt) and ships the parsed METAR plus a UTC timestamp. Use the
request's timestamp_utc for any time-of-day rules — never the wall clock.

Try it by hand:
    RUNWAY_SELECTOR_PORT=8080 python server.py &
    curl localhost:8080/health
    curl -X POST localhost:8080/runway-selections -d '{
        "timestamp_utc": "2026-05-14T10:20:00Z",
        "area_timezone": "Etc/UTC",
        "airports": [{"icao": "ZZZA", "runways": [
            {"identifier": "18", "heading": 180, "headwind_kt": 8},
            {"identifier": "36", "heading": 360, "headwind_kt": -8}
        ]}]
    }'
"""

from __future__ import annotations

import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer


def pick(airport: dict) -> dict:
    """Best headwind wins; no usable wind means handled=false so the host
    falls back to area.toml's default_runways for this airport."""
    with_wind = [r for r in airport.get("runways", []) if r.get("headwind_kt") is not None]
    if not with_wind:
        return {"icao": airport["icao"], "handled": False}

    best = max(with_wind, key=lambda r: r["headwind_kt"])
    return {
        "icao": airport["icao"],
        "handled": True,
        "source": "Metar",
        "runway_uses": [{"runway": best["identifier"], "use": "Both"}],
    }


class Handler(BaseHTTPRequestHandler):
    def _reply(self, status: int, body: dict | None = None) -> None:
        payload = json.dumps(body or {}).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
        if self.path == "/health":
            self._reply(200)
        else:
            self._reply(404)

    def do_POST(self) -> None:  # noqa: N802 (stdlib naming)
        if self.path == "/runway-selections":
            length = int(self.headers.get("Content-Length", "0"))
            try:
                request = json.loads(self.rfile.read(length))
                results = [pick(a) for a in request.get("airports", [])]
            except (json.JSONDecodeError, KeyError, TypeError) as e:
                # Never crash on bad input — report the error instead.
                self._reply(400, {"error": str(e)})
                return
            self._reply(200, {"results": results})
        elif self.path == "/shutdown":
            self._reply(200)
            # shutdown() blocks until the server loop exits, so call it from
            # another thread after this handler returns.
            threading.Thread(target=self.server.shutdown, daemon=True).start()
        else:
            self._reply(404)

    def log_message(self, format: str, *args) -> None:  # noqa: A002 (stdlib signature)
        print(f"example-python: {format % args}", file=sys.stderr)


def main() -> None:
    port = int(os.environ["RUNWAY_SELECTOR_PORT"])
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"example-python listening on 127.0.0.1:{port}", file=sys.stderr)
    server.serve_forever()
    print("example-python shutting down", file=sys.stderr)


if __name__ == "__main__":
    main()
