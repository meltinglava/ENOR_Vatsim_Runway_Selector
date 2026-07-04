//! Runway-selection logic for the ENOR area.
//!
//! Strategy per airport:
//! 1. Dispatch by ICAO (the host applies ATIS itself; airports already
//!    decided by ATIS never reach the plugin):
//!    - **ENGM** (Oslo Gardermoen): pick a direction from headwind, then
//!      pick Mixed / Segregated / Single ops based on Europe/Oslo local time
//!      and METAR-derived LVP, RVR, low visibility, vertical visibility,
//!      freezing weather, or possible-de-ice conditions.
//!    - **ENZV** (Stavanger): default to 18/36, but if its crosswind ≥ 15 kt
//!      and the perpendicular runway (10/28) has a strictly lower crosswind,
//!      switch to the secondary runway.
//!    - **everything else**: max headwind with a 2-kt margin over the
//!      runner-up to avoid flipping every cycle. If the margin is not met,
//!      report `handled: false` and let the host fall back to area defaults.
//! 2. If no METAR is available, report `handled: false`.
//!
//! Time is always taken from the request (`timestamp_utc`) — never the wall
//! clock — so a given request always produces the same response.
//!
//! LVP trigger note: the trigger set deliberately includes "any RVR group
//! reported", "visibility below 5000 m", and "vertical visibility present"
//! in addition to the classic ceiling/freezing/de-ice conditions. This is
//! the reviewed, unit-tested behaviour carried over from the gRPC-era ENOR
//! selector, kept intentionally (an alternative port omitted these).

use jiff::{Timestamp as JiffTimestamp, Zoned, tz::TimeZone};
use runway_plugin_api::{
    AirportSelectionRequest, AirportSelectionResult, ParsedMetar, RunwayInfo,
    RunwaySelectionsRequest, RunwaySelectionsResponse, RunwayUse, RunwayUseEntry, SelectionSource,
    SelectionTag, Tag, WeatherDescriptor, helpers::best_headwind, tags,
};
use runway_selector_area_config::AreaConfig;
use thiserror::Error;
use tracing::{debug, warn};

const HEADWIND_MARGIN_KT: i32 = 2;
const ENZV_CROSSWIND_SWITCH_KT: i32 = 15;
const ENGM_LVP_CEILING_HUNDREDS_FT: i32 = 15;
const ENGM_LOW_VISIBILITY_METERS: u32 = 5000;
const ENGM_POSSIBLE_DEICE_TEMP_C: i32 = 5;

/// Plugin-specific tags for the ENOR area. Hosts that don't know these ids
/// render them as neutral pills using the shipped symbol/label.
pub const ENGM_MIXED: Tag = Tag {
    id: "engm-mixed",
    symbol: "⇅",
    label: "ENGM mixed-mode parallel operations",
};
pub const ENGM_SEGREGATED: Tag = Tag {
    id: "engm-segregated",
    symbol: "⇈",
    label: "ENGM segregated parallel operations",
};
pub const ENGM_SINGLE: Tag = Tag {
    id: "engm-single",
    symbol: "①",
    label: "ENGM single-runway night operations",
};
pub const ENZV_CROSSWIND_SWITCH: Tag = Tag {
    id: "enzv-crosswind-switch",
    symbol: "⤨",
    label: "ENZV switched to secondary runway due to crosswind",
};

#[derive(Debug, Error)]
pub enum SelectError {
    #[error("Invalid timestamp_utc {value:?}: {message}")]
    BadTimestamp { value: String, message: String },
}

pub struct EnorSelector {
    config: AreaConfig,
}

impl EnorSelector {
    pub fn new(config: AreaConfig) -> Self {
        Self { config }
    }

