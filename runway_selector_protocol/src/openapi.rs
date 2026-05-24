use utoipa::OpenApi;

use crate::types::{
    AirportInfo, AirportRunwayAssignment, AtisEntry, AtisRequest, AtisResponse, CloudCoverage,
    CloudLayer, MetarData, ParseAtisRequest, ParseAtisResponse, ParseMetarRequest,
    ParseMetarResponse, PhysicalRunway, PluginAirportsResponse, RunwayAssignment, RunwayInfo,
    RunwaySelectionRequest, RunwaySelectionResponse, RunwayUse, VariableWind, WindData,
    WindDirection,
};

/// Serialize the full plugin + parent OpenAPI spec to a pretty-printed JSON string.
pub fn generate_openapi_json() -> String {
    use utoipa::OpenApi as _;
    PluginAndParentApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI serialization failed")
}

/// Combined OpenAPI document covering both:
/// - The plugin HTTP API (what implementors must expose)
/// - The parent helper API (what `es_runway_selector` exposes)
///
/// Use `PluginAndParentApiDoc::openapi().to_pretty_json()` to serialize.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "ES Runway Selector – Plugin & Parent API",
        version = "1",
        description = "
## Plugin API (implementors must expose these endpoints)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check – return 200 when ready |
| GET | `/airports` | List ICAO codes this plugin handles |
| POST | `/atis` | Parse ATIS texts and return runway assignments |
| POST | `/runways` | Select active runways from METAR data |

## Parent API (helpers exposed by `es_runway_selector`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check |
| POST | `/parse-atis` | Parse ATIS using the built-in regex parser |
| POST | `/parse-metar` | Parse a raw METAR string |

The parent port is provided to plugins via the `ES_RUNWAY_SELECTOR_PORT` environment variable.
The plugin's own port is provided via `ES_RUNWAY_SELECTOR_PLUGIN_PORT`.
"
    ),
    components(schemas(
        // shared primitives
        WindDirection, VariableWind, WindData,
        CloudCoverage, CloudLayer,
        MetarData,
        RunwayInfo, PhysicalRunway, AirportInfo,
        RunwayUse, RunwayAssignment, AirportRunwayAssignment,
        // plugin API
        AtisEntry, AtisRequest, AtisResponse,
        RunwaySelectionRequest, RunwaySelectionResponse,
        PluginAirportsResponse,
        // parent helpers
        ParseAtisRequest, ParseAtisResponse,
        ParseMetarRequest, ParseMetarResponse,
    )),
    tags(
        (name = "plugin", description = "Endpoints a plugin must implement"),
        (name = "parent", description = "Helper endpoints exposed by es_runway_selector"),
    )
)]
pub struct PluginAndParentApiDoc;
