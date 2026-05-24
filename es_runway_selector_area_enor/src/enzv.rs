//! Stavanger Sola (ENZV) runway selection logic.
//!
//! ENZV has two runway pairs:
//! - Main: 18/36
//! - Secondary: 10/28
//!
//! Use the main runway unless crosswind exceeds 15 kt, in which case check
//! whether the secondary offers lower crosswind.

use runway_selector_protocol::{
    AirportInfo, MetarData, PhysicalRunway, RunwayAssignment, RunwayInfo, RunwayUse, SelectionTag,
    wind_calc::{crosswind_kt, headwind_kt},
};
use tracing::warn;

pub fn select_runways(
    airport: &AirportInfo,
    metar: Option<&MetarData>,
) -> (Vec<RunwayAssignment>, Vec<SelectionTag>) {
    let Some(wind) = metar.and_then(|m| m.wind.as_ref()) else {
        // No wind data: use default runway 18.
        return (
            vec![RunwayAssignment {
                runway_id: "18".to_string(),
                runway_use: RunwayUse::Both,
            }],
            Vec::new(),
        );
    };

    let main_pair = airport
        .runways
        .iter()
        .find(|rw| rw.iter().any(|r| r.identifier == "18"));
    let secondary_pair = airport
        .runways
        .iter()
        .find(|rw| rw.iter().any(|r| r.identifier == "10"));

    let Some(main_pair) = main_pair else {
        warn!("ENZV main runway pair (18/36) not found in airport data");
        return (Vec::new(), Vec::new());
    };

    // Pick the better-headwind direction from the main pair.
    let main_rwy = best_headwind_direction(main_pair, wind);
    let main_cw = crosswind_kt(main_rwy, wind);

    if main_cw < 15.0 {
        return (
            vec![RunwayAssignment {
                runway_id: main_rwy.identifier.clone(),
                runway_use: RunwayUse::Both,
            }],
            Vec::new(),
        );
    }

    // Crosswind ≥ 15 kt: compare with secondary pair.
    let Some(secondary_pair) = secondary_pair else {
        return (
            vec![RunwayAssignment {
                runway_id: main_rwy.identifier.clone(),
                runway_use: RunwayUse::Both,
            }],
            Vec::new(),
        );
    };

    let secondary_rwy = best_headwind_direction(secondary_pair, wind);
    let secondary_cw = crosswind_kt(secondary_rwy, wind);

    let chosen = if secondary_cw < main_cw {
        secondary_rwy
    } else {
        main_rwy
    };
    (
        vec![RunwayAssignment {
            runway_id: chosen.identifier.clone(),
            runway_use: RunwayUse::Both,
        }],
        Vec::new(),
    )
}

fn best_headwind_direction<'a>(
    runway: &'a PhysicalRunway,
    wind: &runway_selector_protocol::WindData,
) -> &'a RunwayInfo {
    runway
        .iter()
        .max_by(|a, b| {
            headwind_kt(a, wind)
                .partial_cmp(&headwind_kt(b, wind))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap() // non-empty by construction
}
