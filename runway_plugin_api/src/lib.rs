pub mod helpers;

use serde::{Deserialize, Serialize};

/// Batch request for runway selections sent to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunwaySelectionsRequest {
    pub airports: Vec<AirportSelectionRequest>,
}

/// Per-airport data sent to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub enum CrosswindDirection {
    Left,
    Right,
    Variable,
}

/// METAR data: raw string plus optionally parsed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetarData {
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<ParsedMetar>,
}

/// Structured METAR content.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct RvrData {
    /// Runway designator, e.g. "28L"
    pub runway: String,
    /// RVR value in metres. `None` if unreadable (reported as /////).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meters: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub enum CloudCoverage {
    Few,
    Scattered,
    Broken,
    Overcast,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub enum WeatherIntensity {
    Light,
    Heavy,
    Vicinity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct RunwaySelectionsResponse {
    pub results: Vec<AirportSelectionResult>,
}

/// Per-airport result from the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct RunwayUseEntry {
    /// Runway identifier, e.g. "01L"
    pub runway: String,
    #[serde(rename = "use")]
    pub use_: RunwayUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunwayUse {
    Departing,
    Arriving,
    Both,
}