    /// Handle a full batch request. Never panics on bad input: a malformed
    /// timestamp is reported as an error (the host logs it and falls back),
    /// and airports without usable data come back `handled: false`.
    pub fn select_runways(
        &self,
        request: &RunwaySelectionsRequest,
    ) -> Result<RunwaySelectionsResponse, SelectError> {
        let now_utc: JiffTimestamp =
            request
                .timestamp_utc
                .parse()
                .map_err(|e: jiff::Error| SelectError::BadTimestamp {
                    value: request.timestamp_utc.clone(),
                    message: e.to_string(),
                })?;

        let tz = TimeZone::get(&request.area_timezone).unwrap_or_else(|err| {
            warn!(
                tz = %request.area_timezone,
                error = ?err,
                "Failed to resolve area_timezone; defaulting to UTC"
            );
            TimeZone::UTC
        });
        let local = now_utc.to_zoned(tz);

        debug!(
            airport_count = request.airports.len(),
            %local,
            "runway-selections request"
        );

        let results = request
            .airports
            .iter()
            .map(|a| self.select_for_airport(a, &local))
            .collect();

        Ok(RunwaySelectionsResponse { results })
    }

    fn select_for_airport(
        &self,
        airport: &AirportSelectionRequest,
        local: &Zoned,
    ) -> AirportSelectionResult {
        match airport.icao.as_str() {
            "ENGM" => self.select_for_engm(airport, local),
            "ENZV" => Self::select_for_enzv(airport),
            _ => Self::select_generic(airport),
        }
    }

    fn select_generic(airport: &AirportSelectionRequest) -> AirportSelectionResult {
        if airport.metar.is_none() {
            return not_handled(&airport.icao);
        }
        match pick_best_headwind(&airport.runways) {
            Some(best) => metar_selection(
                &airport.icao,
                vec![entry(&best, RunwayUse::Both)],
                Vec::new(),
            ),
            None => not_handled(&airport.icao),
        }
    }

    /// ENGM: route through Mixed / Segregated / Single ops based on
    /// Europe/Oslo local time and LVP-relevant METAR conditions.
    fn select_for_engm(
        &self,
        airport: &AirportSelectionRequest,
        local: &Zoned,
    ) -> AirportSelectionResult {
        let (direction, source) = self.engm_direction(airport);
        let (mode, lvp_triggered) = engm_mode(parsed_metar(airport), local);

        let mut tags_out: Vec<SelectionTag> = Vec::new();
        let runways = match mode {
            EngmMode::Mixed => {
                tags_out.push(ENGM_MIXED.reason());
                vec![
                    entry(&format!("{direction}L"), RunwayUse::Both),
                    entry(&format!("{direction}R"), RunwayUse::Both),
                ]
            }
            EngmMode::Segregated => {
                tags_out.push(ENGM_SEGREGATED.reason());
                if lvp_triggered {
                    tags_out.push(tags::LVP.reason());
                }
                vec![
                    entry(&format!("{direction}L"), RunwayUse::Departing),
                    entry(&format!("{direction}R"), RunwayUse::Arriving),
                ]
            }
            EngmMode::Single => {
                tags_out.push(ENGM_SINGLE.reason());
                let id = match direction.as_str() {
                    "01" => "01L",
                    "19" => "19R",
                    other => {
                        warn!(
                            airport = %airport.icao,
                            direction = %other,
                            "ENGM single-mode hit an unexpected direction; defaulting to 01L"
                        );
                        "01L"
                    }
                };
                vec![entry(id, RunwayUse::Both)]
            }
        };

        AirportSelectionResult {
            icao: airport.icao.clone(),
            handled: true,
            source,
            runway_uses: runways,
            tags: tags_out,
        }
    }

    /// Returns the ENGM direction prefix ("01" or "19") and the source the
    /// selection should be attributed to (METAR if we picked from wind,
    /// Default if we fell back to area defaults).
    fn engm_direction(&self, airport: &AirportSelectionRequest) -> (String, SelectionSource) {
        if let Some(dir) = pick_best_direction_prefix(&airport.runways) {
            return (dir, SelectionSource::Metar);
        }
        let fallback = self
            .config
            .default_runways
            .get(&airport.icao)
            .map(|n| format!("{n:02}"))
            .unwrap_or_else(|| "01".to_string());
        (fallback, SelectionSource::Default)
    }

