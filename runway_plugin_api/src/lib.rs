//! The HTTP/JSON contract between `es_runway_selector` (the host) and area
//! plugins, plus ready-made selection helpers ([`helpers`]).
//!
//! An area plugin is a subprocess that serves plain HTTP on
//! `127.0.0.1:$RUNWAY_SELECTOR_PORT` and implements:
//!
//! - `GET  /health` — return `200` once ready to accept requests.
//! - `POST /runway-selections` — body [`RunwaySelectionsRequest`], response
//!   [`RunwaySelectionsResponse`]. One batch request per host run.
//! - `POST /shutdown` — optional but recommended: begin a graceful exit and
//!   return `200`. The host posts this before terminating the process, which
//!   is the only reliable graceful-shutdown signal on Windows.
//!
//! The host pre-computes per-runway wind components (headwind / tailwind /
//! crosswind and side) and ships them together with the parsed METAR and a
//! UTC timestamp — plugins never do wind trigonometry and must never read
//! the wall clock ([`RunwaySelectionsRequest::timestamp_utc`] is the time).
//!
//! ATIS-derived runways are applied by the host itself; airports already
//! decided by ATIS are not included in the request.

pub mod helpers;

use serde::{Deserialize, Serialize};

// ── Plugin API request / response types ──────────────────────────────────────

/// Batch request for runway selections sent to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunwaySelectionsRequest {
    /// Current UTC time as an RFC 3339 string, e.g. `"2026-05-14T10:20:00Z"`.
    /// Plugins must use this — never the wall clock — so selections are a
    /// pure function of the request.
    pub timestamp_utc: String,
    /// IANA timezone for the area (e.g. `"Europe/Oslo"`), for any
    /// time-of-day rules (night modes, LVP windows, …).
    pub area_timezone: String,
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
    /// What the selection was derived from. The host uses this for its
    /// source-priority table (ATIS > METAR > DEFAULT); ATIS never appears
    /// here because the host applies ATIS itself.
    #[serde(default)]
    pub source: SelectionSource,
    /// Non-empty only when `handled` is `true`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runway_uses: Vec<RunwayUseEntry>,
    /// Tags explaining and/or qualifying this selection. Empty when none apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<SelectionTag>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum SelectionSource {
    /// Derived from the METAR (wind, weather, …).
    #[default]
    Metar,
    /// Fell back to the area's configured default (e.g. calm-wind runway).
    Default,
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

// ── Selection tags ────────────────────────────────────────────────────────────

/// Static display metadata for a registered tag. Not transmitted over the wire.
///
/// Define instances as `pub const` in the [`tags`] module (well-known) or in
/// your own crate (plugin-specific). Use [`Tag::reason`] / [`Tag::conflict`]
/// to construct a [`SelectionTag`] for inclusion in a response.
pub struct Tag {
    pub id: &'static str,
    /// Unicode symbol / emoji shown in the report column.
    pub symbol: &'static str,
    /// Human-readable label used as a tooltip.
    pub label: &'static str,
}

impl Tag {
    /// Returns a [`SelectionTag`] that explains *why* this runway was chosen.
    pub fn reason(&self) -> SelectionTag {
        self.selection_tag(false)
    }

    /// Returns a [`SelectionTag`] for a negative factor that was *accepted*
    /// against the runway choice (e.g. tailwind accepted due to LVP).
    pub fn conflict(&self) -> SelectionTag {
        self.selection_tag(true)
    }

    fn selection_tag(&self, conflict: bool) -> SelectionTag {
        SelectionTag {
            id: self.id.to_string(),
            conflict,
            symbol: self.symbol.to_string(),
            label: self.label.to_string(),
        }
    }
}

/// A tag attached to a runway selection, transmitted in plugin responses.
///
/// `conflict = false` → the tag *explains* the selection (a reason).
/// `conflict = true`  → the tag marks a *negative factor that was accepted*
/// against the chosen runway (e.g. tailwind accepted during segregated ops).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SelectionTag {
    /// Stable identifier; matches a [`Tag::id`] in the registry.
    pub id: String,
    /// Whether this tag represents an accepted negative factor.
    pub conflict: bool,
    /// Unicode symbol / emoji shown in the report (populated from [`Tag`]).
    pub symbol: String,
    /// Full label used as a tooltip (populated from [`Tag`]).
    pub label: String,
}

