//! ENOR area plugin for es_runway_selector.
//!
//! Handles runway selection for ENGM (Oslo Gardermoen) and ENZV (Stavanger Sola).
//!
//! ## Startup
//!
//! The plugin reads two environment variables set by the parent:
//! - `ES_RUNWAY_SELECTOR_PLUGIN_PORT` – the TCP port it should listen on.
//! - `ES_RUNWAY_SELECTOR_PORT`        – the parent's helper API port.
//!
//! ## Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/health` | Returns 200 when ready |
//! | GET | `/airports` | Returns `["ENGM", "ENZV"]` |
//! | POST | `/atis` | Delegates to parent `/parse-atis` for each airport |
//! | POST | `/runways` | ENGM time/weather logic + ENZV crosswind logic |

mod engm;
mod enzv;

use std::net::SocketAddr;

use axum::{Json, Router, extract::State, routing::get, routing::post};
use reqwest::Client;
use runway_selector_protocol::{
    AirportRunwayAssignment, AtisRequest, AtisResponse, ParseAtisRequest, ParseAtisResponse,
    PluginAirportsResponse, RunwayAssignment, RunwaySelectionRequest, RunwaySelectionResponse,
    SelectionTag,
};
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const HANDLED_AIRPORTS: &[&str] = &["ENGM", "ENZV"];

#[derive(Clone)]
struct PluginState {
    parent_port: u16,
    client: Client,
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

async fn health() -> &'static str {
    "ok"
}

async fn airports() -> Json<PluginAirportsResponse> {
    Json(PluginAirportsResponse {
        airports: HANDLED_AIRPORTS.iter().map(|s| s.to_string()).collect(),
    })
}

/// POST /atis
///
/// For each ATIS entry, calls the parent's `/parse-atis` helper to do the
/// regex-based parsing, then returns the results.
async fn atis_handler(
    State(state): State<PluginState>,
    Json(req): Json<AtisRequest>,
) -> Json<AtisResponse> {
    let mut airports = Vec::new();

    for entry in &req.atis_entries {
        if !HANDLED_AIRPORTS.contains(&entry.airport_icao.as_str()) {
            continue;
        }

        let url = format!("http://127.0.0.1:{}/parse-atis", state.parent_port);
        let parse_req = ParseAtisRequest {
            atis_text: entry.atis_text.clone(),
        };

        match state.client.post(&url).json(&parse_req).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ParseAtisResponse>().await {
                    Ok(parsed) => {
                        airports.push(AirportRunwayAssignment {
                            airport_icao: entry.airport_icao.clone(),
                            assignments: parsed.assignments,
                            tags: Vec::new(),
                        });
                    }
                    Err(e) => warn!(
                        airport = %entry.airport_icao,
                        error = %e,
                        "Failed to deserialize /parse-atis response"
                    ),
                }
            }
            Ok(resp) => warn!(
                airport = %entry.airport_icao,
                status = %resp.status(),
                "Parent /parse-atis returned error"
            ),
            Err(e) => warn!(
                airport = %entry.airport_icao,
                error = %e,
                "Failed to call parent /parse-atis"
            ),
        }
    }

    Json(AtisResponse { airports })
}

/// POST /runways
///
/// Delegates to ENGM-specific or ENZV-specific logic.
async fn runways_handler(
    State(_state): State<PluginState>,
    Json(req): Json<RunwaySelectionRequest>,
) -> Json<RunwaySelectionResponse> {
    let (runways, tags): (Vec<RunwayAssignment>, Vec<SelectionTag>) =
        match req.airport.icao.as_str() {
            "ENGM" => engm::select_runways(&req.airport, req.metar.as_ref()),
            "ENZV" => enzv::select_runways(&req.airport, req.metar.as_ref()),
            other => {
                warn!(
                    airport = other,
                    "ENOR plugin asked to select runways for unknown airport"
                );
                (Vec::new(), Vec::new())
            }
        };
    Json(RunwaySelectionResponse { runways, tags })
}

// ─── Entry point ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let plugin_port: u16 = std::env::var("ES_RUNWAY_SELECTOR_PLUGIN_PORT")
        .expect("ES_RUNWAY_SELECTOR_PLUGIN_PORT not set")
        .parse()
        .expect("ES_RUNWAY_SELECTOR_PLUGIN_PORT is not a valid port number");

    let parent_port: u16 = std::env::var("ES_RUNWAY_SELECTOR_PORT")
        .expect("ES_RUNWAY_SELECTOR_PORT not set")
        .parse()
        .expect("ES_RUNWAY_SELECTOR_PORT is not a valid port number");

    let state = PluginState {
        parent_port,
        client: Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build reqwest client"),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/airports", get(airports))
        .route("/atis", post(atis_handler))
        .route("/runways", post(runways_handler))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], plugin_port));
    let listener = TcpListener::bind(addr)
        .await
        .expect("Failed to bind plugin listener");

    info!(port = plugin_port, parent_port, "ENOR plugin listening");

    axum::serve(listener, app)
        .await
        .expect("Plugin server error");
}
