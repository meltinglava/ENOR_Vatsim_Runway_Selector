pub mod helpers;

use serde::{Deserialize, Serialize};

// ── Plugin API request / response types ──────────────────────────────────────

/// Batch request for runway selections sent to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwaySelectionsRequest {
    pub airports: Vec<AirportSelectionRequest>,
}

/// Per-airport data sent to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AirportSelectionRequest {
    /// ICAO airport identifier, e.g. "ENGM"
    pub icao: String,
    /// All runway directions at this airport with pre-computed wind components.
    pub runways: Vec<RunwayInfo>,
    /// METAR data, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metar: Option<MetarData>,
    /// Current UTC time as an RFC 3339 string, e.g. "2026-05-14T10:20:00Z"
    pub timestamp_utc: String,
}

/// A single runway direction with pre-computed wind components from the current METAR.
///
/// Wind components are `None` when no METAR is available.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwayInfo {
    /// Runway identifier, e.g. "01L", "19R", "18"
    pub identifier: String,
    /// Runway heading in degrees true (0–359)
    pub heading: u16,
    /// Max headwind in knots. Positive = headwind, negative = tailwind. `None` if no METAR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headwind_kt: Option<i32>,
    /// Max tailwind in knots (always ≥ 0). `None` if no METAR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tailwind_kt: Option<i32>,
    /// Max crosswind magnitude in knots (always ≥ 0). `None` if no METAR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crosswind_kt: Option<i32>,
    /// Direction the crosswind comes from relative to the runway centerline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crosswind_direction: Option<CrosswindDirection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CrosswindDirection {
    Left,
    Right,
    Variable,
}

/// METAR data: raw string plus optionally parsed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MetarData {
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<ParsedMetar>,
}

/// Structured METAR content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ParsedMetar {
    /// True if METAR reports CAVOK (no significant weather, visibility > 10 km).
    pub is_cavok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind: Option<WindData>,
    /// Prevailing visibility in metres. `None` if CAVOK or not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility_meters: Option<u32>,
    #[serde(default)]
    pub rvr: Vec<RvrData>,
    #[serde(default)]
    pub clouds: Vec<CloudData>,
    /// Vertical visibility in hundreds of feet. `None` if VV not reported.
    /// A value of 0 means VV was reported but the height was unreadable (VV///).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_visibility_hundreds_ft: Option<i32>,
    #[serde(default)]
    pub weather_phenomena: Vec<WeatherPhenomenonData>,
    /// Temperature in °C. `None` if not reported or unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<i32>,
    /// Dew point in °C. `None` if not reported or unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dew_point_c: Option<i32>,
    /// QNH in hPa. `None` if not reported or unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qnh_hpa: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WindData {
    /// Wind direction in degrees true (0–359). `None` if variable or unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction_degrees: Option<u32>,
    /// True when direction is explicitly reported as variable (VRB).
    pub is_variable: bool,
    /// Wind speed in knots.
    pub speed_kt: u32,
    /// Gust speed in knots, if reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gust_kt: Option<u32>,
    /// Start of variable wind range, e.g. 250 in "250V310".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_from_degrees: Option<u32>,
    /// End of variable wind range, e.g. 310 in "250V310".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_to_degrees: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RvrData {
    /// Runway designator, e.g. "28L"
    pub runway: String,
    /// RVR value in metres. `None` if unreadable (reported as /////).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meters: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CloudData {
    /// Cloud coverage. `None` if unreadable (reported as ///).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CloudCoverage>,
    /// Cloud base in hundreds of feet. `None` if unreadable (reported as ///).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_hundreds_ft: Option<i32>,
    /// Cloud type, e.g. "CB", "TCU". `None` if not reported or unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum CloudCoverage {
    Few,
    Scattered,
    Broken,
    Overcast,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WeatherPhenomenonData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity: Option<WeatherIntensity>,
    #[serde(default)]
    pub descriptors: Vec<WeatherDescriptor>,
    /// METAR weather codes: "DZ", "RA", "SN", "FG", etc.
    #[serde(default)]
    pub phenomena: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum WeatherIntensity {
    Light,
    Heavy,
    Vicinity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum WeatherDescriptor {
    Shallow,
    Partial,
    Patches,
    LowDrifting,
    Blowing,
    Shower,
    Thunderstorm,
    Freezing,
}

/// Plugin response for all airports in the batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwaySelectionsResponse {
    pub results: Vec<AirportSelectionResult>,
}

