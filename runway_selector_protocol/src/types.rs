use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ─── Wind ────────────────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindDirection {
    Variable,
    Heading { degrees: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct VariableWind {
    pub from_degrees: u16,
    pub to_degrees: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct WindData {
    pub direction: WindDirection,
    /// Wind speed in knots.
    pub speed_kt: f64,
    /// Gust speed in knots, if reported.
    pub gust_kt: Option<f64>,
    /// Variable wind sector, e.g. `250V330`.
    pub variable_sector: Option<VariableWind>,
}

// ─── Obscuration ─────────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloudCoverage {
    Few,
    Scattered,
    Broken,
    Overcast,
    /// Coverage field was `//` (undefined).
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct CloudLayer {
    pub coverage: CloudCoverage,
    /// Cloud base in feet. `None` when the height field was `///`.
    pub height_ft: Option<u32>,
    pub is_cumulonimbus: bool,
}

// ─── METAR ───────────────────────────────────────────────────────────────────

/// Structured METAR data ready for plugin consumption.
///
/// Always includes `raw` so plugins can do their own parsing if needed.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct MetarData {
    pub raw: String,
    pub icao: String,
    pub wind: Option<WindData>,
    /// Temperature in degrees Celsius.
    pub temp_c: Option<i32>,
    /// Dew point in degrees Celsius.
    pub dew_point_c: Option<i32>,
    /// QNH in hPa.
    pub qnh_hpa: Option<u32>,
    /// Prevailing visibility in metres. `None` for CAVOK or `////`.
    pub visibility_m: Option<u32>,
    pub clouds: Vec<CloudLayer>,
    pub rvr_reported: bool,
    /// Vertical visibility in hundreds of feet, if reported.
    pub vertical_visibility_ft: Option<u32>,
    /// Present weather codes, e.g. `["-SN", "+TSRA", "FG"]`.
    pub present_weather: Vec<String>,
}

// ─── Runway ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct RunwayInfo {
    pub identifier: String,
    pub degrees: u16,
}

/// A physical runway strip with a mandatory primary direction and an optional reciprocal.
///
/// A normal two-way runway has both fields; a one-way or approach-only runway has only `primary`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct PhysicalRunway {
    pub primary: RunwayInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reciprocal: Option<RunwayInfo>,
}

impl PhysicalRunway {
    pub fn single(primary: RunwayInfo) -> Self {
        Self {
            primary,
            reciprocal: None,
        }
    }

    pub fn pair(primary: RunwayInfo, reciprocal: RunwayInfo) -> Self {
        Self {
            primary,
            reciprocal: Some(reciprocal),
        }
    }

    /// Iterate over whichever directions are present (1 or 2).
    pub fn iter(&self) -> impl Iterator<Item = &RunwayInfo> {
        std::iter::once(&self.primary).chain(self.reciprocal.iter())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct AirportInfo {
    pub icao: String,
    pub runways: Vec<PhysicalRunway>,
}

// ─── Selection tags ───────────────────────────────────────────────────────────

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
        SelectionTag {
            id: self.id.to_string(),
            conflict: false,
            symbol: self.symbol.to_string(),
            label: self.label.to_string(),
        }
    }

    /// Returns a [`SelectionTag`] for a negative factor that was *accepted*
    /// against the runway choice (e.g. tailwind accepted due to LVP).
    pub fn conflict(&self) -> SelectionTag {
        SelectionTag {
            id: self.id.to_string(),
            conflict: true,
            symbol: self.symbol.to_string(),
            label: self.label.to_string(),
        }
    }
}

/// A tag attached to a runway selection, transmitted in plugin responses.
///
/// `conflict = false` → the tag *explains* the selection (a reason).
/// `conflict = true`  → the tag marks a *negative factor that was accepted*
/// against the chosen runway (e.g. tailwind, low visibility accepted with
/// a tailwind).  When conflict tags are present alongside reason tags the
/// report highlights the combination as a compromise.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
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

/// Well-known tag constants shared across parent and plugins.
///
/// Plugin-specific tags (e.g. ENGM runway modes) should be defined as
/// `pub const` [`Tag`] values in the plugin's own crate and are rendered
/// as generic neutral pills by any parent that does not know them by id.
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

// ─── RunwayUse ────────────────────────────────────────────────────────────────

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunwayUse {
    Departing,
    Arriving,
    Both,
}

// ─── ATIS plugin protocol: POST /atis ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AtisEntry {
    pub airport_icao: String,
    pub atis_text: String,
    pub information_letter: Option<char>,
}

/// Body sent to the plugin's `POST /atis` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AtisRequest {
    pub atis_entries: Vec<AtisEntry>,
    pub airports: Vec<AirportInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunwayAssignment {
    pub runway_id: String,
    pub runway_use: RunwayUse,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AirportRunwayAssignment {
    pub airport_icao: String,
    pub assignments: Vec<RunwayAssignment>,
    /// Tags explaining and/or qualifying this selection. Empty when none apply.
    #[serde(default)]
    pub tags: Vec<SelectionTag>,
}

/// Body returned from the plugin's `POST /atis` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AtisResponse {
    pub airports: Vec<AirportRunwayAssignment>,
}

// ─── Runway-selection plugin protocol: POST /runways ─────────────────────────

/// Body sent to the plugin's `POST /runways` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunwaySelectionRequest {
    pub airport: AirportInfo,
    pub metar: Option<MetarData>,
}

/// Body returned from the plugin's `POST /runways` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RunwaySelectionResponse {
    pub runways: Vec<RunwayAssignment>,
    /// Tags explaining and/or qualifying this selection. Empty when none apply.
    #[serde(default)]
    pub tags: Vec<SelectionTag>,
}

// ─── Plugin /airports endpoint ────────────────────────────────────────────────

/// Response from the plugin's `GET /airports` endpoint.
///
/// Lists the ICAO codes this plugin claims to handle.
/// Only airports in this list are routed to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginAirportsResponse {
    pub airports: Vec<String>,
}

// ─── Parent helper: POST /parse-atis ─────────────────────────────────────────

/// Request body for the parent's `POST /parse-atis` helper.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ParseAtisRequest {
    pub atis_text: String,
}

/// Response from the parent's `POST /parse-atis` helper.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ParseAtisResponse {
    pub assignments: Vec<RunwayAssignment>,
}

// ─── Parent helper: POST /parse-metar ────────────────────────────────────────

/// Request body for the parent's `POST /parse-metar` helper.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ParseMetarRequest {
    pub raw_metar: String,
}

/// Response from the parent's `POST /parse-metar` helper.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ParseMetarResponse {
    pub metar: Option<MetarData>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runway_iter_size_hint() {
        let runways = PhysicalRunway {
            primary: RunwayInfo {
                identifier: "01L".into(),
                degrees: 12,
            },
            reciprocal: Some(RunwayInfo {
                identifier: "19R".into(),
                degrees: 192,
            }),
        };
        assert_eq!(runways.iter().size_hint(), (2, Some(2)))
    }

    #[test]
    fn test_runway_single_only_iter_size_hint() {
        let runways = PhysicalRunway {
            primary: RunwayInfo {
                identifier: "18".into(),
                degrees: 176,
            },
            reciprocal: None,
        };
        assert_eq!(runways.iter().size_hint(), (1, Some(1)))
    }
}
