#!/usr/bin/env node
/**
 * Example runway-selector plugin written in Node.js (built-ins only).
 *
 * es_runway_selector runs this via mise when plugin_binary ends in .js:
 *   plugin_binary = "/path/to/node_plugin_example.js"
 *
 * mise installs Node.js automatically if it is not already present.
 *
 * Receives: --port N  --helpers-port M
 */

"use strict";

const http = require("http");

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
const argv = process.argv.slice(2);
const get = (flag) => {
  const i = argv.indexOf(flag);
  return i !== -1 ? parseInt(argv[i + 1], 10) : null;
};
const PORT = get("--port");
const HELPERS_PORT = get("--helpers-port");
if (!PORT || !HELPERS_PORT) {
  process.stderr.write("Usage: node plugin.js --port N --helpers-port M\n");
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Helpers — call back to the es_runway_selector helpers server
// ---------------------------------------------------------------------------

function postJson(path, payload) {
  return new Promise((resolve, reject) => {
    const body = JSON.stringify(payload);
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port: HELPERS_PORT,
        path: `/helpers${path}`,
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Content-Length": Buffer.byteLength(body),
        },
      },
      (res) => {
        let data = "";
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => resolve(JSON.parse(data)));
      }
    );
    req.on("error", reject);
    req.write(body);
    req.end();
  });
}

const helpers = {
  preferUnlessTailwind: (runways, preferredId, maxTailwindKt) =>
    postJson("/prefer-unless-tailwind", {
      runways,
      preferred_id: preferredId,
      max_tailwind_kt: maxTailwindKt,
    }).then((r) => r.runway),

  preferUnlessCrosswind: (runways, preferredId, maxCrosswindKt) =>
    postJson("/prefer-unless-crosswind", {
      runways,
      preferred_id: preferredId,
      max_crosswind_kt: maxCrosswindKt,
    }).then((r) => r.runway),

  bestHeadwind: (runways, advantageThresholdKt = 2) =>
    postJson("/best-headwind", {
      runways,
      advantage_threshold_kt: advantageThresholdKt,
    }).then((r) => r.runway),
};

// ---------------------------------------------------------------------------
// Per-airport selection logic
// ---------------------------------------------------------------------------

async function selectAirport(airport) {
  const { icao, runways = [] } = airport;

  // Example: ENXX prefers 18 unless tailwind > 5 kt.
  if (icao === "ENXX") {
    const rwy = (await helpers.preferUnlessTailwind(runways, "18", 5)) ?? "18";
    return { icao, handled: true, runway_uses: [{ runway: rwy, use: "Both" }] };
  }

  // Example: ENYZ prefers 27 unless crosswind > 15 kt.
  if (icao === "ENYZ") {
    const rwy =
      (await helpers.preferUnlessCrosswind(runways, "27", 15)) ?? "27";
    return { icao, handled: true, runway_uses: [{ runway: rwy, use: "Both" }] };
  }

  return { icao, handled: false };
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

const server = http.createServer(async (req, res) => {
  if (req.method === "GET" && req.url === "/health") {
    res.writeHead(200).end("ok");
    return;
  }

  if (req.method === "POST" && req.url === "/runway-selections") {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", async () => {
      try {
        const { airports = [] } = JSON.parse(body);
        const results = await Promise.all(airports.map(selectAirport));
        const payload = JSON.stringify({ results });
        res
          .writeHead(200, {
            "Content-Type": "application/json",
            "Content-Length": Buffer.byteLength(payload),
          })
          .end(payload);
      } catch (err) {
        res.writeHead(500).end(String(err));
      }
    });
    return;
  }

  res.writeHead(404).end("not found");
});

server.listen(PORT, "127.0.0.1", () => {
  process.stderr.write(`plugin listening on 127.0.0.1:${PORT}\n`);
});
