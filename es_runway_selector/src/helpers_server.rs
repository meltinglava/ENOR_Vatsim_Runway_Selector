use axum::{Json, Router, routing::post};
use runway_plugin_api::{
    RunwayInfo,
    helpers::{
        best_headwind, min_crosswind, prefer_unless_crosswind, prefer_unless_tailwind,
        within_crosswind_limit,
    },
};
use serde::{Deserialize, Serialize};

pub struct HelpersServer {
    pub port: u16,
    _handle: tokio::task::JoinHandle<()>,
}

impl HelpersServer {
    pub async fn start() -> std::io::Result<Self> {
        let port = crate::plugin_client::find_free_port().await?;
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;

        let app = Router::new()
            .route("/helpers/best-headwind", post(handle_best_headwind))
            .route(
                "/helpers/prefer-unless-tailwind",
                post(handle_prefer_unless_tailwind),
            )
            .route(
                "/helpers/prefer-unless-crosswind",
                post(handle_prefer_unless_crosswind),
            )
            .route("/helpers/min-crosswind", post(handle_min_crosswind))
            .route(
                "/helpers/within-crosswind-limit",
                post(handle_within_crosswind_limit),
            );

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok(Self {
            port,
            _handle: handle,
        })
    }
}

impl Drop for HelpersServer {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct BestHeadwindRequest {
    runways: Vec<RunwayInfo>,
    advantage_threshold_kt: i32,
}

#[derive(Deserialize)]
struct PreferUnlessTailwindRequest {
    runways: Vec<RunwayInfo>,
    preferred_id: String,
    max_tailwind_kt: i32,
}

#[derive(Deserialize)]
struct PreferUnlessCrosswindRequest {
    runways: Vec<RunwayInfo>,
    preferred_id: String,
    max_crosswind_kt: i32,
}

#[derive(Deserialize)]
struct RunwaysOnlyRequest {
    runways: Vec<RunwayInfo>,
}

#[derive(Deserialize)]
struct WithinCrosswindLimitRequest {
    runways: Vec<RunwayInfo>,
    max_kt: i32,
}

/// Single-runway result — `null` when no runway qualifies.
#[derive(Serialize)]
struct RunwayResult {
    runway: Option<String>,
}

/// Multi-runway result.
#[derive(Serialize)]
struct RunwaysResult {
    runways: Vec<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn handle_best_headwind(Json(req): Json<BestHeadwindRequest>) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: best_headwind(&req.runways, req.advantage_threshold_kt)
            .map(|r| r.identifier.clone()),
    })
}

async fn handle_prefer_unless_tailwind(
    Json(req): Json<PreferUnlessTailwindRequest>,
) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: prefer_unless_tailwind(&req.runways, &req.preferred_id, req.max_tailwind_kt)
            .map(|r| r.identifier.clone()),
    })
}

async fn handle_prefer_unless_crosswind(
    Json(req): Json<PreferUnlessCrosswindRequest>,
) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: prefer_unless_crosswind(&req.runways, &req.preferred_id, req.max_crosswind_kt)
            .map(|r| r.identifier.clone()),
    })
}

async fn handle_min_crosswind(Json(req): Json<RunwaysOnlyRequest>) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: min_crosswind(&req.runways).map(|r| r.identifier.clone()),
    })
}

async fn handle_within_crosswind_limit(
    Json(req): Json<WithinCrosswindLimitRequest>,
) -> Json<RunwaysResult> {
    Json(RunwaysResult {
        runways: within_crosswind_limit(&req.runways, req.max_kt)
            .into_iter()
            .map(|r| r.identifier.clone())
            .collect(),
    })
}