    /// ENZV: keep the main 18/36 runway unless its crosswind ≥ 15 kt and the
    /// perpendicular 10/28 runway has a strictly lower crosswind.
    fn select_for_enzv(airport: &AirportSelectionRequest) -> AirportSelectionResult {
        if airport.metar.is_none() {
            return not_handled(&airport.icao);
        }

        let main = pick_best_in_set(&airport.runways, &["18", "36"]).unwrap_or_else(|| "18".into());
        let main_crosswind = crosswind_kt(&airport.runways, &main);

        if main_crosswind < ENZV_CROSSWIND_SWITCH_KT {
            return metar_selection(
                &airport.icao,
                vec![entry(&main, RunwayUse::Both)],
                Vec::new(),
            );
        }

        let secondary_best = pick_best_in_set(&airport.runways, &["10", "28"]);

        match secondary_best {
            Some(sec) if crosswind_kt(&airport.runways, &sec) < main_crosswind => metar_selection(
                &airport.icao,
                vec![entry(&sec, RunwayUse::Both)],
                vec![ENZV_CROSSWIND_SWITCH.reason()],
            ),
            _ => metar_selection(
                &airport.icao,
                vec![entry(&main, RunwayUse::Both)],
                Vec::new(),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum EngmMode {
    Mixed,
    Segregated,
    Single,
}

fn parsed_metar(airport: &AirportSelectionRequest) -> Option<&ParsedMetar> {
    airport.metar.as_ref().and_then(|m| m.parsed.as_ref())
}

fn not_handled(icao: &str) -> AirportSelectionResult {
    AirportSelectionResult {
        icao: icao.to_string(),
        handled: false,
        source: SelectionSource::Metar,
        runway_uses: Vec::new(),
        tags: Vec::new(),
    }
}

fn metar_selection(
    icao: &str,
    runway_uses: Vec<RunwayUseEntry>,
    tags: Vec<SelectionTag>,
) -> AirportSelectionResult {
    AirportSelectionResult {
        icao: icao.to_string(),
        handled: true,
        source: SelectionSource::Metar,
        runway_uses,
        tags,
    }
}

fn entry(identifier: &str, use_: RunwayUse) -> RunwayUseEntry {
    RunwayUseEntry {
        runway: identifier.to_string(),
        use_,
    }
}

/// Pick the runway identifier with the strictly highest headwind, provided it
/// beats the next-best by at least [`HEADWIND_MARGIN_KT`]. Ambiguous winds
/// return `None` so the host falls back to defaults.
///
/// Delegates to the shared [`best_headwind`] helper; that helper's threshold
/// is exclusive (`advantage > threshold`), so "advantage ≥ margin" becomes
/// `threshold = margin − 1`.
fn pick_best_headwind(runways: &[RunwayInfo]) -> Option<String> {
    best_headwind(runways, HEADWIND_MARGIN_KT - 1).map(|r| r.identifier.clone())
}

/// Group runways by direction prefix (first 2 chars of identifier) and pick
/// the prefix whose best runway beats every other prefix's best by at least
/// [`HEADWIND_MARGIN_KT`]. Parallel runways (01L/01R) sharing a prefix don't
/// fight each other.
fn pick_best_direction_prefix(runways: &[RunwayInfo]) -> Option<String> {
    use std::collections::HashMap;
    let mut by_prefix: HashMap<String, i32> = HashMap::new();
    for r in runways {
        if r.identifier.len() < 2 {
            continue;
        }
        let Some(headwind) = r.headwind_kt else {
            continue;
        };
        let prefix = r.identifier[..2].to_string();
        by_prefix
            .entry(prefix)
            .and_modify(|hw| {
                if headwind > *hw {
                    *hw = headwind;
                }
            })
            .or_insert(headwind);
    }

    let mut scored: Vec<(i32, String)> = by_prefix.into_iter().map(|(k, v)| (v, k)).collect();
    scored.sort_by_key(|scored_entry| std::cmp::Reverse(scored_entry.0));

    let (top_score, top_prefix) = scored.first().cloned()?;
    let beats_runner_up = match scored.get(1) {
        Some((second, _)) => top_score.saturating_sub(*second) >= HEADWIND_MARGIN_KT,
        None => true,
    };
    beats_runner_up.then_some(top_prefix)
}

/// Same as [`pick_best_headwind`] but restricted to runways whose first two
/// characters match one of `prefixes`. Useful for picking a direction within a
/// physical-runway pair (e.g. main 18/36 vs secondary 10/28 at ENZV).
fn pick_best_in_set(runways: &[RunwayInfo], prefixes: &[&str]) -> Option<String> {
    let subset: Vec<RunwayInfo> = runways
        .iter()
        .filter(|r| r.identifier.len() >= 2 && prefixes.contains(&&r.identifier[..2]))
        .cloned()
        .collect();
    pick_best_headwind(&subset)
}

fn crosswind_kt(runways: &[RunwayInfo], identifier: &str) -> i32 {
    runways
        .iter()
        .find(|r| r.identifier == identifier)
        .and_then(|r| r.crosswind_kt)
        .unwrap_or(0)
}

/// Returns the operating mode and whether weather (rather than time of day)
/// forced segregated ops — the latter drives the LVP tag.
fn engm_mode(metar: Option<&ParsedMetar>, local: &Zoned) -> (EngmMode, bool) {
    let date = local.date();
    let segregated_after = date
        .at(22, 30, 0, 0)
        .to_zoned(local.time_zone().clone())
        .ok();
    let single_before = date
        .at(6, 30, 0, 0)
        .to_zoned(local.time_zone().clone())
        .ok();

    if let Some(t) = segregated_after.as_ref()
        && local >= t
    {
        return (EngmMode::Segregated, false);
    }
    if let Some(t) = single_before.as_ref()
        && local < t
    {
        return (EngmMode::Single, false);
    }

    if engm_low_visibility_procedures_apply(metar) {
        (EngmMode::Segregated, true)
    } else {
        (EngmMode::Mixed, false)
    }
}

/// LVP-style triggers that flip ENGM into Segregated mode during the day:
/// cloud ceiling below 1500 ft, any RVR group reported, visibility below
/// 5000 m, vertical visibility present, freezing weather, or possible-de-ice
/// precipitation with temperature below 5 °C (or temperature unknown).
fn engm_low_visibility_procedures_apply(metar: Option<&ParsedMetar>) -> bool {
    let Some(m) = metar else { return false };
    if m.is_cavok {
        return false;
    }

    if has_ceiling_below_1500(m) {
        return true;
    }
    if !m.rvr.is_empty() {
        return true;
    }
    // Visibility unknown ("////") is treated as below the threshold
    // (worst case), hence unwrap_or(0).
    if m.visibility_meters.unwrap_or(0) < ENGM_LOW_VISIBILITY_METERS {
        return true;
    }
    if m.vertical_visibility_hundreds_ft.is_some() {
        return true;
    }
    if has_freezing(m) {
        return true;
    }
    if possible_deice(m) {
        return true;
    }

    false
}

fn has_ceiling_below_1500(m: &ParsedMetar) -> bool {
    use runway_plugin_api::CloudCoverage::{Broken, Overcast};
    m.clouds.iter().any(|cloud| {
        // coverage None → "///"; treat as broken/overcast (worst-case)
        let coverage_qualifies = cloud
            .coverage
            .map(|c| matches!(c, Broken | Overcast))
            .unwrap_or(true);
        if !coverage_qualifies {
            return false;
        }
        // height None → "///"; treat as below 1500 ft
        cloud
            .height_hundreds_ft
            .map(|h| h < ENGM_LVP_CEILING_HUNDREDS_FT)
            .unwrap_or(true)
    })
}

fn has_freezing(m: &ParsedMetar) -> bool {
    m.weather_phenomena
        .iter()
        .any(|pw| pw.descriptors.contains(&WeatherDescriptor::Freezing))
}

fn possible_deice(m: &ParsedMetar) -> bool {
    const QUALIFYING: [&str; 10] = ["DZ", "RA", "SN", "SG", "PL", "GR", "GS", "UP", "BR", "FG"];

    let contender = m.weather_phenomena.iter().any(|pw| {
        pw.phenomena
            .iter()
            .any(|code| QUALIFYING.contains(&code.as_str()))
    });
    if !contender {
        return false;
    }

    // Temperature unknown → treat as cold enough (worst case for de-icing).
    match m.temperature_c {
        Some(c) => c < ENGM_POSSIBLE_DEICE_TEMP_C,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runway_plugin_api::{
        CloudCoverage, CloudData, CrosswindDirection, MetarData, RvrData, WeatherIntensity,
        WeatherPhenomenonData,
    };

    fn enor_selector() -> EnorSelector {
        let config = AreaConfig {
            default_runways: indexmap::IndexMap::from([
                ("ENGM".to_string(), 1u8),
                ("ENZV".to_string(), 18u8),
            ]),
            time_zone: Some("Europe/Oslo".to_string()),
            ..AreaConfig::default()
        };
        EnorSelector::new(config)
    }

    fn runway(identifier: &str, headwind: i32, crosswind: i32) -> RunwayInfo {
        RunwayInfo {
            identifier: identifier.into(),
            heading: 0,
            headwind_kt: Some(headwind),
            tailwind_kt: Some((-headwind).max(0)),
            crosswind_kt: Some(crosswind),
            crosswind_direction: Some(CrosswindDirection::Left),
        }
    }

    fn empty_parsed() -> ParsedMetar {
        ParsedMetar {
            is_cavok: true,
            wind: None,
            visibility_meters: None,
            rvr: vec![],
            clouds: vec![],
            vertical_visibility_hundreds_ft: None,
            weather_phenomena: vec![],
            temperature_c: Some(10),
            dew_point_c: Some(5),
            qnh_hpa: Some(1013),
        }
    }

    fn empty_metar(icao: &str) -> MetarData {
        MetarData {
            raw: format!("{icao} TEST"),
            parsed: Some(empty_parsed()),
        }
    }

    fn described(parsed: ParsedMetar) -> MetarData {
        MetarData {
            raw: "TEST".into(),
            parsed: Some(ParsedMetar {
                is_cavok: false,
                visibility_meters: Some(9999),
                ..parsed
            }),
        }
    }

    fn oslo_zoned(local_h: i8, local_m: i8) -> Zoned {
        let oslo = TimeZone::get("Europe/Oslo").unwrap();
        jiff::civil::date(2026, 5, 31)
            .at(local_h, local_m, 0, 0)
            .to_zoned(oslo)
            .unwrap()
    }

    fn select(
        sel: &EnorSelector,
        airport: AirportSelectionRequest,
        local: &Zoned,
    ) -> AirportSelectionResult {
        sel.select_for_airport(&airport, local)
    }

    fn ids(result: &AirportSelectionResult) -> Vec<&str> {
        result
            .runway_uses
            .iter()
            .map(|r| r.runway.as_str())
            .collect()
    }

    fn uses(result: &AirportSelectionResult) -> Vec<RunwayUse> {
        result.runway_uses.iter().map(|r| r.use_).collect()
    }

    #[test]
    fn picks_highest_headwind_when_margin_exceeded() {
        let runways = vec![runway("01", 10, 0), runway("19", 5, 0)];
        assert_eq!(pick_best_headwind(&runways), Some("01".to_string()));
    }

    #[test]
    fn returns_none_when_top_two_are_within_margin() {
        let runways = vec![runway("01", 10, 0), runway("19", 9, 0)];
        assert!(pick_best_headwind(&runways).is_none());
    }

    #[test]
    fn generic_select_defers_without_metar() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENBR".into(),
            runways: vec![runway("17", 8, 0), runway("35", 0, 0)],
            metar: None,
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(!out.handled);
        assert!(out.runway_uses.is_empty());
    }

    #[test]
    fn generic_select_defers_when_wind_is_ambiguous() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENBR".into(),
            runways: vec![runway("17", 5, 0), runway("35", 4, 0)],
            metar: Some(empty_metar("ENBR")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(!out.handled);
    }

    #[test]
    fn generic_select_picks_headwind_winner() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENBR".into(),
            runways: vec![runway("17", 12, 0), runway("35", -12, 0)],
            metar: Some(empty_metar("ENBR")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(out.handled);
        assert_eq!(out.source, SelectionSource::Metar);
        assert_eq!(ids(&out), vec!["17"]);
    }

    #[test]
    fn engm_mixed_during_daytime_calm_weather() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));

        assert!(out.handled);
        assert_eq!(out.source, SelectionSource::Metar);
        assert_eq!(ids(&out), vec!["01L", "01R"]);
        assert!(uses(&out).iter().all(|u| *u == RunwayUse::Both));
        assert!(out.tags.iter().any(|t| t.id == ENGM_MIXED.id));
    }

    #[test]
    fn engm_segregated_after_2230_local() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
        };
        let out = select(&sel, airport, &oslo_zoned(23, 0));