/// Per-airport result from the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AirportSelectionResult {
    pub icao: String,
    /// `true` if the plugin is handling this airport.
    /// `false` means the caller should use its own fallback logic.
    pub handled: bool,
    /// Non-empty only when `handled` is `true`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runway_uses: Vec<RunwayUseEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwayUseEntry {
    /// Runway identifier, e.g. "01L"
    pub runway: String,
    #[serde(rename = "use")]
    pub use_: RunwayUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RunwayUse {
    Departing,
    Arriving,
    Both,
}

// ── Helpers API request / response types ─────────────────────────────────────
//
// These describe the JSON bodies for the HTTP helpers server that
// es_runway_selector hosts. They are mirrored in the generated OpenAPI spec
// so that plugin authors can generate typed clients in any language.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BestHeadwindRequest {
    pub runways: Vec<RunwayInfo>,
    /// Minimum headwind advantage over the runner-up required to declare a winner.
    /// Use `0` to always pick the leader when any advantage exists.
    pub advantage_threshold_kt: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreferUnlessTailwindRequest {
    pub runways: Vec<RunwayInfo>,
    /// Identifier of the preferred runway (e.g. `"18"`).
    pub preferred_id: String,
    /// Switch away from the preferred runway when its tailwind exceeds this.
    pub max_tailwind_kt: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreferUnlessCrosswindRequest {
    pub runways: Vec<RunwayInfo>,
    /// Identifier of the preferred runway (e.g. `"27"`).
    pub preferred_id: String,
    /// Switch away from the preferred runway when its crosswind exceeds this.
    pub max_crosswind_kt: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MinCrosswindRequest {
    pub runways: Vec<RunwayInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WithinCrosswindLimitRequest {
    pub runways: Vec<RunwayInfo>,
    pub max_kt: i32,
}

/// Single-runway result — `runway` is `null` when no runway qualifies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwayResult {
    pub runway: Option<String>,
}

/// Multi-runway result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwaysResult {
    pub runways: Vec<String>,
}

// ── OpenAPI spec ──────────────────────────────────────────────────────────────

// ── Plugin API OpenAPI spec ───────────────────────────────────────────────────
//
// Documents the two endpoints your plugin binary must implement.
// The Helpers API spec (endpoints hosted by es_runway_selector) is generated
// from the actual axum handlers in es_runway_selector and merged at build time.

#[cfg(feature = "openapi")]
#[allow(dead_code)]
pub mod plugin_api_paths {
    use super::*;

    /// Health check — return 200 to indicate the plugin is ready to accept requests.
    #[utoipa::path(
        get,
        path = "/health",
        tag = "Plugin API",
        responses((status = 200, description = "Plugin is ready"))
    )]
    pub fn plugin_health() {}

    /// Batch runway selection.
    ///
    /// `es_runway_selector` sends all airports in a single request.
    /// Return `handled: true` with `runway_uses` for airports you manage;
    /// return `handled: false` to let es_runway_selector apply its own fallback.
    #[utoipa::path(
        post,
        path = "/runway-selections",
        tag = "Plugin API",
        request_body = RunwaySelectionsRequest,
        responses((status = 200, body = RunwaySelectionsResponse))
    )]
    pub fn plugin_runway_selections() {}
}

#[cfg(feature = "openapi")]
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Runway Plugin API",
        description = "
Endpoints your plugin binary must implement.
`es_runway_selector` spawns your binary with `--port N --helpers-port M`,
waits for `/health` to return 200, then POST to `/runway-selections` once per run.
",
        version = "1"
    ),
    paths(
        plugin_api_paths::plugin_health,
        plugin_api_paths::plugin_runway_selections,
    ),
    components(schemas(
        RunwaySelectionsRequest,
        AirportSelectionRequest,
        RunwayInfo,
        CrosswindDirection,
        MetarData,
        ParsedMetar,
        WindData,
        RvrData,
        CloudData,
        CloudCoverage,
        WeatherPhenomenonData,
        WeatherIntensity,
        WeatherDescriptor,
        RunwaySelectionsResponse,
        AirportSelectionResult,
        RunwayUseEntry,
        RunwayUse,
    )),
    tags(
        (name = "Plugin API", description = "Endpoints your plugin must implement"),
    )
)]
pub struct PluginApiDoc;
