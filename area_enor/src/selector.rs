//! Runway-selection logic for the ENOR area.
//!
//! Strategy:
//! 1. If the request has ATIS-derived runways, pass them through with
//!    `SOURCE = ATIS` — controllers' authority overrides our math.
//! 2. Otherwise pick the runway whose pre-computed `WindComponents` show the
//!    highest headwind, with a 2 kt margin over the next-best direction to
//!    avoid flipping every cycle. `SOURCE = METAR`.
//! 3. If no METAR is available (or it has no usable wind), emit nothing and
//!    let the host fall back to area defaults.
//!
//! ENGM (Oslo) and ENZV (Stavanger) hand-tuned rules are scheduled to move
//! here in the Phase 8 cleanup; for now the generic logic above runs for
//! every airport.

use runway_selector_core::area_config::AreaConfig;
use runway_selector_protocol::v1::{
    AirportRequest, AirportSelection, GetAirportsResponse, RunwayAssignment, RunwayUse,
    SelectRunwaysRequest, SelectRunwaysResponse, SelectionSource,
    runway_selector_server::RunwaySelector,
};
use tonic::{Request, Response, Status};
use tracing::debug;

const HEADWIND_MARGIN_KT: i32 = 2;

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

    fn select_for_airport(airport: &AirportRequest) -> Option<AirportSelection> {
        if !airport.atis_runways.is_empty() {
            return Some(AirportSelection {
                icao: airport.icao.clone(),
                source: SelectionSource::Atis as i32,
                runways: airport.atis_runways.clone(),
            });
        }

        // Require a METAR to be present even though wind is read off
        // pre-computed `WindComponents` — without a METAR there is no signal.
        airport.metar.as_ref()?;

        let best_runway = pick_best_headwind(&airport.runways)?;
        Some(AirportSelection {
            icao: airport.icao.clone(),
            source: SelectionSource::Metar as i32,
            runways: vec![RunwayAssignment {
                identifier: best_runway,
                r#use: RunwayUse::Both as i32,
            }],
        })
    }
}

/// Return the runway identifier with the strictly highest headwind, provided
/// it beats the next-best direction by at least [`HEADWIND_MARGIN_KT`]. The
/// margin avoids flipping direction every cycle when winds are near
/// perpendicular.
fn pick_best_headwind(runways: &[runway_selector_protocol::v1::RunwayInfo]) -> Option<String> {
    let mut scored: Vec<(i32, &str)> = runways
        .iter()
        .filter_map(|r| {
            r.wind_components
                .as_ref()
                .map(|wc| (wc.headwind_kt, r.identifier.as_str()))
        })
        .collect();

    scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));

    let (top_score, top_id) = scored.first().copied()?;
    let beats_runner_up = match scored.get(1) {
        Some((second, _)) => top_score.saturating_sub(*second) >= HEADWIND_MARGIN_KT,
        None => true,
    };

    if beats_runner_up {
        Some(top_id.to_string())
    } else {
        // ambiguous wind → don't select; let host fall back to defaults
        None
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

        let selections = req
            .airports
            .iter()
            .filter_map(Self::select_for_airport)
            .collect();

        Ok(Response::new(SelectRunwaysResponse { selections }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runway_selector_protocol::v1::{CrosswindDirection, RunwayInfo, WindComponents};

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
    fn ignores_runways_without_wind_components() {
        let mut r1 = runway("01", 5, 0);
        r1.wind_components = None;
        let r2 = runway("19", 12, 0);
        assert_eq!(pick_best_headwind(&[r1, r2]), Some("19".to_string()));
    }

    #[test]
    fn select_for_airport_passes_atis_through() {
        let airport = AirportRequest {
            icao: "ENGM".into(),
            runways: vec![],
            metar: None,
            atis_runways: vec![RunwayAssignment {
                identifier: "01L".into(),
                r#use: RunwayUse::Departing as i32,
            }],
        };
        let sel = EnorSelector::select_for_airport(&airport).unwrap();
        assert_eq!(sel.source, SelectionSource::Atis as i32);
        assert_eq!(sel.runways.len(), 1);
        assert_eq!(sel.runways[0].identifier, "01L");
    }

    #[test]
    fn select_for_airport_returns_none_without_metar() {
        let airport = AirportRequest {
            icao: "ENZV".into(),
            runways: vec![runway("18", 10, 0)],
            metar: None,
            atis_runways: vec![],
        };
        assert!(EnorSelector::select_for_airport(&airport).is_none());
    }
}
