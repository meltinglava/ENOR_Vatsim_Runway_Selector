# Area developer guide

This crate is the gRPC contract that every area plugin for
[`es_runway_selector`](../README.md) implements. This README is the
**area author's** guide — read it if you want to teach the host how to
pick runways for a FIR (or any set of airports) it doesn't already know
about.

If you only want to install and use existing areas, see the top-level
[README](../README.md) instead.

> **Status: pre-1.0.** The `.proto` is on `package runway_selector.v1`
> and any breaking change will bump the package to `v2`, but expect the
> v1 surface itself to evolve until a 1.0 host release is tagged.

---

## How it works

The host (`es_runway_selector`) treats each area as a separate process:

1. The host locates the installed area on disk (`<install_dir>/<name>/`)
   and reads its `manifest.toml`.
2. It picks a free localhost port, then spawns the entry point in
   `manifest.toml`. For Rust areas that's a direct `exec`; for Python /
   Node / Deno areas, the host routes the launch through
   [`mise`](https://mise.jdx.dev/) so end users don't have to install
   the runtime themselves (see [Choosing a runtime](#choosing-a-runtime)).
3. Two environment variables are set for the child:
   - `RUNWAY_SELECTOR_PORT` — the port the plugin must bind on `127.0.0.1`.
   - `RUNWAY_SELECTOR_AREA_DIR` — the absolute path to the area
     directory on disk (where `area.toml`, `profiles/`, etc. live).
4. The host polls
   [`grpc.health.v1.Health/Check`](https://github.com/grpc/grpc/blob/master/doc/health-checking.md)
   until the plugin reports `SERVING`. The default startup timeout is
   10 seconds.
5. Once the plugin is healthy, the host calls
   `runway_selector.v1.RunwaySelector/GetAirports` once (to learn which
   ICAOs the plugin claims), then `SelectRunways` for those airports.
6. After applying the response, the host terminates the subprocess.

The contract therefore has three pieces: the **package layout on disk**,
the **gRPC service**, and the **runtime conventions** that make
non-Rust plugins work. Each is described below.

---

## Area package layout

```text
<install_dir>/<name>/
    manifest.toml          # immutable identity (never edited by users)
    area.toml              # runtime defaults shipped by you
    area.local.toml        # user sparse overrides (preserved across updates)
    plugin/<entry>         # the binary/script the host spawns
    profiles/<profile>.toml         # controller positions (prf, app launchers)
    profiles/<profile>.local.toml   # user sparse overrides
    test_fixtures/         # optional, anything you like
```

**The user-facing rule: anything ending in `.local.toml` belongs to the
end user and is preserved across area updates.** All `*.toml` files
support a sibling `*.local.toml` that the host overlays on top — tables
merge key-by-key, every other value is replaced wholesale. See
[The `.local.toml` overlay](#the-localtoml-overlay) below.

### `manifest.toml`

The area's identity. Replaced wholesale on update; never subject to
`.local.toml` overrides.

```toml
name = "enor"
version = "0.1.0"
display_name = "Polaris / ENOR (Norway FIR)"
description = "Runway selection logic for the Norwegian FIR (ENOR)."
runtime = "rust"
entry = "area_enor"
supported_icaos = ["ENGM", "ENZV", "ENBR", "..."]
# optional:
min_core_version = "0.1.0"
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | yes | Globally unique slug. Matches the directory name and the registry entry. |
| `version` | semver | yes | Area version. The host enforces a bump on update. |
| `display_name` | string | yes | Human-readable label shown in `area list`. |
| `description` | string | no | One-line description shown in `area available`. |
| `runtime` | enum | yes | `rust` \| `python` \| `node` \| `deno`. Controls how the host launches `entry`. |
| `entry` | string | yes | Path **relative to `plugin/`** of the executable or script the host spawns. |
| `supported_icaos` | string[] | no | Informational only — the *runtime* answer comes from `GetAirports`. |
| `min_core_version` | semver | no | Minimum host version required. The host refuses to spawn older plugins. |

### `area.toml`

Runtime defaults the host needs and that the plugin may want to read.
The host parses this directly (it doesn't go over gRPC), so the schema
is fixed by `runway_selector_core::area_config::AreaConfig`:

```toml
metar_urls       = ["https://metar.vatsim.net/EN", "https://metar.vatsim.net/ESKS"]
time_zone        = "Europe/Oslo"
sector_file_prefix = "ENOR"

ignore_airports  = ["ENQC", "ENQR"]   # bad METARs etc.

[default_runways]
ENGM = 1
ENZV = 18
```

| Field | Type | Meaning |
| --- | --- | --- |
| `metar_urls` | string[] | URLs the host fetches METARs from. Each response is parsed line-by-line. |
| `time_zone` | string | IANA zone the host puts into `SelectRunwaysRequest.area_timezone`. |
| `sector_file_prefix` | string | EuroScope `.sct` filename prefix (e.g. `ENOR` matches `ENOR-*.sct`). |
| `ignore_airports` | string[] | ICAOs whose METARs the host drops before parsing. |
| `default_runways` | table<ICAO, u8> | Per-ICAO fallback heading-in-tens-of-degrees (e.g. `18` → runway 18). Used when neither ATIS, METAR, nor the plugin produced a selection. |

All fields default to empty / absent if you omit them.

### `profiles/<name>.toml`

A controller position (TWR, APP, RADAR…) — picks which EuroScope `.prf`
file opens and which extra apps should launch alongside.

```toml
name         = "rads"
display_name = "Radar / Approach"
prf_files    = ["enor_rads.prf"]
default_apps = ["EuroScope", "TrackAudio", "vacs"]
```

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | string | Profile slug. Must match the filename stem. |
| `display_name` | string | Human-readable label shown in `area profile list`. |
| `prf_files` | path[] | `.prf` files inside the EuroScope sector folder to open. |
| `default_apps` | string[] | Names of entries from the user's `app_launchers.toml` to spawn alongside. |

### The `.local.toml` overlay

For any `foo.toml` shipped by the area, an `foo.local.toml` next to it
is overlaid on top — recursively. Tables merge key-by-key; scalars and
arrays are replaced wholesale. The merge is done by
`runway_selector_core::area_config::merge_local_overrides`.

Example. The area ships:

```toml
# area.toml
metar_urls = ["https://metar.vatsim.net/EN"]
[default_runways]
ENGM = 1
ENZV = 18
```

The end user writes:

```toml
# area.local.toml
[default_runways]
ENZV = 36       # I prefer landing the other way
```

The host parses the merged view:

```toml
metar_urls = ["https://metar.vatsim.net/EN"]    # unchanged
[default_runways]
ENGM = 1                                         # unchanged
ENZV = 36                                        # overridden
```

When publishing an update, **never ship a `.local.toml` file**. Update
your shipped `.toml` files only; the user's overrides will continue to
apply on top.

---

## The gRPC contract

### Subprocess lifecycle

When the host spawns your plugin:

1. `RUNWAY_SELECTOR_PORT` and `RUNWAY_SELECTOR_AREA_DIR` are set in the
   environment, and the current working directory is set to
   `RUNWAY_SELECTOR_AREA_DIR`.
2. Bind a gRPC server on `127.0.0.1:$RUNWAY_SELECTOR_PORT`.
3. Register two services on that server:
   - `grpc.health.v1.Health` — the standard health-check service.
     Mark `""` (and optionally
     `"runway_selector.v1.RunwaySelector"`) as `SERVING` as soon as
     you're ready to handle calls.
   - `runway_selector.v1.RunwaySelector` — the contract below.
4. Handle `SIGTERM` (and on Windows, `Ctrl+Break`) by shutting down
   cleanly. The host calls `kill` on the child after each cycle, so
   long-running state inside the plugin is not preserved between calls.

### Service definition

The authoritative definition is
[`proto/runway_selector.proto`](./proto/runway_selector.proto). The two
RPCs:

```proto
service RunwaySelector {
  rpc GetAirports  (google.protobuf.Empty)   returns (GetAirportsResponse);
  rpc SelectRunways(SelectRunwaysRequest)    returns (SelectRunwaysResponse);
}
```

#### `GetAirports`

Empty request. Return the list of ICAOs your plugin handles:

```proto
message GetAirportsResponse {
  repeated string icaos = 1;
}
```

The host filters its airport list by this set before calling
`SelectRunways`. Order is not significant.

#### `SelectRunways`

The host calls this once per cycle with the airports you claimed:

```proto
message SelectRunwaysRequest {
  google.protobuf.Timestamp now_utc = 1;   // wall-clock at request time
  string area_timezone = 2;                // IANA zone, e.g. "Europe/Oslo"
  repeated AirportRequest airports = 3;
}

message AirportRequest {
  string icao = 1;
  repeated RunwayInfo runways = 2;         // every direction at this airport
  optional Metar metar = 3;                // parsed METAR or absent
  repeated RunwayAssignment atis_runways = 4;  // host-parsed ATIS, may be empty
}

message RunwayInfo {
  string identifier = 1;                   // "01L", "19R", "18"
  uint32 heading_degrees_true = 2;         // 0–359
  optional WindComponents wind_components = 3;  // pre-computed by the host
}
```

The host pre-computes per-runway `WindComponents` (headwind /
crosswind / direction of crosswind) from the METAR. **Use them.** They
encode the same math the host's wind-component formatting uses, so any
selection you make is consistent with what's shown in the runway report.

Respond with one `AirportSelection` per airport you want to assign
runways to:

```proto
message SelectRunwaysResponse {
  repeated AirportSelection selections = 1;
}

message AirportSelection {
  string icao = 1;
  SelectionSource source = 2;              // ATIS | METAR | DEFAULT
  repeated RunwayAssignment runways = 3;   // empty = "no selection this cycle"
}

message RunwayAssignment {
  string identifier = 1;                   // must match a RunwayInfo.identifier
  RunwayUse use = 2;                       // DEPARTING | ARRIVING | BOTH
}
```

**Source attribution matters.** The host stores selections by source
and the `.rwy` writer / HTML report walk `ATIS > METAR > DEFAULT` and
take the first present. So:

- If the host gave you `atis_runways` and you trust them, return them
  with `source = ATIS`. Controllers' authority overrides your math.
- If you derived the selection from the METAR / wind components, return
  `source = METAR`.
- If you fell back to the area's `default_runways` because the wind was
  ambiguous or there was no METAR, return `source = DEFAULT`.
- If you have no opinion at all (no METAR, no ATIS, can't decide),
  **omit the airport from the response**. The host will then fall back
  to its own `apply_default_runways` step.

### When to emit what — common patterns

| Situation | What to return |
| --- | --- |
| Plugin doesn't know this airport | Omit from `GetAirports`. |
| ATIS picked a runway and you trust it | Pass `atis_runways` through with `source = ATIS`. |
| Wind clearly favours one direction | One `RunwayAssignment` with `source = METAR`. |
| Two directions tie within a margin | Omit the airport; let the host fall back to defaults. |
| Parallel runways open at once | Two assignments under the same airport, both with `use = BOTH` (mixed ops) or split as `DEPARTING` / `ARRIVING` (segregated ops). |
| Local-time mode rules apply (e.g. "after 22:30 segregated") | Use `now_utc` + `area_timezone` from the request — **don't** call `Zoned::now()` directly so tests stay deterministic. |

See `area_enor::selector` for a worked example covering all of these.

---

## Choosing a runtime

You can write an area in any language with a gRPC implementation. The
host launches the entry point based on the `runtime` field in
`manifest.toml`:

| Runtime | How the host launches it | Use when |
| --- | --- | --- |
| `rust` | Exec the entry directly. You ship a native binary per OS. | You want the smallest install, no language manager, full type safety. |
| `python` | `mise exec python -- python <entry>` | You want quick iteration and don't mind shipping `.py` files. |
| `node` | `mise exec node -- node <entry>` | Same as Python but for JS/TS ecosystems. |
| `deno` | `mise exec deno -- deno <entry>` | TypeScript with native protobuf support and no `package.json`. |

For non-Rust runtimes the host invokes
[`mise`](https://mise.jdx.dev/) so end users don't have to install
Python / Node / Deno themselves. Ship a `mise.toml` (or `.tool-versions`)
at the root of your area package — `mise` will pick up the pinned
version when it runs your entry. Example:

```toml
# mise.toml
[tools]
python = "3.12"
```

If `mise` isn't installed on the user's machine, the host fails the
plugin spawn with a clear error pointing the user at
<https://mise.jdx.dev/getting-started.html>. Areas with
`runtime = "rust"` don't need `mise`.

### Rust example

The simplest working area is
[`area_enor`](../area_enor) — its `selector.rs` is ~250 lines and
covers ATIS pass-through, generic max-headwind, ENGM time-of-day mode
selection, and the ENZV crosswind switch.

Skeleton:

```rust
use runway_selector_protocol::v1::{
    AirportRequest, AirportSelection, GetAirportsResponse, RunwayAssignment, RunwayUse,
    SelectRunwaysRequest, SelectRunwaysResponse, SelectionSource,
    runway_selector_server::{RunwaySelector, RunwaySelectorServer},
};
use std::{env, net::SocketAddr};
use tonic::{Request, Response, Status, transport::Server};

pub struct MyArea;

#[tonic::async_trait]
impl RunwaySelector for MyArea {
    async fn get_airports(
        &self,
        _: Request<()>,
    ) -> Result<Response<GetAirportsResponse>, Status> {
        Ok(Response::new(GetAirportsResponse {
            icaos: vec!["EXAM".into()],
        }))
    }

    async fn select_runways(
        &self,
        request: Request<SelectRunwaysRequest>,
    ) -> Result<Response<SelectRunwaysResponse>, Status> {
        let selections = request
            .into_inner()
            .airports
            .into_iter()
            .filter_map(pick)
            .collect();
        Ok(Response::new(SelectRunwaysResponse { selections }))
    }
}

fn pick(a: AirportRequest) -> Option<AirportSelection> {
    let best = a.runways.iter().max_by_key(|r| {
        r.wind_components.as_ref().map(|w| w.headwind_kt).unwrap_or(i32::MIN)
    })?;
    Some(AirportSelection {
        icao: a.icao,
        source: SelectionSource::Metar as i32,
        runways: vec![RunwayAssignment {
            identifier: best.identifier.clone(),
            r#use: RunwayUse::Both as i32,
        }],
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = env::var("RUNWAY_SELECTOR_PORT")?.parse()?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;

    let (health, health_svc) = tonic_health::server::health_reporter();
    health.set_serving::<RunwaySelectorServer<MyArea>>().await;

    Server::builder()
        .add_service(health_svc)
        .add_service(RunwaySelectorServer::new(MyArea))
        .serve(addr)
        .await?;
    Ok(())
}
```

Cargo dependencies: `tonic`, `tonic-prost`, `tonic-health`, `prost`,
`prost-types`, `tokio` with `macros + rt-multi-thread`, plus
`tonic-prost-build` as a build dependency to compile the `.proto`.

### Python via mise example

```toml
# mise.toml at the area root
[tools]
python = "3.12"
```

```python
# plugin/server.py — manifest.toml entry = "server.py"
import asyncio, os
import grpc
import runway_selector_pb2 as pb
import runway_selector_pb2_grpc as rpc
from grpc_health.v1 import health, health_pb2, health_pb2_grpc

class MyArea(rpc.RunwaySelectorServicer):
    async def GetAirports(self, request, context):
        return pb.GetAirportsResponse(icaos=["EXAM"])

    async def SelectRunways(self, request, context):
        selections = []
        for a in request.airports:
            best = max(
                (r for r in a.runways if r.HasField("wind_components")),
                key=lambda r: r.wind_components.headwind_kt,
                default=None,
            )
            if best is None:
                continue
            selections.append(pb.AirportSelection(
                icao=a.icao,
                source=pb.SELECTION_SOURCE_METAR,
                runways=[pb.RunwayAssignment(
                    identifier=best.identifier,
                    use=pb.RUNWAY_USE_BOTH,
                )],
            ))
        return pb.SelectRunwaysResponse(selections=selections)

async def serve():
    port = int(os.environ["RUNWAY_SELECTOR_PORT"])
    server = grpc.aio.server()
    rpc.add_RunwaySelectorServicer_to_server(MyArea(), server)

    svc = health.HealthServicer()
    health_pb2_grpc.add_HealthServicer_to_server(svc, server)
    await svc.set("", health_pb2.HealthCheckResponse.SERVING)

    server.add_insecure_port(f"127.0.0.1:{port}")
    await server.start()
    await server.wait_for_termination()

asyncio.run(serve())
```

Generate `runway_selector_pb2.py` / `_pb2_grpc.py` once at packaging
time with `grpcio-tools`:

```bash
mise exec python -- python -m grpc_tools.protoc \
    -I path/to/runways/runway_selector_protocol/proto \
    --python_out=plugin --grpc_python_out=plugin \
    path/to/runways/runway_selector_protocol/proto/runway_selector.proto
```

Ship the generated files inside `plugin/`. Don't make end users
regenerate them.

`manifest.toml`:

```toml
name = "myarea"
version = "0.1.0"
display_name = "My Area"
runtime = "python"
entry = "server.py"
```

### Node and Deno

Same pattern — bind the port from `RUNWAY_SELECTOR_PORT`, register
the health service and `RunwaySelector` service, generate (or
import-at-runtime, in Deno's case) the proto stubs at packaging time
and ship them alongside `entry`.

---

## Publishing your area

End users install areas with `es_runway_selector area install <name>`.
The host fetches a registry JSON (one or more), finds the entry for
`<name>`, downloads the tarball, verifies the SHA-256, and extracts it
to `<install_dir>/<name>/`.

### 1. Package layout to ship

Tar the contents of your area directory **without** wrapping them in
the area name as a top-level directory:

```text
manifest.toml
area.toml
plugin/area_myarea           # or server.py / index.mjs / ...
profiles/twr.toml
profiles/rads.toml
mise.toml                    # only for non-Rust runtimes
```

The host creates `<install_dir>/<name>/` and untars into it, so an
extra leading directory would land at
`<install_dir>/<name>/<name>/...` and your entry would never be
found.

### 2. Build the tarball

```bash
cd /path/to/your/area-package
tar -czf myarea-0.1.0.tar.gz .
```

For Rust plugins, build the entry per target OS and put the resulting
binary at `plugin/<entry>`. You'll typically ship one tarball per
`(name, version, target)` tuple.

### 3. Compute the SHA-256

```bash
sha256sum myarea-0.1.0.tar.gz
```

You'll need the hex digest for the registry entry.

### 4. Registry JSON entry

A registry is a JSON document with this shape:

```json
{
  "schema_version": 1,
  "areas": [
    {
      "name": "myarea",
      "display_name": "My Area",
      "description": "Runway selection for the XYZ FIR",
      "version": "0.1.0",
      "download_url": "https://example.org/myarea-0.1.0.tar.gz",
      "checksum_sha256": "<hex digest from step 3>",
      "maintainers": ["yourhandle"]
    }
  ]
}
```

`schema_version` must currently be `1`. Areas are matched by `name`,
so multiple registries listing the same name will be deduplicated with
"later registry wins".

### 5. Distribution options

You have two ways to get your area in front of users:

- **Primary registry.** Open a PR against the upstream registry JSON
  served at `area_registry_url` (default
  `https://raw.githubusercontent.com/meltinglava/ENOR_Vatsim_Runway_Selector/main/areas.json`).
  Once merged, every user can `area install <name>` without any extra
  configuration.

- **Self-hosted / extra registry.** Host your own `areas.json`
  somewhere (a GitHub Pages site, an S3 bucket, anywhere reachable
  over HTTPS), then tell users to add it to their
  `<config_dir>/config.local.toml`:

  ```toml
  extra_registries = ["https://example.org/my-areas.json"]
  ```

  The host fetches the primary registry plus every entry in
  `extra_registries`, dedupes by name, and presents the union. Useful
  for staging, private FIRs, or areas you don't want in the upstream
  registry.

---

## Reference: regenerating the protocol code

For non-Rust runtimes you need to generate language-specific stubs
from the `.proto`. The protobuf descriptor lives at
[`proto/runway_selector.proto`](./proto/runway_selector.proto).

You need `protoc` available — `mise` can install it for you:

```bash
mise use -g protoc        # or:  mise install protoc@latest
```

Then run the protoc plugin for your language. See the
[Python](#python-via-mise-example) example above for `grpcio-tools`,
or the [grpc documentation](https://grpc.io/docs/languages/) for
Node / Go / Java / C# / etc.

For Rust, the build is automatic — depend on
`runway_selector_protocol = { path = "..." }` (workspace) or pin a
git revision, and `build.rs` will regenerate `v1` from the
`.proto` on every build.
