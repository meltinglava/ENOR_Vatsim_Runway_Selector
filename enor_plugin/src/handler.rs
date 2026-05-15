use axum::{Json, http::StatusCode};
use runway_plugin_api::*;

use crate::{engm, enzv};

pub async fn health() -> StatusCode {
    StatusCode::OK
}

pub async fn runway_selections(
    Json(request): Json<RunwaySelectionsRequest>,
) -> Json<RunwaySelectionsResponse> {
    let results = request
        .airports
        .iter()
        .map(|airport| match airport.icao.as_str() {
            "ENGM" => engm::select(airport),
            "ENZV" => enzv::select(airport),
            _ => AirportSelectionResult {
                icao: airport.icao.clone(),
                handled: false,
                runway_uses: vec![],
            },
        })
        .collect();

    Json(RunwaySelectionsResponse { results })
}
