use std::net::SocketAddr;

use axum::{Json, Router, extract::State, routing::get, routing::post};
use tokio::net::TcpListener;
use tracing::info;

use runway_selector_protocol::{
    ParseAtisRequest, ParseAtisResponse, ParseMetarRequest, ParseMetarResponse, RunwayAssignment,
    RunwayUse, openapi::PluginAndParentApiDoc,
};
use utoipa::OpenApi;

use crate::{atis_parser::find_runway_in_use_from_atis, runway::RunwayUse as InternalRunwayUse};

/// Shared state available to all handlers.
#[derive(Clone)]
struct AppState;

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// GET /health
#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Server is healthy")),
    tag = "parent"
)]
async fn health() -> &'static str {
    "ok"
}

/// POST /parse-atis
///
/// Runs the built-in regex ATIS parser. Plugins can call this to avoid
/// re-implementing parsing logic.
#[utoipa::path(
    post,
    path = "/parse-atis",
    request_body = ParseAtisRequest,
    responses(
        (status = 200, description = "Parsed runway assignments", body = ParseAtisResponse)
    ),
    tag = "parent"
)]
async fn parse_atis(
    State(_): State<AppState>,
    Json(req): Json<ParseAtisRequest>,
) -> Json<ParseAtisResponse> {
    let map = find_runway_in_use_from_atis(&req.atis_text);
    let assignments = map
        .into_iter()
        .map(|(runway_id, use_)| RunwayAssignment {
            runway_id,
            runway_use: internal_to_protocol_use(use_),
        })
        .collect();
    Json(ParseAtisResponse { assignments })
}

/// POST /parse-metar
///
/// Parses a raw METAR string using `metar_decoder` and returns structured data.
#[utoipa::path(
    post,
    path = "/parse-metar",
    request_body = ParseMetarRequest,
    responses(
        (status = 200, description = "Parsed METAR data", body = ParseMetarResponse)
    ),
    tag = "parent"
)]
async fn parse_metar(
    State(_): State<AppState>,
    Json(req): Json<ParseMetarRequest>,
) -> Json<ParseMetarResponse> {
    match req.raw_metar.parse::<metar_decoder::metar::Metar>() {
        Ok(metar) => Json(ParseMetarResponse {
            metar: Some(crate::protocol_convert::metar_to_protocol(&metar)),
            error: None,
        }),
        Err(e) => Json(ParseMetarResponse {
            metar: None,
            error: Some(e.to_string()),
        }),
    }
}

// ─── Conversions ──────────────────────────────────────────────────────────────

fn internal_to_protocol_use(use_: InternalRunwayUse) -> RunwayUse {
    match use_ {
        InternalRunwayUse::Departing => RunwayUse::Departing,
        InternalRunwayUse::Arriving => RunwayUse::Arriving,
        InternalRunwayUse::Both => RunwayUse::Both,
    }
}

// ─── Server startup ───────────────────────────────────────────────────────────

/// Start the parent helper HTTP API. Returns the bound port.
pub(crate) async fn start(requested_port: u16) -> std::io::Result<u16> {
    let app = Router::new()
        .route("/health", get(health))
        .route("/parse-atis", post(parse_atis))
        .route("/parse-metar", post(parse_metar))
        .with_state(AppState);

    let addr = SocketAddr::from(([127, 0, 0, 1], requested_port));
    let listener = TcpListener::bind(addr).await?;
    let actual_port = listener.local_addr()?.port();

    info!(port = actual_port, "Parent API server listening");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("API server error");
    });

    Ok(actual_port)
}

// ─── OpenAPI spec generation ──────────────────────────────────────────────────

/// Serialise the combined OpenAPI spec to pretty JSON.
pub(crate) fn generate_openapi_json() -> String {
    PluginAndParentApiDoc::openapi()
        .to_pretty_json()
        .expect("Failed to serialize OpenAPI spec")
}
