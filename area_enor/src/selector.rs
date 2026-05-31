//! Runway-selection logic for the ENOR area.
//!
//! Strategy per airport:
//! 1. If the request carries ATIS-derived runways, pass them through
//!    (`SOURCE = ATIS`) — controller authority overrides our math.
//! 2. Otherwise dispatch by ICAO:
//!    - **ENGM** (Oslo Gardermoen): pick a direction from headwind, then
//!      pick Mixed / Segregated / Single ops based on Europe/Oslo local time
//!      and METAR-derived LVP, RVR, low visibility, vertical visibility,
//!      freezing weather, or possible-de-ice conditions.
//!    - **ENZV** (Stavanger): default to 18/36, but if its crosswind ≥ 15 kt
//!      and the perpendicular runway (10/28) has a strictly lower crosswind,
//!      switch to the secondary runway.
//!    - **everything else**: max headwind with a 2-kt margin over the
//!      runner-up to avoid flipping every cycle. If the margin is not met,
//!      emit nothing and let the host fall back to area defaults.
//! 3. If no METAR is available, emit nothing.

use std::cmp::Reverse;

use jiff::{Timestamp as JiffTimestamp, Zoned, tz::TimeZone};
use runway_selector_area_config::AreaConfig;
use runway_selector_protocol::v1::{
    AirportRequest, AirportSelection, GetAirportsResponse, Metar, RunwayAssignment, RunwayInfo,
    RunwayUse, SelectRunwaysRequest, SelectRunwaysResponse, SelectionSource, WeatherDescriptor,
    WeatherPhenomenon, obscuration::Variant as ObscurationVariant,
    runway_selector_server::RunwaySelector, visibility::Value as VisibilityValue,
};
use tonic::{Request, Response, Status};
use tracing::{debug, warn};

const HEADWIND_MARGIN_KT: i32 = 2;
const ENZV_CROSSWIND_SWITCH_KT: u32 = 15;
const ENGM_LVP_CEILING_HUNDREDS_FT: i32 = 15;
const ENGM_LOW_VISIBILITY_METERS: u32 = 5000;
const ENGM_POSSIBLE_DEICE_TEMP_C: i32 = 5;

pub struct EnorSelector {
    config: AreaConfig,
}

impl EnorSelector {
    pub fn new(config: AreaConfig) -> Self {
        Self { config }
    }

    fn supported_icaos(&self) -> Vec<String> {
        self.config.default_runways.keys().cloned().collect()
    }

    fn select_for_airport(
        &self,
        airport: &AirportRequest,
        now_utc: Option<&prost_types::Timestamp>,
        area_tz: &TimeZone,
    ) -> Option<AirportSelection> {
        if !airport.atis_runways.is_empty() {
            return Some(AirportSelection {
                icao: airport.icao.clone(),
                source: SelectionSource::Atis as i32,
                runways: airport.atis_runways.clone(),
            });
        }

        match airport.icao.as_str() {
            "ENGM" => self.select_for_engm(airport, now_utc, area_tz),
            "ENZV" => Self::select_for_enzv(airport),
            _ => Self::select_generic(airport),
        }
    }

    fn select_generic(airport: &AirportRequest) -> Option<AirportSelection> {
        airport.metar.as_ref()?;
        let best = pick_best_headwind(&airport.runways)?;
        Some(metar_selection(
            &airport.icao,
            vec![assignment(&best, RunwayUse::Both)],
        ))
    }

    /// ENGM: route through Mixed / Segregated / Single ops based on
    /// Europe/Oslo local time and LVP-relevant METAR conditions.
    fn select_for_engm(
        &self,
        airport: &AirportRequest,
        now_utc: Option<&prost_types::Timestamp>,
        area_tz: &TimeZone,
    ) -> Option<AirportSelection> {
        let (direction, source) = self.engm_direction(airport);
        let mode = self.engm_mode(airport, now_utc, area_tz);

        let runways = match mode {
            EngmMode::Mixed => vec![
                assignment(&format!("{direction}L"), RunwayUse::Both),
                assignment(&format!("{direction}R"), RunwayUse::Both),
            ],
            EngmMode::Segregated => vec![
                assignment(&format!("{direction}L"), RunwayUse::Departing),
                assignment(&format!("{direction}R"), RunwayUse::Arriving),
            ],
            EngmMode::Single => {
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
                vec![assignment(id, RunwayUse::Both)]
            }
        };

        Some(AirportSelection {
            icao: airport.icao.clone(),
            source: source as i32,
            runways,
        })
    }