/// Well-known tag constants shared across host and plugins.
///
/// Plugin-specific tags (e.g. ENGM runway modes) should be defined as
/// `pub const` [`Tag`] values in the plugin's own crate; hosts render tags
/// they don't know by id as neutral pills.
pub mod tags {
    use super::Tag;

    pub const TAILWIND: Tag = Tag {
        id: "tailwind",
        symbol: "⬇",
        label: "Tailwind on selected runway",
    };

    pub const LVP: Tag = Tag {
        id: "lvp",
        symbol: "🌫",
        label: "Low Visibility Procedures active",
    };
}

// ── OpenAPI spec ──────────────────────────────────────────────────────────────
//
// Code-first: the spec is derived from these Rust types so it cannot drift
// from the implementation. `cargo run -p runway_plugin_api --features openapi
// --bin generate_openapi` prints the JSON document.

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
    /// `es_runway_selector` sends all airports it wants the plugin to decide
    /// in a single request. Return `handled: true` with `runway_uses` for
    /// airports you manage; return `handled: false` to let the host apply its
    /// own fallback for that airport only.
    #[utoipa::path(
        post,
        path = "/runway-selections",
        tag = "Plugin API",
        request_body = RunwaySelectionsRequest,
        responses((status = 200, body = RunwaySelectionsResponse))
    )]
    pub fn plugin_runway_selections() {}

    /// Graceful shutdown request. Return 200 and then exit. Implementing this
    /// is the only way the host can shut you down gracefully on Windows.
    #[utoipa::path(
        post,
        path = "/shutdown",
        tag = "Plugin API",
        responses((status = 200, description = "Plugin will exit"))
    )]
    pub fn plugin_shutdown() {}
}

#[cfg(feature = "openapi")]
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Runway Plugin API",
        description = "
Endpoints your area plugin must implement.
`es_runway_selector` spawns your binary/script with the environment variables
`RUNWAY_SELECTOR_PORT` (HTTP port to bind on 127.0.0.1) and
`RUNWAY_SELECTOR_AREA_DIR` (your installed package directory), waits for
`GET /health` to return 200, then POSTs to `/runway-selections` once per run
and finally POSTs `/shutdown`.
",
        version = "1"
    ),
    paths(
        plugin_api_paths::plugin_health,
        plugin_api_paths::plugin_runway_selections,
        plugin_api_paths::plugin_shutdown,
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
        SelectionSource,
        RunwayUseEntry,
        RunwayUse,
        SelectionTag,
    )),
    tags(
        (name = "Plugin API", description = "Endpoints your plugin must implement"),
    )
)]
pub struct PluginApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_source_defaults_to_metar_when_absent() {
        let json =
            r#"{"icao":"ENGM","handled":true,"runway_uses":[{"runway":"01L","use":"Both"}]}"#;
        let result: AirportSelectionResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.source, SelectionSource::Metar);
        assert!(result.tags.is_empty());
    }

    #[test]
    fn tag_reason_and_conflict_round_trip() {
        let reason = tags::LVP.reason();
        assert_eq!(reason.id, "lvp");
        assert!(!reason.conflict);
        let conflict = tags::TAILWIND.conflict();
        assert!(conflict.conflict);

        let json = serde_json::to_string(&conflict).unwrap();
        let back: SelectionTag = serde_json::from_str(&json).unwrap();
        assert_eq!(back, conflict);
    }

    #[test]
    fn request_round_trips() {
        let req = RunwaySelectionsRequest {
            timestamp_utc: "2026-05-14T10:20:00Z".into(),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![AirportSelectionRequest {
                icao: "ENGM".into(),
                runways: vec![RunwayInfo {
                    identifier: "01L".into(),
                    heading: 7,
                    headwind_kt: Some(10),
                    tailwind_kt: Some(0),
                    crosswind_kt: Some(3),
                    crosswind_direction: Some(CrosswindDirection::Left),
                }],
                metar: None,
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RunwaySelectionsRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.airports[0].icao, "ENGM");
        assert_eq!(back.airports[0].runways[0].headwind_kt, Some(10));
    }
}
