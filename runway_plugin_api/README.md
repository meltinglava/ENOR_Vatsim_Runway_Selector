# Writing an area plugin

An *area* is a per-FIR plugin that picks runways. The host
(`es_runway_selector`) spawns it as a subprocess, talks to it over
plain HTTP/JSON on localhost, and applies the result to the `.rwy`
file. Any language that can serve HTTP works — a plugin is
`curl`-testable.

This guide walks you through writing one from scratch.

> **Pre-1.0.** The contract lives in this crate (`runway_plugin_api`)
> and is mirrored in the committed [`openapi.json`](../openapi.json),
> generated code-first from the Rust types:
> `cargo run -p runway_plugin_api --features openapi --bin generate_openapi`.
> The surface is still evolving.

---

## 1. Scaffold the package

An area is just a directory:

```text
my-area/
    manifest.toml
    area.toml
    plugin/area_my_area        # binary or script the host spawns
    profiles/twr.toml          # at least one profile
```

Anything ending in `.local.toml` belongs to the end user — don't ship
those (see [.local.toml overlay](#localtoml-overlay)).

## 2. Write `manifest.toml`

The package's identity. Never overridden by users.

```toml
name            = "my-area"
version         = "0.1.0"
display_name    = "My Area"
runtime         = "rust"           # rust | python | node | deno
entry           = "area_my_area"   # path relative to plugin/
supported_icaos = ["EXAM", "EXBM"] # the airports you own — authoritative
```

`supported_icaos` is the **single source of truth** for which airports
your plugin decides. The host only sends you airports from this list
(that also exist in the open sector file), and when two installed areas
claim the same ICAO, the first installed one wins.

Optional: `description`, `min_core_version` (hosts older than this
refuse to spawn you).

## 3. Write `area.toml`

Runtime defaults the host reads directly:

```toml
metar_urls         = ["https://metar.vatsim.net/EN"]
time_zone          = "Europe/Oslo"     # IANA — sent to you as area_timezone
sector_file_prefix = "ENOR"            # matches ENOR-*.sct
ignore_airports    = ["ENQR"]          # ICAOs whose METARs to drop

[default_runways]
ENGM = 1                                # heading-in-tens-of-degrees fallback
```

All fields are optional.

## 4. Implement the plugin server

The host runs your `plugin/<entry>` with these env vars set:

- `RUNWAY_SELECTOR_PORT` — serve HTTP on `127.0.0.1:$PORT`.
- `RUNWAY_SELECTOR_AREA_DIR` — your installed area directory (also the cwd).

Implement three endpoints (full schemas in
[`openapi.json`](../openapi.json)):

- `GET /health` — return `200` once you're ready. The host polls this
  after spawning you.
- `POST /runway-selections` — one batch request per run. The body has a
  `timestamp_utc` (RFC 3339), an `area_timezone`, and one entry per
  airport with the raw + parsed METAR and **pre-computed per-runway wind
  components** (`headwind_kt`, `tailwind_kt`, `crosswind_kt`,
  `crosswind_direction`). Use them — your math will line up with the
  runway report. Never read the wall clock; use `timestamp_utc` so your
  selections are reproducible.
- `POST /shutdown` — return `200`, then exit. This is how the host
  stops you gracefully on Windows.

For each airport, answer either

```json
{ "icao": "EXAM", "handled": true, "source": "Metar",
  "runway_uses": [{ "runway": "18", "use": "Both" }],
  "tags": [] }
```

or `{ "icao": "EXAM", "handled": false }` to make the host fall back to
its built-in logic (`area.toml`'s `default_runways`) *for that airport
only*.

Airports the controller has already decided via ATIS never reach you —
the host applies ATIS itself.

`tags` is optional and machine-readable: each entry
(`{id, conflict, symbol, label}`) explains *why* you picked a runway
(`conflict: false`) or flags a negative factor you accepted
(`conflict: true`, e.g. tailwind during LVP). The host renders them in
the HTML runway report.

### Rust skeleton

Rust plugins get the wire types plus tested selection helpers
(`best_headwind`, `prefer_unless_tailwind`, `prefer_unless_crosswind`,
`min_crosswind`, `within_crosswind_limit`) from this crate:

```rust,ignore
use axum::{Json, Router, http::StatusCode, routing::{get, post}};
use runway_plugin_api::{
    AirportSelectionRequest, AirportSelectionResult, RunwaySelectionsRequest,
    RunwaySelectionsResponse, RunwayUse, RunwayUseEntry, SelectionSource,
    helpers::best_headwind,
};

fn pick(a: &AirportSelectionRequest) -> AirportSelectionResult {
    match best_headwind(&a.runways, 0) {
        Some(best) => AirportSelectionResult {
            icao: a.icao.clone(),
            handled: true,
            source: SelectionSource::Metar,
            runway_uses: vec![RunwayUseEntry {
                runway: best.identifier.clone(),
                use_: RunwayUse::Both,
            }],
            tags: vec![],
        },
        None => AirportSelectionResult {
            icao: a.icao.clone(),
            handled: false,
            source: SelectionSource::Metar,
            runway_uses: vec![],
            tags: vec![],
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = std::env::var("RUNWAY_SELECTOR_PORT")?.parse()?;
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/runway-selections", post(
            |Json(req): Json<RunwaySelectionsRequest>| async move {
                Json(RunwaySelectionsResponse {
                    results: req.airports.iter().map(pick).collect(),
                })
            },
        ))
        .route("/shutdown", post(|| async { std::process::exit(0) }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

Cargo deps: `runway_plugin_api`, `axum`, `tokio`
(`macros + rt-multi-thread + net`). See
[`examples/area_example_rust`](../examples/area_example_rust) for a
complete plugin with graceful shutdown, and
[`area_enor/src/selector.rs`](../area_enor/src/selector.rs) for a
worked example covering mixed/segregated/single ops by local time and a
crosswind-driven runway switch.

### Python / Node / Deno

The host runs non-Rust entries through
[`mise`](https://mise.jdx.dev/), so end users don't need the runtime
installed. Ship a `mise.toml` pinning your runtime next to
`manifest.toml`. The Python example
([`examples/area_example_python`](../examples/area_example_python))
uses only the standard library; the Deno example
([`examples/area_example_deno`](../examples/area_example_deno)) is a
single dependency-free TypeScript file. Typed clients for any language
can be generated from [`openapi.json`](../openapi.json) (e.g.
`datamodel-codegen` for Python, `openapi-typescript` for TS).

## 5. Add at least one profile

A profile is a controller position. The host needs one to know what to
launch:

```toml
# profiles/twr.toml
name         = "twr"
display_name = "Tower / GND"
prf_files    = ["my_area_twr.prf"]              # opened in the user's sector folder
default_apps = ["EuroScope", "TrackAudio"]      # names from the user's app_launchers.toml
```

## 6. Test locally

Talk to your plugin by hand before involving the host:

```bash
RUNWAY_SELECTOR_PORT=8080 RUNWAY_SELECTOR_AREA_DIR=$PWD plugin/area_my_area &
curl localhost:8080/health
curl -X POST localhost:8080/runway-selections -d '{
    "timestamp_utc": "2026-05-14T10:20:00Z",
    "area_timezone": "Etc/UTC",
    "airports": [{"icao": "EXAM", "runways": [
        {"identifier": "18", "heading": 180, "headwind_kt": 8},
        {"identifier": "36", "heading": 360, "headwind_kt": -8}
    ]}]
}'
```

Then symlink (or copy) your in-progress package into the host's
install dir:

```bash
ln -s "$PWD/my-area" "$HOME/.local/share/es_runway_selector/areas/my-area"

es_runway_selector area list                     # should list "my-area"
es_runway_selector area profile show my-area twr # sanity-check profile parsing
es_runway_selector                                # full run
```

The host kills the subprocess after each cycle, so iterate by rebuilding
and re-running.

## 7. Publish

Tar the **contents** of your package directory — no top-level
`my-area/` wrapper, the host creates that on install:

```bash
cd /path/to/my-area
tar -czf my-area-0.1.0.tar.gz .
sha256sum my-area-0.1.0.tar.gz
```

For Rust plugins, ship one tarball per `(name, version, target)` — the
binary at `plugin/<entry>` must match the user's OS/arch.

Then either:

- **Primary registry** — open a PR against
  [`areas.json`](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/blob/main/areas.json).
  Once merged, anyone can `area install <name>`.
- **Self-hosted** — host your own `areas.json` and tell users to add it
  to their `config.local.toml`:

  ```toml
  extra_registries = ["https://example.org/my-areas.json"]
  ```

Registry entry shape:

```json
{
  "schema_version": 1,
  "areas": [{
    "name": "my-area",
    "display_name": "My Area",
    "description": "Runway selection for the XYZ FIR",
    "version": "0.1.0",
    "download_url": "https://example.org/my-area-0.1.0.tar.gz",
    "checksum_sha256": "<hex digest from sha256sum>",
    "maintainers": ["yourhandle"]
  }]
}
```

---

## Reference

### `POST /runway-selections` request

```jsonc
{
  "timestamp_utc": "2026-05-14T10:20:00Z",  // use this, not the local clock — keeps runs reproducible
  "area_timezone": "Europe/Oslo",           // IANA, from your area.toml
  "airports": [{
    "icao": "ENGM",
    "runways": [{
      "identifier": "01L",                  // "01L", "19R", "18"
      "heading": 7,                         // degrees true
      "headwind_kt": 8,                     // pre-computed by the host; negative = tailwind
      "tailwind_kt": 0,
      "crosswind_kt": 3,
      "crosswind_direction": "Left"         // Left | Right | Variable
    }],
    "metar": {                              // absent if no METAR
      "raw": "ENGM 111150Z ...",
      "parsed": { "is_cavok": false, "wind": { /* … */ }, "clouds": [ /* … */ ] }
    }
  }]
}
```

Wind components are `null`/absent when there is no usable METAR wind.
Full schema: [`openapi.json`](../openapi.json).

### `POST /runway-selections` response

```jsonc
{
  "results": [{
    "icao": "ENGM",
    "handled": true,                        // false = host falls back for this airport
    "source": "Metar",                      // Metar | Default (ATIS is host-side)
    "runway_uses": [
      { "runway": "01L", "use": "Departing" },   // Departing | Arriving | Both
      { "runway": "01R", "use": "Arriving" }
    ],
    "tags": [{ "id": "lvp", "conflict": false, "symbol": "🌫",
               "label": "Low Visibility Procedures active" }]
  }]
}
```

Source attribution matters — the `.rwy` writer prefers
`ATIS > METAR > DEFAULT` and takes the first one present:

| You picked from…                        | Set `source` to | Notes                                             |
| --------------------------------------- | --------------- | ------------------------------------------------- |
| Wind / METAR                            | `Metar`         |                                                   |
| `default_runways` in `area.toml`        | `Default`       | e.g. calm-wind runway at night                    |
| Nothing — wind ambiguous, no METAR      | —               | Answer `handled: false`. The host falls back.     |
| Parallel runways, mixed ops             | usually `Metar` | Two entries, both `use = "Both"`.                 |
| Parallel runways, segregated ops        | usually `Metar` | Two entries: one `Departing`, one `Arriving`.     |

(ATIS never appears: the host applies controller-announced runways
itself and does not send you those airports.)

### `.local.toml` overlay

For any `foo.toml` you ship, an end user may write a sibling
`foo.local.toml`. The host overlays the two before parsing — tables
merge key-by-key, scalars and arrays are replaced wholesale. **Never
ship a `.local.toml` yourself**; it would clobber the user's overrides.

### Non-Rust runtimes

Pin the runtime version at your area's root so the host can install
it on demand:

```toml
# mise.toml
[tools]
python = "3.12"
```

Then serve the three endpoints with whatever HTTP server your language
ships — see
[`examples/area_example_python/package/plugin/server.py`](../examples/area_example_python/package/plugin/server.py)
(standard library only) and
[`examples/area_example_deno/package/plugin/server.ts`](../examples/area_example_deno/package/plugin/server.ts).

If the user doesn't have `mise` installed, the host fails the spawn
with a pointer to <https://mise.jdx.dev/getting-started.html>. Rust
areas don't need `mise`.
