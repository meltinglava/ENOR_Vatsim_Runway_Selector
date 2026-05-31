# Writing an area plugin

An *area* is a per-FIR plugin that picks runways. The host
(`es_runway_selector`) spawns it as a subprocess, talks to it over
gRPC, and applies the result to the `.rwy` file.

This guide walks you through writing one from scratch.

> **Pre-1.0.** The contract is on `package runway_selector.v1`. Any
> breaking change bumps to `v2`, but the v1 surface itself is still
> evolving.

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
name         = "my-area"
version      = "0.1.0"
display_name = "My Area"
runtime      = "rust"           # rust | python | node | deno
entry        = "area_my_area"   # path relative to plugin/
```

Optional: `description`, `supported_icaos` (informational — the live
list comes from `GetAirports`), `min_core_version`.

## 3. Write `area.toml`

Runtime defaults the host reads directly:

```toml
metar_urls         = ["https://metar.vatsim.net/EN"]
time_zone          = "Europe/Oslo"     # IANA — used for local-time rules
sector_file_prefix = "ENOR"            # matches ENOR-*.sct
ignore_airports    = ["ENQR"]          # ICAOs whose METARs to drop

[default_runways]
ENGM = 1                                # heading-in-tens-of-degrees fallback
```

All fields are optional.

## 4. Implement the selector

The host runs your `plugin/<entry>` with these env vars set:

- `RUNWAY_SELECTOR_PORT` — bind a gRPC server on `127.0.0.1:$PORT`.
- `RUNWAY_SELECTOR_AREA_DIR` — your installed area directory (also the cwd).

Register two services on that server: `grpc.health.v1.Health` (mark
`""` as `SERVING` once you're ready) and
`runway_selector.v1.RunwaySelector`. The host polls health, then calls
two RPCs:

1. `GetAirports()` — return the ICAOs you handle.
2. `SelectRunways(request)` — return your picks for those ICAOs.

The host pre-computes per-runway `WindComponents` (head/cross wind)
from the METAR. **Use them.** Your math will line up with the runway
report.

### Rust skeleton

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
    async fn get_airports(&self, _: Request<()>) -> Result<Response<GetAirportsResponse>, Status> {
        Ok(Response::new(GetAirportsResponse { icaos: vec!["EXAM".into()] }))
    }

    async fn select_runways(
        &self,
        request: Request<SelectRunwaysRequest>,
    ) -> Result<Response<SelectRunwaysResponse>, Status> {
        let selections = request.into_inner().airports.into_iter().filter_map(pick).collect();
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

Cargo deps: `tonic`, `tonic-prost`, `tonic-health`, `prost`,
`prost-types`, `tokio` (`macros + rt-multi-thread`); build dep
`tonic-prost-build`.

For a worked example covering ATIS pass-through, mixed/segregated/single
ops by local time, and a crosswind-driven runway switch, see
[`area_enor/src/selector.rs`](../area_enor/src/selector.rs).

### Python / Node / Deno

The host runs non-Rust entries through
[`mise`](https://mise.jdx.dev/), so end users don't need the runtime
installed. See [Non-Rust runtimes](#non-rust-runtimes) for the pattern.

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

Symlink (or copy) your in-progress package into the host's install dir:

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

### `SelectRunways` request

```proto
message SelectRunwaysRequest {
  google.protobuf.Timestamp now_utc = 1;       // use this, not the local clock — keeps tests deterministic
  string area_timezone = 2;                    // IANA, from your area.toml
  repeated AirportRequest airports = 3;
}

message AirportRequest {
  string icao = 1;
  repeated RunwayInfo runways = 2;             // every direction at this airport
  optional Metar metar = 3;
  repeated RunwayAssignment atis_runways = 4;  // host-parsed ATIS, may be empty
}

message RunwayInfo {
  string identifier = 1;                       // "01L", "19R", "18"
  uint32 heading_degrees_true = 2;
  optional WindComponents wind_components = 3; // pre-computed by the host
}
```

Full schema:
[`proto/runway_selector.proto`](./proto/runway_selector.proto).

### `SelectRunways` response

```proto
message AirportSelection {
  string icao = 1;
  SelectionSource source = 2;                  // ATIS | METAR | DEFAULT
  repeated RunwayAssignment runways = 3;
}

message RunwayAssignment {
  string identifier = 1;                       // must match a RunwayInfo.identifier
  RunwayUse use = 2;                           // DEPARTING | ARRIVING | BOTH
}
```

Source attribution matters — the `.rwy` writer prefers
`ATIS > METAR > DEFAULT` and takes the first one present:

| You picked from…                          | Set `source` to | Notes                                       |
| ----------------------------------------- | --------------- | ------------------------------------------- |
| `atis_runways` (controller authority)     | `ATIS`          | Pass them through verbatim.                 |
| Wind / METAR                              | `METAR`         |                                             |
| `default_runways` in `area.toml`          | `DEFAULT`       |                                             |
| Nothing — wind ambiguous, no METAR        | —               | Omit the airport. The host falls back.      |
| Parallel runways, mixed ops               | usually `METAR` | Two assignments, both `use = BOTH`.         |
| Parallel runways, segregated ops          | usually `METAR` | Two assignments: one `DEPARTING`, one `ARRIVING`. |

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

Generate gRPC stubs once at packaging time and ship them in `plugin/`
— don't make users regenerate them:

```bash
mise exec python -- python -m grpc_tools.protoc \
    -I path/to/runway_selector_protocol/proto \
    --python_out=plugin --grpc_python_out=plugin \
    path/to/runway_selector_protocol/proto/runway_selector.proto
```

Then bind the port, register health + `RunwaySelector`, and serve.
Python skeleton (Node and Deno mirror it almost line-for-line):

```python
# plugin/server.py   — manifest.toml: entry = "server.py", runtime = "python"
import asyncio, os
import grpc
import runway_selector_pb2 as pb, runway_selector_pb2_grpc as rpc
from grpc_health.v1 import health, health_pb2, health_pb2_grpc

class MyArea(rpc.RunwaySelectorServicer):
    async def GetAirports(self, request, context):
        return pb.GetAirportsResponse(icaos=["EXAM"])

    async def SelectRunways(self, request, context):
        ...  # build pb.SelectRunwaysResponse

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

If the user doesn't have `mise` installed, the host fails the spawn
with a pointer to <https://mise.jdx.dev/getting-started.html>. Rust
areas don't need `mise`.