    /// Returns the ENGM direction prefix ("01" or "19") and the source the
    /// selection should be attributed to (METAR if we picked from wind, Default
    /// if we fell back to area defaults).
    fn engm_direction(&self, airport: &AirportRequest) -> (String, SelectionSource) {
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

    fn engm_mode(
        &self,
        airport: &AirportRequest,
        now_utc: Option<&prost_types::Timestamp>,
        area_tz: &TimeZone,
    ) -> EngmMode {
        let local = now_in_tz(now_utc, area_tz);
        let date = local.date();
        let segregated_after = date.at(22, 30, 0, 0).to_zoned(area_tz.clone()).ok();
        let single_before = date.at(6, 30, 0, 0).to_zoned(area_tz.clone()).ok();

        if let Some(t) = segregated_after.as_ref()
            && local >= *t
        {
            return EngmMode::Segregated;
        }
        if let Some(t) = single_before.as_ref()
            && local < *t
        {
            return EngmMode::Single;
        }

        if engm_low_visibility_procedures_apply(airport.metar.as_ref()) {
            EngmMode::Segregated
        } else {
            EngmMode::Mixed
        }
    }

    /// ENZV: keep the main 18/36 runway unless its crosswind ≥ 15 kt and the
    /// perpendicular 10/28 runway has a strictly lower crosswind.
    fn select_for_enzv(airport: &AirportRequest) -> Option<AirportSelection> {
        airport.metar.as_ref()?;

        let main = pick_best_in_set(&airport.runways, &["18", "36"]).unwrap_or_else(|| "18".into());
        let main_crosswind = crosswind_kt(&airport.runways, &main);

        if main_crosswind < ENZV_CROSSWIND_SWITCH_KT {
            return Some(metar_selection(
                &airport.icao,
                vec![assignment(&main, RunwayUse::Both)],
            ));
        }

        let Some(secondary) = pick_best_in_set(&airport.runways, &["10", "28"]) else {
            return Some(metar_selection(
                &airport.icao,
                vec![assignment(&main, RunwayUse::Both)],
            ));
        };
        let secondary_crosswind = crosswind_kt(&airport.runways, &secondary);

        if secondary_crosswind < main_crosswind {
            Some(metar_selection(
                &airport.icao,
                vec![assignment(&secondary, RunwayUse::Both)],
            ))
        } else {
            Some(metar_selection(
                &airport.icao,
                vec![assignment(&main, RunwayUse::Both)],
            ))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum EngmMode {
    Mixed,
    Segregated,
    Single,
}

fn metar_selection(icao: &str, runways: Vec<RunwayAssignment>) -> AirportSelection {
    AirportSelection {
        icao: icao.to_string(),
        source: SelectionSource::Metar as i32,
        runways,
    }
}

fn assignment(identifier: &str, r#use: RunwayUse) -> RunwayAssignment {
    RunwayAssignment {
        identifier: identifier.to_string(),
        r#use: r#use as i32,
    }
}

/// Pick the runway identifier with the strictly highest headwind, provided it
/// beats the next-best by at least [`HEADWIND_MARGIN_KT`]. Ambiguous winds
/// return `None` so the host falls back to defaults.
fn pick_best_headwind(runways: &[RunwayInfo]) -> Option<String> {
    let mut scored: Vec<(i32, &str)> = runways
        .iter()
        .filter_map(|r| {
            r.wind_components
                .as_ref()
                .map(|wc| (wc.headwind_kt, r.identifier.as_str()))
        })
        .collect();
    scored.sort_by_key(|entry| Reverse(entry.0));

    let (top_score, top_id) = scored.first().copied()?;
    let beats_runner_up = match scored.get(1) {
        Some((second, _)) => top_score.saturating_sub(*second) >= HEADWIND_MARGIN_KT,
        None => true,
    };
    beats_runner_up.then(|| top_id.to_string())
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
        let Some(wc) = r.wind_components.as_ref() else {
            continue;
        };
        let prefix = r.identifier[..2].to_string();
        by_prefix
            .entry(prefix)
            .and_modify(|hw| {
                if wc.headwind_kt > *hw {
                    *hw = wc.headwind_kt;
                }
            })
            .or_insert(wc.headwind_kt);
    }

    let mut scored: Vec<(i32, String)> = by_prefix.into_iter().map(|(k, v)| (v, k)).collect();
    scored.sort_by_key(|entry| Reverse(entry.0));

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

fn crosswind_kt(runways: &[RunwayInfo], identifier: &str) -> u32 {
    runways
        .iter()
        .find(|r| r.identifier == identifier)
        .and_then(|r| r.wind_components.as_ref())
        .map(|wc| wc.crosswind_kt)
        .unwrap_or(0)
}

fn now_in_tz(now_utc: Option<&prost_types::Timestamp>, tz: &TimeZone) -> Zoned {
    let utc = now_utc
        .and_then(|ts| JiffTimestamp::new(ts.seconds, ts.nanos).ok())
        .unwrap_or_else(JiffTimestamp::now);
    utc.to_zoned(tz.clone())
}

/// LVP-style triggers that flip ENGM into Segregated mode during the day:
/// cloud ceiling below 1500 ft, any RVR group reported, visibility below
/// 5000 m, vertical visibility present, freezing weather, or possible-de-ice
/// precipitation with temperature below 5 °C (or temperature unknown).
fn engm_low_visibility_procedures_apply(metar: Option<&Metar>) -> bool {
    let Some(metar) = metar else { return false };
    let Some(obs) = metar.obscuration.as_ref() else {
        return false;
    };
    let Some(ObscurationVariant::Described(desc)) = obs.variant.as_ref() else {
        return false;
    };

    if has_ceiling_below_1500(desc.clouds.iter()) {
        return true;
    }
    if !desc.rvr.is_empty() {
        return true;
    }
    if visibility_below(&desc.visibility, ENGM_LOW_VISIBILITY_METERS) {
        return true;
    }
    if desc.vertical_visibility.is_some() {
        return true;
    }
    if has_freezing(&desc.present_weather) {
        return true;
    }
    if possible_deice(&desc.present_weather, metar.temperature.as_ref()) {
        return true;
    }

    false
}

fn has_ceiling_below_1500<'a>(
    clouds: impl IntoIterator<Item = &'a runway_selector_protocol::v1::Cloud>,
) -> bool {
    use runway_selector_protocol::v1::CloudCoverage;
    let ceiling_coverages = [CloudCoverage::Broken as i32, CloudCoverage::Overcast as i32];
    clouds.into_iter().any(|cloud| {
        let Some(variant) = cloud.variant.as_ref() else {
            return false;
        };
        let data = match variant {
            runway_selector_protocol::v1::cloud::Variant::Data(d) => d,
            _ => return false,
        };
        // coverage None → "///"; treat as broken/overcast (worst-case)
        let coverage_qualifies = data
            .coverage
            .map(|c| ceiling_coverages.contains(&c))
            .unwrap_or(true);
        if !coverage_qualifies {
            return false;
        }
        // height None → "///"; treat as below 1500 ft
        data.height_hundreds_ft
            .map(|h| h < ENGM_LVP_CEILING_HUNDREDS_FT)
            .unwrap_or(true)
    })
}

fn visibility_below(
    visibility: &Option<runway_selector_protocol::v1::Visibility>,
    threshold_m: u32,
) -> bool {
    let Some(v) = visibility.as_ref() else {
        return false;
    };
    let Some(VisibilityValue::Meters(meters)) = v.value.as_ref() else {
        return false;
    };
    // None → "////"; treat as below threshold (worst-case)
    meters.value.map(|m| m < threshold_m).unwrap_or(true)
}

fn has_freezing(present_weather: &[runway_selector_protocol::v1::PresentWeather]) -> bool {
    let freezing = WeatherDescriptor::Freezing as i32;
    present_weather
        .iter()
        .any(|pw| pw.descriptor == Some(freezing))
}

fn possible_deice(
    present_weather: &[runway_selector_protocol::v1::PresentWeather],
    temperature: Option<&runway_selector_protocol::v1::Temperature>,
) -> bool {
    let qualifying = |code: i32| {
        let phenomenon = match WeatherPhenomenon::try_from(code) {
            Ok(p) => p,
            Err(_) => return false,
        };
        use WeatherPhenomenon::*;
        matches!(phenomenon, Dz | Ra | Sn | Sg | Pl | Gr | Gs | Up | Br | Fg)
    };

    let contender = present_weather
        .iter()
        .any(|pw| pw.phenomena.iter().filter_map(|p| p.code).any(qualifying));
    if !contender {
        return false;
    }

    // Temperature unknown → treat as cold enough (worst case for de-icing).
    match temperature.and_then(|t| t.celsius) {
        Some(c) => c < ENGM_POSSIBLE_DEICE_TEMP_C,
        None => true,
    }
}

#[tonic::async_trait]
impl RunwaySelector for EnorSelector {
    async fn get_airports(
        &self,
        _request: Request<()>,
    ) -> Result<Response<GetAirportsResponse>, Status> {
        Ok(Response::new(GetAirportsResponse {
            icaos: self.supported_icaos(),
        }))
    }

    async fn select_runways(
        &self,
        request: Request<SelectRunwaysRequest>,
    ) -> Result<Response<SelectRunwaysResponse>, Status> {
        let req = request.into_inner();
        debug!(
            airport_count = req.airports.len(),
            tz = %req.area_timezone,
            "SelectRunways"
        );

        let tz = TimeZone::get(&req.area_timezone).unwrap_or_else(|err| {
            warn!(
                tz = %req.area_timezone,
                error = ?err,
                "Failed to resolve area_timezone; defaulting to UTC"
            );
            TimeZone::UTC
        });

        let selections = req
            .airports
            .iter()
            .filter_map(|a| self.select_for_airport(a, req.now_utc.as_ref(), &tz))
            .collect();

        Ok(Response::new(SelectRunwaysResponse { selections }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runway_selector_protocol::v1::{
        Cloud, CloudCoverage, CloudData, CrosswindDirection, DescribedObscuration, Metar,
        OptionalMeters, PresentWeather, Pressure, Rvr, Temperature, VelocityUnit,
        VerticalVisibility, Visibility, WeatherIntensity, WeatherPhenomenonValue, Wind,
        WindComponents, obscuration::Variant as ObsVariant, visibility::Value as VisValue,
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

    fn runway(identifier: &str, headwind: i32, crosswind: u32) -> RunwayInfo {
        RunwayInfo {
            identifier: identifier.into(),
            heading_degrees_true: 0,
            wind_components: Some(WindComponents {
                headwind_kt: headwind,
                crosswind_kt: crosswind,
                crosswind_direction: CrosswindDirection::Left as i32,
            }),
        }
    }

    fn empty_metar(icao: &str) -> Metar {
        Metar {
            raw: format!("{icao} TEST"),
            icao: icao.into(),
            observation_time: None,
            corrected: false,
            auto: false,
            nosig: false,
            wind: Some(Wind {
                direction: None,
                speed: Some(runway_selector_protocol::v1::OptionalVelocity {
                    value: Some(0),
                    unit: VelocityUnit::Knots as i32,
                }),
                gust: None,
                variation: None,
            }),
            obscuration: Some(runway_selector_protocol::v1::Obscuration {
                variant: Some(ObsVariant::Cavok(())),
            }),
            temperature: Some(Temperature {
                celsius: Some(10),
                dew_point_celsius: Some(5),
            }),
            pressure: Some(Pressure {
                qnh: None,
                altimeter: None,
            }),
            recent_weather: vec![],
            remarks: None,
        }
    }

    fn timestamp_at(local_h: u8, local_m: u8) -> prost_types::Timestamp {
        // Build a "noon local Oslo" anchor and shift to local_h:local_m.
        let oslo = TimeZone::get("Europe/Oslo").unwrap();
        let local = jiff::civil::date(2026, 5, 31)
            .at(local_h as i8, local_m as i8, 0, 0)
            .to_zoned(oslo)
            .unwrap();
        let utc = local.timestamp();
        prost_types::Timestamp {
            seconds: utc.as_second(),
            nanos: utc.subsec_nanosecond(),
        }
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
    fn generic_select_passes_atis_through() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENBR".into(),
            runways: vec![],
            metar: None,
            atis_runways: vec![assignment("17", RunwayUse::Departing)],
        };
        let out = sel
            .select_for_airport(&airport, None, &TimeZone::UTC)
            .unwrap();
        assert_eq!(out.source, SelectionSource::Atis as i32);
        assert_eq!(out.runways.len(), 1);
        assert_eq!(out.runways[0].identifier, "17");
    }

    #[test]
    fn generic_select_returns_none_without_metar() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENBR".into(),
            runways: vec![runway("17", 8, 0), runway("35", 0, 0)],
            metar: None,
            atis_runways: vec![],
        };
        assert!(
            sel.select_for_airport(&airport, None, &TimeZone::UTC)
                .is_none()
        );
    }

    #[test]
    fn engm_mixed_during_daytime_calm_weather() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();

        assert_eq!(out.source, SelectionSource::Metar as i32);
        let ids: Vec<&str> = out.runways.iter().map(|r| r.identifier.as_str()).collect();
        assert_eq!(ids, vec!["01L", "01R"]);
        assert!(
            out.runways
                .iter()
                .all(|r| r.r#use == RunwayUse::Both as i32)
        );
    }

    #[test]
    fn engm_segregated_after_2230_local() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(23, 0)), &tz)
            .unwrap();

        let uses: Vec<i32> = out.runways.iter().map(|r| r.r#use).collect();
        assert_eq!(
            uses,
            vec![RunwayUse::Departing as i32, RunwayUse::Arriving as i32]
        );
    }

    #[test]
    fn engm_single_before_0630_local() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![
                runway("01L", 10, 0),
                runway("01R", 10, 0),
                runway("19L", -10, 0),
                runway("19R", -10, 0),
            ],
            metar: Some(empty_metar("ENGM")),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(5, 0)), &tz)
            .unwrap();

