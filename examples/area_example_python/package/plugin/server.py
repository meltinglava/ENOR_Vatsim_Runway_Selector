"""Minimal Python area plugin.

Run via mise (the host does this automatically):
    mise exec python -- python server.py

Environment provided by the host:
    RUNWAY_SELECTOR_PORT       — bind 127.0.0.1:$PORT
    RUNWAY_SELECTOR_AREA_DIR   — area directory (also cwd)

Before packaging, run scripts/generate-stubs.sh once. That populates
runway_selector_pb2.py and runway_selector_pb2_grpc.py next to this
file. They're committed in this example so you can read them.
"""

from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path

# Generated stubs live next to this file.
sys.path.insert(0, str(Path(__file__).parent))

import grpc
from grpc_health.v1 import health, health_pb2, health_pb2_grpc

import runway_selector_pb2 as pb        # type: ignore[import-not-found]
import runway_selector_pb2_grpc as rpc  # type: ignore[import-not-found]


ICAOS = ["ZZZA", "ZZZB"]


class ExampleArea(rpc.RunwaySelectorServicer):
    async def GetAirports(self, request, context):
        return pb.GetAirportsResponse(icaos=ICAOS)

    async def SelectRunways(self, request, context):
        selections = [s for a in request.airports if (s := pick(a)) is not None]
        return pb.SelectRunwaysResponse(selections=selections)


def pick(airport):
    if airport.atis_runways:
        return pb.AirportSelection(
            icao=airport.icao,
            source=pb.SELECTION_SOURCE_ATIS,
            runways=list(airport.atis_runways),
        )

    with_wind = [r for r in airport.runways if r.HasField("wind_components")]
    if not with_wind:
        return None

    best = max(with_wind, key=lambda r: r.wind_components.headwind_kt)
    return pb.AirportSelection(
        icao=airport.icao,
        source=pb.SELECTION_SOURCE_METAR,
        runways=[
            pb.RunwayAssignment(
                identifier=best.identifier,
                use=pb.RUNWAY_USE_BOTH,
            ),
        ],
    )


async def serve():
    port = int(os.environ["RUNWAY_SELECTOR_PORT"])
    server = grpc.aio.server()

    rpc.add_RunwaySelectorServicer_to_server(ExampleArea(), server)

    h = health.HealthServicer()
    health_pb2_grpc.add_HealthServicer_to_server(h, server)
    # HealthServicer.set is synchronous (it just updates internal state); the
    # aio server still serves Health/Check correctly.
    h.set("", health_pb2.HealthCheckResponse.SERVING)
    h.set(
        "runway_selector.v1.RunwaySelector",
        health_pb2.HealthCheckResponse.SERVING,
    )

    server.add_insecure_port(f"127.0.0.1:{port}")
    await server.start()
    print(f"example-python listening on 127.0.0.1:{port}", file=sys.stderr)
    await server.wait_for_termination()


if __name__ == "__main__":
    asyncio.run(serve())
