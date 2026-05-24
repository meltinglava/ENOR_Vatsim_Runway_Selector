/**
 * Example es_runway_selector plugin skeleton — TypeScript / Node.js.
 *
 * The parent process sets these environment variables before starting the plugin:
 *   ES_RUNWAY_SELECTOR_PLUGIN_PORT  – port this server must listen on
 *   ES_RUNWAY_SELECTOR_PORT         – port of the parent's helper API
 *
 * One-time setup (mise manages Node 22; packages go into node_modules/, not global):
 *   mise run install    # npm install
 *   mise run generate   # writes src/generated/schema.ts from ../../openapi.json
 *
 * Running (normally done by the parent, but you can test manually):
 *   ES_RUNWAY_SELECTOR_PLUGIN_PORT=8100 ES_RUNWAY_SELECTOR_PORT=8200 mise run dev
 */

import express, { Request, Response } from "express";
import type { components } from "./generated/schema";

// ─── Types from the generated schema ─────────────────────────────────────────

type AtisRequest = components["schemas"]["AtisRequest"];
type AtisResponse = components["schemas"]["AtisResponse"];
type AirportRunwayAssignment = components["schemas"]["AirportRunwayAssignment"];
type RunwaySelectionRequest = components["schemas"]["RunwaySelectionRequest"];
type RunwaySelectionResponse = components["schemas"]["RunwaySelectionResponse"];
type ParseAtisRequest = components["schemas"]["ParseAtisRequest"];
type ParseAtisResponse = components["schemas"]["ParseAtisResponse"];

// ─── Configuration ────────────────────────────────────────────────────────────

/** ICAO codes this plugin handles. Replace with your own airports. */
const HANDLED_AIRPORTS: string[] = ["EGLL", "EGKK"];

const pluginPort = parseInt(process.env.ES_RUNWAY_SELECTOR_PLUGIN_PORT ?? "0");
const parentPort = parseInt(process.env.ES_RUNWAY_SELECTOR_PORT ?? "0");
const parentUrl = `http://127.0.0.1:${parentPort}`;

// ─── Server ───────────────────────────────────────────────────────────────────

const app = express();
app.use(express.json());

/** Readiness probe — return 200 as soon as the server is up. */
app.get("/health", (_req: Request, res: Response) => {
  res.send("ok");
});

/** List ICAO codes this plugin handles. */
app.get("/airports", (_req: Request, res: Response) => {
  res.json({ airports: HANDLED_AIRPORTS });
});

/**
 * POST /atis
 *
 * Receive ATIS texts for the plugin's airports and return runway assignments.
 * This skeleton delegates straight to the parent's /parse-atis helper.
 * Override the logic per-airport for custom ATIS parsing.
 */
app.post("/atis", async (req: Request, res: Response) => {
  const request: AtisRequest = req.body;
  const airports: AirportRunwayAssignment[] = [];

  for (const entry of request.atis_entries ?? []) {
    if (!HANDLED_AIRPORTS.includes(entry.airport_icao)) continue;

    try {
      const resp = await fetch(`${parentUrl}/parse-atis`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ atis_text: entry.atis_text } satisfies ParseAtisRequest),
      });
      if (resp.ok) {
        const parsed = (await resp.json()) as ParseAtisResponse;
        airports.push({
          airport_icao: entry.airport_icao,
          assignments: parsed.assignments,
        });
      }
    } catch (err) {
      console.error(`/parse-atis call failed for ${entry.airport_icao}:`, err);
    }
  }

  res.json({ airports } satisfies AtisResponse);
});

/**
 * POST /runways
 *
 * Receive airport info + METAR and return which runways are active.
 * Implement your selection logic per ICAO below.
 */
app.post("/runways", (req: Request, res: Response) => {
  const request: RunwaySelectionRequest = req.body;
  const { airport, metar } = request;

  let runways: RunwaySelectionResponse["runways"] = [];

  switch (airport.icao) {
    case "EGLL":
      // TODO: implement EGLL runway selection using `airport` and `metar`
      break;
    case "EGKK":
      // TODO: implement EGKK runway selection
      break;
    default:
      console.warn(`Unexpected airport: ${airport.icao}`);
  }

  res.json({ runways } satisfies RunwaySelectionResponse);
});

// ─── Startup ──────────────────────────────────────────────────────────────────

if (!pluginPort) {
  console.error("ES_RUNWAY_SELECTOR_PLUGIN_PORT is not set");
  process.exit(1);
}

app.listen(pluginPort, "127.0.0.1", () => {
  console.log(`Plugin listening on 127.0.0.1:${pluginPort} (parent: ${parentPort})`);
});
