//! Minimal Rust area plugin.
//!
//! Handles two made-up airports (`ZZZA`, `ZZZB`):
//!   * If the host parsed a runway out of ATIS, pass it through as `ATIS`.
//!   * Otherwise pick the runway with the strongest headwind, attribute it
//!     to `METAR`, and assign it to both arrivals and departures.
//!   * If there's no usable wind, omit the airport so the host falls back to
//!     `area.toml`'s `default_runways`.

use std::{env, net::SocketAddr};

use runway_selector_protocol::v1::{
    AirportRequest, AirportSelection, GetAirportsResponse, RunwayAssignment, RunwayUse,
    SelectRunwaysRequest, SelectRunwaysResponse, SelectionSource,
    runway_selector_server::{RunwaySelector, RunwaySelectorServer},
};
use tonic::{Request, Response, Status, transport::Server};

const ICAOS: &[&str] = &["ZZZA", "ZZZB"];

#[derive(Default)]
pub struct ExampleArea;

#[tonic::async_trait]
impl RunwaySelector for ExampleArea {
    async fn get_airports(&self, _: Request<()>) -> Result<Response<GetAirportsResponse>, Status> {
        Ok(Response::new(GetAirportsResponse {
            icaos: ICAOS.iter().map(|s| s.to_string()).collect(),
        }))
    }

    async fn select_runways(
        &self,
        request: Request<SelectRunwaysRequest>,
    ) -> Result<Response<SelectRunwaysResponse>, Status> {
        let selections = request
            .into_inner()
            .airports
            .into_iter()
            .filter_map(pick)
            .collect();
        Ok(Response::new(SelectRunwaysResponse { selections }))
    }
}

fn pick(airport: AirportRequest) -> Option<AirportSelection> {
    if !airport.atis_runways.is_empty() {
        return Some(AirportSelection {
            icao: airport.icao,
            source: SelectionSource::Atis as i32,
            runways: airport.atis_runways,
        });
    }

    let best = airport
        .runways
        .iter()
        .filter(|r| r.wind_components.is_some())
        .max_by_key(|r| r.wind_components.as_ref().unwrap().headwind_kt)?;

    Some(AirportSelection {
        icao: airport.icao,
        source: SelectionSource::Metar as i32,
        runways: vec![RunwayAssignment {
            identifier: best.identifier.clone(),
            r#use: RunwayUse::Both as i32,
        }],
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let port: u16 = env::var("RUNWAY_SELECTOR_PORT")?.parse()?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;

    let (health, health_svc) = tonic_health::server::health_reporter();
    health
        .set_serving::<RunwaySelectorServer<ExampleArea>>()
        .await;

    Server::builder()
        .add_service(health_svc)
        .add_service(RunwaySelectorServer::new(ExampleArea))
        .serve_with_shutdown(addr, shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