        assert_eq!(out.runways.len(), 1);
        assert_eq!(out.runways[0].identifier, "01L");
    }

    #[test]
    fn engm_segregated_during_day_when_vertical_visibility_reported() {
        let sel = enor_selector();
        let mut m = empty_metar("ENGM");
        m.obscuration = Some(runway_selector_protocol::v1::Obscuration {
            variant: Some(ObsVariant::Described(DescribedObscuration {
                visibility: Some(Visibility {
                    value: Some(VisValue::Meters(OptionalMeters { value: Some(9999) })),
                    direction: None,
                    no_directional_variation: false,
                }),
                directional_visibility: vec![],
                rvr: vec![],
                present_weather: vec![],
                clouds: vec![],
                vertical_visibility: Some(VerticalVisibility {
                    hundreds_of_feet: Some(3),
                }),
            })),
        });
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(m),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();
        let uses: Vec<i32> = out.runways.iter().map(|r| r.r#use).collect();
        assert_eq!(
            uses,
            vec![RunwayUse::Departing as i32, RunwayUse::Arriving as i32]
        );
    }

    #[test]
    fn engm_segregated_during_day_when_low_visibility_reported() {
        let sel = enor_selector();
        let mut m = empty_metar("ENGM");
        m.obscuration = Some(runway_selector_protocol::v1::Obscuration {
            variant: Some(ObsVariant::Described(DescribedObscuration {
                visibility: Some(Visibility {
                    value: Some(VisValue::Meters(OptionalMeters { value: Some(3000) })),
                    direction: None,
                    no_directional_variation: false,
                }),
                directional_visibility: vec![],
                rvr: vec![],
                present_weather: vec![],
                clouds: vec![],
                vertical_visibility: None,
            })),
        });
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(m),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();
        assert!(
            out.runways
                .iter()
                .any(|r| r.r#use == RunwayUse::Departing as i32)
        );
    }

    #[test]
    fn engm_segregated_during_day_when_freezing_drizzle_reported() {
        let sel = enor_selector();
        let mut m = empty_metar("ENGM");
        m.obscuration = Some(runway_selector_protocol::v1::Obscuration {
            variant: Some(ObsVariant::Described(DescribedObscuration {
                visibility: Some(Visibility {
                    value: Some(VisValue::Meters(OptionalMeters { value: Some(9999) })),
                    direction: None,
                    no_directional_variation: false,
                }),
                directional_visibility: vec![],
                rvr: vec![],
                present_weather: vec![PresentWeather {
                    intensity: Some(WeatherIntensity::Light as i32),
                    descriptor: Some(WeatherDescriptor::Freezing as i32),
                    phenomena: vec![WeatherPhenomenonValue {
                        code: Some(WeatherPhenomenon::Dz as i32),
                    }],
                }],
                clouds: vec![],
                vertical_visibility: None,
            })),
        });
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(m),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();
        let uses: Vec<i32> = out.runways.iter().map(|r| r.r#use).collect();
        assert!(uses.contains(&(RunwayUse::Departing as i32)));
        assert!(uses.contains(&(RunwayUse::Arriving as i32)));
    }

    #[test]
    fn engm_segregated_during_day_when_rvr_reported() {
        let sel = enor_selector();
        let mut m = empty_metar("ENGM");
        m.obscuration = Some(runway_selector_protocol::v1::Obscuration {
            variant: Some(ObsVariant::Described(DescribedObscuration {
                visibility: Some(Visibility {
                    value: Some(VisValue::Meters(OptionalMeters { value: Some(9999) })),
                    direction: None,
                    no_directional_variation: false,
                }),
                directional_visibility: vec![],
                rvr: vec![Rvr {
                    runway: "01L".into(),
                    meters: Some(800),
                    modifier: None,
                    trend: None,
                }],
                present_weather: vec![],
                clouds: vec![],
                vertical_visibility: None,
            })),
        });
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(m),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();
        let uses: Vec<i32> = out.runways.iter().map(|r| r.r#use).collect();
        assert_eq!(
            uses,
            vec![RunwayUse::Departing as i32, RunwayUse::Arriving as i32]
        );
    }

    #[test]
    fn engm_mixed_when_low_clouds_are_few_or_scattered() {
        let sel = enor_selector();
        let mut m = empty_metar("ENGM");
        m.obscuration = Some(runway_selector_protocol::v1::Obscuration {
            variant: Some(ObsVariant::Described(DescribedObscuration {
                visibility: Some(Visibility {
                    value: Some(VisValue::Meters(OptionalMeters { value: Some(9999) })),
                    direction: None,
                    no_directional_variation: false,
                }),
                directional_visibility: vec![],
                rvr: vec![],
                present_weather: vec![],
                clouds: vec![Cloud {
                    variant: Some(runway_selector_protocol::v1::cloud::Variant::Data(
                        CloudData {
                            coverage: Some(CloudCoverage::Few as i32),
                            height_hundreds_ft: Some(5),
                            cloud_type: None,
                        },
                    )),
                }],
                vertical_visibility: None,
            })),
        });
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![runway("01L", 10, 0), runway("01R", 10, 0)],
            metar: Some(m),
            atis_runways: vec![],
        };
        let tz = TimeZone::get("Europe/Oslo").unwrap();
        let out = sel
            .select_for_airport(&airport, Some(&timestamp_at(12, 0)), &tz)
            .unwrap();
        assert!(
            out.runways
                .iter()
                .all(|r| r.r#use == RunwayUse::Both as i32)
        );
    }

    #[test]
    fn enzv_keeps_main_runway_when_crosswind_is_under_15kt() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", 10, 5),
                runway("36", -10, 5),
                runway("10", 2, 8),
                runway("28", -2, 8),
            ],
            metar: Some(empty_metar("ENZV")),
            atis_runways: vec![],
        };
        let out = sel
            .select_for_airport(&airport, None, &TimeZone::UTC)
            .unwrap();
        assert_eq!(out.runways.len(), 1);
        assert_eq!(out.runways[0].identifier, "18");
        assert_eq!(out.source, SelectionSource::Metar as i32);
    }

    #[test]
    fn enzv_switches_to_secondary_when_crosswind_high_and_secondary_lower() {
        let sel = enor_selector();
        // 18 crosswind 20 kt, 28 crosswind 5 kt → must switch to 28.
        let airport = AirportRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", -5, 20),
                runway("36", 5, 20),
                runway("10", -10, 5),
                runway("28", 10, 5),
            ],
            metar: Some(empty_metar("ENZV")),
            atis_runways: vec![],
        };
        let out = sel
            .select_for_airport(&airport, None, &TimeZone::UTC)
            .unwrap();
        assert_eq!(out.runways.len(), 1);
        assert_eq!(out.runways[0].identifier, "28");
    }

    #[test]
    fn enzv_keeps_main_when_secondary_crosswind_is_not_strictly_lower() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENZV".into(),
            runways: vec![
                runway("18", -5, 20),
                runway("36", 5, 20),
                runway("10", -10, 20),
                runway("28", 10, 20),
            ],
            metar: Some(empty_metar("ENZV")),
            atis_runways: vec![],
        };
        let out = sel
            .select_for_airport(&airport, None, &TimeZone::UTC)
            .unwrap();
        assert_eq!(out.runways[0].identifier, "36");
    }

    #[tokio::test]
    async fn select_runways_rpc_returns_per_airport_selections() {
        let sel = enor_selector();
        let airport = AirportRequest {
            icao: "ENBR".into(),
            runways: vec![runway("17", 12, 0), runway("35", -12, 0)],
            metar: Some(empty_metar("ENBR")),
            atis_runways: vec![],
        };
        let req = SelectRunwaysRequest {
            now_utc: Some(timestamp_at(12, 0)),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![airport],
        };
        let resp = sel.select_runways(Request::new(req)).await.unwrap();
        let body = resp.into_inner();
        assert_eq!(body.selections.len(), 1);
        assert_eq!(body.selections[0].runways[0].identifier, "17");
    }
}
