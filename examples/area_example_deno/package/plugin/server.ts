/**
 * Minimal TypeScript (Deno) area plugin — no dependencies.
 *
 * Run via mise (the host does this automatically):
 *     mise exec deno -- deno run -A server.ts
 *
 * Environment provided by the host:
 *     RUNWAY_SELECTOR_PORT       — bind 127.0.0.1:$PORT
 *     RUNWAY_SELECTOR_AREA_DIR   — area directory (also cwd)
 *
 * The contract is plain HTTP/JSON (see openapi.json in the repository root):
 *     GET  /health             → 200 once ready
 *     POST /runway-selections  → batch request in, selections out
 *     POST /shutdown           → 200, then exit gracefully
 *
 * The host pre-computes per-runway wind components (headwind_kt, tailwind_kt,
 * crosswind_kt) and ships the parsed METAR plus a UTC timestamp. Use the
 * request's timestamp_utc for any time-of-day rules — never the wall clock.
 */

interface RunwayInfo {
  identifier: string;
  heading: number;
  headwind_kt?: number;
  tailwind_kt?: number;
  crosswind_kt?: number;
}

interface AirportRequest {
  icao: string;
  runways: RunwayInfo[];
}

interface SelectionsRequest {
  timestamp_utc: string;
  area_timezone: string;
  airports: AirportRequest[];
}

interface AirportResult {
  icao: string;
  handled: boolean;
  source?: "Metar" | "Default";
  runway_uses?: { runway: string; use: "Departing" | "Arriving" | "Both" }[];
}

/** Best headwind wins; no usable wind means handled=false so the host falls
 * back to area.toml's default_runways for this airport. */
function pick(airport: AirportRequest): AirportResult {
  const withWind = airport.runways.filter((r) => r.headwind_kt !== undefined);
  if (withWind.length === 0) {
    return { icao: airport.icao, handled: false };
  }
  const best = withWind.reduce((a, b) =>
    (b.headwind_kt ?? -Infinity) > (a.headwind_kt ?? -Infinity) ? b : a
  );
  return {
    icao: airport.icao,
    handled: true,
    source: "Metar",
    runway_uses: [{ runway: best.identifier, use: "Both" }],
  };
}

const port = Number(Deno.env.get("RUNWAY_SELECTOR_PORT"));
if (!Number.isInteger(port)) {
  console.error("RUNWAY_SELECTOR_PORT must be set to a port number");
  Deno.exit(1);
}

const server = Deno.serve(
  { hostname: "127.0.0.1", port },
  async (req: Request): Promise<Response> => {
    const path = new URL(req.url).pathname;

    if (req.method === "GET" && path === "/health") {
      return new Response(null, { status: 200 });
    }

    if (req.method === "POST" && path === "/runway-selections") {
      try {
        const body = (await req.json()) as SelectionsRequest;
        const results = body.airports.map(pick);
        return Response.json({ results });
      } catch (e) {
        // Never crash on bad input — report the error instead.
        return Response.json({ error: String(e) }, { status: 400 });
      }
    }

    if (req.method === "POST" && path === "/shutdown") {
      // Respond first, then stop accepting connections and exit.
      queueMicrotask(() => server.shutdown());
      return new Response(null, { status: 200 });
    }

    return new Response(null, { status: 404 });
  },
);

console.error(`example-deno listening on 127.0.0.1:${port}`);
await server.finished;
console.error("example-deno shutting down");
