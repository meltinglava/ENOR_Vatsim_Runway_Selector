/**
 * TypeScript runway-selector plugin for Deno.
 *
 * es_runway_selector runs this automatically via mise:
 *   deno run --allow-net --allow-read plugin.ts --port N --helpers-port M
 *
 * Configure in config.toml:
 *   plugin_binary = "/path/to/examples/typescript/plugin.ts"
 *
 * Regenerate types after an openapi.yaml change:
 *   mise run generate
 */

import createClient from "openapi-fetch";
import type { components, paths } from "./generated/api.ts";

type Airport = components["schemas"]["AirportSelectionRequest"];
type AirportResult = components["schemas"]["AirportSelectionResult"];
type Request = components["schemas"]["RunwaySelectionsRequest"];
type Response = components["schemas"]["RunwaySelectionsResponse"];

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const args = Deno.args;
const getFlag = (flag: string): number => {
  const i = args.indexOf(flag);
  if (i === -1) throw new Error(`Missing ${flag}`);
  return parseInt(args[i + 1]);
};
const port = getFlag("--port");
const helpersPort = getFlag("--helpers-port");

// ---------------------------------------------------------------------------
// Typed helpers client — generated types enforce correct request/response shapes
// ---------------------------------------------------------------------------

const helpers = createClient<paths>({
  baseUrl: `http://127.0.0.1:${helpersPort}`,
});

// ---------------------------------------------------------------------------
// Per-airport selection logic
// ---------------------------------------------------------------------------

async function selectAirport(airport: Airport): Promise<AirportResult> {
  const { icao, runways = [] } = airport;

  // Example: ENXX prefers runway 18 but switches to 36 when tailwind > 5 kt.
  if (icao === "ENXX") {
    const { data } = await helpers.POST("/helpers/prefer-unless-tailwind", {
      body: { runways, preferred_id: "18", max_tailwind_kt: 5 },
    });
    const rwy = data?.runway ?? "18";
    return { icao, handled: true, runway_uses: [{ runway: rwy, use: "Both" }] };
  }

  // Example: ENYZ uses runway 27 unless crosswind > 15 kt, then tries best headwind.
  if (icao === "ENYZ") {
    const { data } = await helpers.POST("/helpers/prefer-unless-crosswind", {
      body: { runways, preferred_id: "27", max_crosswind_kt: 15 },
    });
    const rwy = data?.runway ?? "27";
    return { icao, handled: true, runway_uses: [{ runway: rwy, use: "Both" }] };
  }

  return { icao, handled: false };
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

Deno.serve({ port, hostname: "127.0.0.1" }, async (req) => {
  const { pathname } = new URL(req.url);

  if (req.method === "GET" && pathname === "/health") {
    return new Response("ok");
  }

  if (req.method === "POST" && pathname === "/runway-selections") {
    try {
      const { airports = [] }: Request = await req.json();
      const results = await Promise.all(airports.map(selectAirport));
      return Response.json({ results } satisfies Response);
    } catch (err) {
      return new Response(String(err), { status: 500 });
    }
  }

  return new Response("not found", { status: 404 });
});

console.error(`plugin listening on 127.0.0.1:${port}`);
