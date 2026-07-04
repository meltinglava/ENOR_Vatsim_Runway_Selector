//! ENOR area plugin. Spawned as a subprocess by `es_runway_selector`;
//! serves the HTTP/JSON plugin contract from `runway_plugin_api`:
//! `GET /health`, `POST /runway-selections`, `POST /shutdown`.
//!
//! Bind port comes from `RUNWAY_SELECTOR_PORT`; the area's package directory
//! (where `area.toml` lives) comes from `RUNWAY_SELECTOR_AREA_DIR`.

use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use runway_plugin_api::{RunwaySelectionsRequest, RunwaySelectionsResponse};
use runway_selector_area_config::{AreaConfig, load_area_config};
use tokio::sync::Notify;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

mod selector;

struct AppState {
    selector: selector::EnorSelector,
    shutdown: Notify,
}

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

    let state = Arc::new(AppState {
        selector: selector::EnorSelector::new(area_config),
        shutdown: Notify::new(),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/runway-selections", post(runway_selections))
        .route("/shutdown", post(shutdown))
        .with_state(state.clone());

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "ENOR area plugin starting");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await?;

    info!("ENOR area plugin shutting down");
    Ok(())
}

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn runway_selections(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RunwaySelectionsRequest>,
) -> Result<Json<RunwaySelectionsResponse>, (StatusCode, String)> {
    state
        .selector
        .select_runways(&request)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

async fn shutdown(State(state): State<Arc<AppState>>) -> StatusCode {
    info!("Received POST /shutdown; exiting gracefully");
    state.shutdown.notify_one();
    StatusCode::OK
}

/// Resolves when the host asks us to stop: `POST /shutdown` (all platforms),
/// Ctrl-C, or SIGTERM (Unix).
async fn shutdown_signal(state: Arc<AppState>) {
    let notified = state.shutdown.notified();
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                warn!(error = ?e, "Failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = notified => {},
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
