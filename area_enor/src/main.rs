//! ENOR area plugin. Spawned as a subprocess by `es_runway_selector`;
//! speaks the `runway_selector.v1` gRPC service plus the standard
//! `grpc.health.v1` health service.
//!
//! Bind port comes from `RUNWAY_SELECTOR_PORT`; the area's package directory
//! (where `area.toml` lives) comes from `RUNWAY_SELECTOR_AREA_DIR`.

use std::{env, net::SocketAddr, path::PathBuf};

use runway_selector_core::area_config::{AreaConfig, load_area_config};
use tonic::transport::Server;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod selector;

const SERVICE_NAME: &str = "runway_selector.v1.RunwaySelector";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,area_enor=debug")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let port: u16 = env::var("RUNWAY_SELECTOR_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or("RUNWAY_SELECTOR_PORT must be set to a u16")?;

    let area_dir: PathBuf = env::var("RUNWAY_SELECTOR_AREA_DIR")
        .ok()
        .map(PathBuf::from)
        .ok_or("RUNWAY_SELECTOR_AREA_DIR must point at the area package")?;

    let area_config = match load_area_config(&area_dir) {
        Ok(c) => c,
        Err(e) => {
            warn!(error = ?e, "Failed to load area.toml, using defaults");
            AreaConfig::default()
        }
    };

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<runway_selector_protocol::v1::runway_selector_server::RunwaySelectorServer<
            selector::EnorSelector,
        >>()
        .await;

    let selector = selector::EnorSelector::new(area_config);
    let svc =
        runway_selector_protocol::v1::runway_selector_server::RunwaySelectorServer::new(selector);

    info!(%addr, service = SERVICE_NAME, "ENOR area plugin starting");

    Server::builder()
        .add_service(health_service)
        .add_service(svc)
        .serve_with_shutdown(addr, shutdown_signal())
        .await?;

    info!("ENOR area plugin shutting down");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let mut sig =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        sig.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
