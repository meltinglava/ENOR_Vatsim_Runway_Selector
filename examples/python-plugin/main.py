"""
Example es_runway_selector plugin skeleton — Python / FastAPI.

The parent process sets these environment variables before starting the plugin:
  ES_RUNWAY_SELECTOR_PLUGIN_PORT  – port this server must listen on
  ES_RUNWAY_SELECTOR_PORT         – port of the parent's helper API

One-time setup (mise manages Python 3.12; packages go into .venv/, not global):
  mise run install    # pip install into .venv/
  mise run generate   # writes generated/models.py from ../../openapi.json

Running (normally done by the parent, but you can test manually):
  ES_RUNWAY_SELECTOR_PLUGIN_PORT=8100 ES_RUNWAY_SELECTOR_PORT=8200 mise run dev
"""

import os
import logging
from contextlib import asynccontextmanager
from typing import AsyncGenerator

import httpx
from fastapi import FastAPI
from fastapi.responses import PlainTextResponse

from generated.models import (
    AtisRequest,
    AtisResponse,
    AirportRunwayAssignment,
    ParseAtisRequest,
    ParseAtisResponse,
    RunwaySelectionRequest,
    RunwaySelectionResponse,
)

# ─── Configuration ────────────────────────────────────────────────────────────

HANDLED_AIRPORTS: list[str] = ["EGLL", "EGKK"]  # replace with your airports

plugin_port = int(os.environ["ES_RUNWAY_SELECTOR_PLUGIN_PORT"])
parent_port = int(os.environ["ES_RUNWAY_SELECTOR_PORT"])
parent_url = f"http://127.0.0.1:{parent_port}"

logger = logging.getLogger(__name__)

# ─── HTTP client (shared across requests) ────────────────────────────────────

http_client: httpx.AsyncClient


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncGenerator[None, None]:
    global http_client
    http_client = httpx.AsyncClient(base_url=parent_url, timeout=10.0)
    yield
    await http_client.aclose()


# ─── App ─────────────────────────────────────────────────────────────────────

app = FastAPI(lifespan=lifespan)


@app.get("/health")
def health() -> PlainTextResponse:
    return PlainTextResponse("ok")


@app.get("/airports")
def airports() -> dict[str, list[str]]:
    return {"airports": HANDLED_AIRPORTS}


@app.post("/atis", response_model=AtisResponse)
async def atis(request: AtisRequest) -> AtisResponse:
    """
    Receive ATIS texts for the plugin's airports and return runway assignments.
    This skeleton delegates straight to the parent's /parse-atis helper.
    Override the logic per-airport for custom ATIS parsing.
    """
    result: list[AirportRunwayAssignment] = []

    for entry in request.atis_entries or []:
        if entry.airport_icao not in HANDLED_AIRPORTS:
            continue
        try:
            resp = await http_client.post(
                "/parse-atis",
                content=ParseAtisRequest(atis_text=entry.atis_text).model_dump_json(),
                headers={"Content-Type": "application/json"},
            )
            resp.raise_for_status()
            parsed = ParseAtisResponse.model_validate(resp.json())
            result.append(
                AirportRunwayAssignment(
                    airport_icao=entry.airport_icao,
                    assignments=parsed.assignments,
                )
            )
        except Exception as exc:
            logger.warning("/parse-atis failed for %s: %s", entry.airport_icao, exc)

    return AtisResponse(airports=result)


@app.post("/runways", response_model=RunwaySelectionResponse)
async def runways(request: RunwaySelectionRequest) -> RunwaySelectionResponse:
    """
    Receive airport info + METAR and return which runways are active.
    Implement your selection logic per ICAO below.
    """
    icao = request.airport.icao

    if icao == "EGLL":
        pass  # TODO: implement EGLL runway selection using request.airport and request.metar
    elif icao == "EGKK":
        pass  # TODO: implement EGKK runway selection
    else:
        logger.warning("Unexpected airport: %s", icao)

    return RunwaySelectionResponse(runways=[])


# ─── Entry point ─────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import uvicorn

    logging.basicConfig(level=logging.INFO)
    uvicorn.run("main:app", host="127.0.0.1", port=plugin_port, log_level="info")