        assert_eq!(uses(&out), vec![RunwayUse::Departing, RunwayUse::Arriving]);
        // Night segregation is time-driven — no LVP tag.
        assert!(!out.tags.iter().any(|t| t.id == tags::LVP.id));
    }

    #[test]
    fn engm_single_before_0630_local() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
        };
        let out = select(&sel, airport, &oslo_zoned(5, 0));

        assert_eq!(ids(&out), vec!["01L"]);
        assert!(out.tags.iter().any(|t| t.id == ENGM_SINGLE.id));
    }

    #[test]
    fn engm_falls_back_to_default_direction_without_wind() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![
                RunwayInfo {
                    identifier: "01L".into(),
                    heading: 7,
                    headwind_kt: None,
                    tailwind_kt: None,
                    crosswind_kt: None,
                    crosswind_direction: None,
                },
                RunwayInfo {
                    identifier: "01R".into(),
                    heading: 7,
                    headwind_kt: None,
                    tailwind_kt: None,
                    crosswind_kt: None,
                    crosswind_direction: None,
                },
            ],
            metar: Some(empty_metar("ENGM")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(out.handled);
        assert_eq!(out.source, SelectionSource::Default);
        assert_eq!(ids(&out), vec!["01L", "01R"]);
    }

    #[test]
    fn engm_segregated_during_day_when_vertical_visibility_reported() {
        let sel = enor_selector();
        let metar = described(ParsedMetar {
            vertical_visibility_hundreds_ft: Some(3),
            ..empty_parsed()
        });
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(metar),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert_eq!(uses(&out), vec![RunwayUse::Departing, RunwayUse::Arriving]);
        assert!(out.tags.iter().any(|t| t.id == tags::LVP.id));
    }

    #[test]
    fn engm_segregated_during_day_when_low_visibility_reported() {
        let sel = enor_selector();
        let mut metar = described(empty_parsed());
        metar.parsed.as_mut().unwrap().visibility_meters = Some(3000);
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(metar),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(uses(&out).contains(&RunwayUse::Departing));
    }

    #[test]
    fn engm_segregated_during_day_when_freezing_drizzle_reported() {
        let sel = enor_selector();
        let metar = described(ParsedMetar {
            weather_phenomena: vec![WeatherPhenomenonData {
                intensity: Some(WeatherIntensity::Light),
                descriptors: vec![WeatherDescriptor::Freezing],
                phenomena: vec!["DZ".into()],
            }],
            ..empty_parsed()
        });
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(metar),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        let u = uses(&out);
        assert!(u.contains(&RunwayUse::Departing));
        assert!(u.contains(&RunwayUse::Arriving));
    }

    #[test]
    fn engm_segregated_during_day_when_rvr_reported() {
        let sel = enor_selector();
        let metar = described(ParsedMetar {
            rvr: vec![RvrData {
                runway: "01L".into(),
                meters: Some(800),
            }],
            ..empty_parsed()
        });
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(metar),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert_eq!(uses(&out), vec![RunwayUse::Departing, RunwayUse::Arriving]);
    }

    #[test]
    fn engm_mixed_when_low_clouds_are_few_or_scattered() {
        let sel = enor_selector();
        let metar = described(ParsedMetar {
            clouds: vec![CloudData {
                coverage: Some(CloudCoverage::Few),
                height_hundreds_ft: Some(5),
                cloud_type: None,
            }],
            ..empty_parsed()
        });
        let airport = AirportSelectionRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(metar),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert!(uses(&out).iter().all(|u| *u == RunwayUse::Both));
    }

    #[test]
    fn engm_possible_deice_needs_cold_temperature() {
        let rain = |temp: Option<i32>| {
            described(ParsedMetar {
                weather_phenomena: vec![WeatherPhenomenonData {
                    intensity: None,
                    descriptors: vec![],
                    phenomena: vec!["RA".into()],
                }],
                temperature_c: temp,
                ..empty_parsed()
            })
        };
        let lvp = |m: &MetarData| engm_low_visibility_procedures_apply(m.parsed.as_ref());
        assert!(lvp(&rain(Some(2))), "rain at 2°C can require de-icing");
        assert!(!lvp(&rain(Some(10))), "warm rain never needs de-icing");
        assert!(lvp(&rain(None)), "unknown temperature is worst-cased");
    }

    #[test]
    fn enzv_keeps_main_runway_when_crosswind_is_under_15kt() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", 10, 5),
                runway("36", -10, 5),
                runway("10", 2, 8),
                runway("28", -2, 8),
            ],
            metar: Some(empty_metar("ENZV")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert_eq!(ids(&out), vec!["18"]);
        assert_eq!(out.source, SelectionSource::Metar);
        assert!(out.tags.is_empty());
    }

    #[test]
    fn enzv_switches_to_secondary_when_crosswind_high_and_secondary_lower() {
        let sel = enor_selector();
        // 18 crosswind 20 kt, 28 crosswind 5 kt → must switch to 28.
        let airport = AirportSelectionRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", -5, 20),
                runway("36", 5, 20),
                runway("10", -10, 5),
                runway("28", 10, 5),
            ],
            metar: Some(empty_metar("ENZV")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert_eq!(ids(&out), vec!["28"]);
        assert!(out.tags.iter().any(|t| t.id == ENZV_CROSSWIND_SWITCH.id));
    }

    #[test]
    fn enzv_keeps_main_when_secondary_crosswind_is_not_strictly_lower() {
        let sel = enor_selector();
        let airport = AirportSelectionRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", -5, 20),
                runway("36", 5, 20),
                runway("10", -10, 20),
                runway("28", 10, 20),
            ],
            metar: Some(empty_metar("ENZV")),
        };
        let out = select(&sel, airport, &oslo_zoned(12, 0));
        assert_eq!(ids(&out), vec!["36"]);
    }

    #[test]
    fn select_runways_uses_request_timestamp_not_wall_clock() {
        let sel = enor_selector();
        // 23:00 Oslo local on 2026-05-31 is 21:00 UTC (CEST).
        let request = RunwaySelectionsRequest {
            timestamp_utc: "2026-05-31T21:00:00Z".into(),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![AirportSelectionRequest {
                icao: "ENGM".into(),
                runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
                metar: Some(empty_metar("ENGM")),
            }],
        };
        let resp = sel.select_runways(&request).unwrap();
        assert_eq!(resp.results.len(), 1);
        // 23:00 local → night segregated mode, regardless of the wall clock.
        assert_eq!(
            uses(&resp.results[0]),
            vec![RunwayUse::Departing, RunwayUse::Arriving]
        );
    }

    #[test]
    fn select_runways_rejects_bad_timestamp_without_panicking() {
        let sel = enor_selector();
        let request = RunwaySelectionsRequest {
            timestamp_utc: "not-a-timestamp".into(),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![],
        };
        let err = sel.select_runways(&request).unwrap_err();
        assert!(matches!(err, SelectError::BadTimestamp { .. }));
    }

    #[test]
    fn select_runways_returns_per_airport_results() {
        let sel = enor_selector();
        let request = RunwaySelectionsRequest {
            timestamp_utc: "2026-05-31T10:00:00Z".into(),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![AirportSelectionRequest {
                icao: "ENBR".into(),
                runways: vec![runway("17", 12, 0), runway("35", -12, 0)],
                metar: Some(empty_metar("ENBR")),
            }],
        };
        let resp = sel.select_runways(&request).unwrap();
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].runway_uses[0].runway, "17");
    }
}
