use runway_plugin_api::{
    helpers::{best_headwind, prefer_unless_crosswind},
    *,
};

pub fn select(airport: &AirportSelectionRequest) -> AirportSelectionResult {
    // ENZV: prefer the main runway pair (18/36) unless crosswind reaches 15 kt,
    // in which case try the cross runway (10/28) if it has less crosswind.

    // 1. Pick the better-headwind direction from the main pair.
    let main_pair = runways_matching(airport, &["18", "36"]);
    let main = best_headwind(&main_pair, 2)
        .or_else(|| airport.runways.iter().find(|r| r.identifier == "18"))
        .map(|r| r.identifier.clone())
        .unwrap_or_else(|| "18".to_string());

    // 2. Try main runway; switch to cross runway only when crosswind is high.
    let cross_pair = runways_matching(airport, &["10", "28"]);
    let cross = best_headwind(&cross_pair, 2)
        .or_else(|| airport.runways.iter().find(|r| r.identifier == "10"))
        .map(|r| r.identifier.clone())
        .unwrap_or_else(|| "10".to_string());

    // Build a two-entry slice so prefer_unless_crosswind can compare them.
    let candidates = runways_matching(airport, &[main.as_str(), cross.as_str()]);
    let selected = prefer_unless_crosswind(&candidates, &main, 15)
        .map(|r| r.identifier.clone())
        .unwrap_or(main);

    AirportSelectionResult {
        icao: airport.icao.clone(),
        handled: true,
        runway_uses: vec![RunwayUseEntry {
            runway: selected,
            use_: RunwayUse::Both,
        }],
    }
}

fn runways_matching(airport: &AirportSelectionRequest, ids: &[&str]) -> Vec<RunwayInfo> {
    airport
        .runways
        .iter()
        .filter(|r| ids.contains(&r.identifier.as_str()))
        .cloned()
        .collect()
}
