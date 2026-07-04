//! Minimal Rust area plugin.
//!
//! Handles two made-up airports (`ZZZA`, `ZZZB`):
//!   * Pick the runway with the strongest headwind (the host has already
//!     computed the wind components — no trigonometry here) and assign it
//!     to both arrivals and departures.
//!   * If there's no usable wind, answer `handled: false` so the host falls
//!     back to `area.toml`'s `default_runways`.
//!
//! Airports the host already decided from ATIS never reach the plugin.
//!
//! Serves the HTTP/JSON contract from `runway_plugin_api`:
//! `GET /health`, `POST /runway-selections`, `POST /shutdown`.

use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use runway_plugin_api::{
    AirportSelectionRequest, AirportSelectionResult, RunwaySelectionsRequest,
    RunwaySelectionsResponse, RunwayUse, RunwayUseEntry, SelectionSource, helpers::best_headwind,
};
use tokio::sync::Notify;

fn pick(airport: &AirportSelectionRequest) -> AirportSelectionResult {
    match best_headwind(&airport.runways, 0) {
        Some(best) => AirportSelectionResult {
            icao: airport.icao.clone(),
            handled: true,
            source: SelectionSource::Metar,
            runway_uses: vec![RunwayUseEntry {
                runway: best.identifier.clone(),
                use_: RunwayUse::Both,
            }],
            tags: vec![],
        },
        // No usable wind: let the host use its defaults for this airport.
        None => AirportSelectionResult {
            icao: airport.icao.clone(),
            handled: false,
            source: SelectionSource::Metar,
            runway_uses: vec![],
            tags: vec![],
        },
    }
}

async fn runway_selections(
    Json(request): Json<RunwaySelectionsRequest>,
) -> Json<RunwaySelectionsResponse> {
    Json(RunwaySelectionsResponse {
        results: request.airports.iter().map(pick).collect(),
    })
}

async fn shutdown(State(notify): State<Arc<Notify>>) -> StatusCode {
    notify.notify_one();
    StatusCode::OK
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = env::var("RUNWAY_SELECTOR_PORT")?.parse()?;

    let notify = Arc::new(Notify::new());
    let app = Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/runway-selections", post(runway_selections))
        .route("/shutdown", post(shutdown))
        .with_state(notify.clone());

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { notify.notified().await })
        .await?;
    Ok(())
}
