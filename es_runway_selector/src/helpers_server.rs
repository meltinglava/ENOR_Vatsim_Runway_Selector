use axum::{Json, Router, routing::post};
use runway_plugin_api::{
    BestHeadwindRequest, MinCrosswindRequest, PreferUnlessCrosswindRequest,
    PreferUnlessTailwindRequest, RunwayInfo, RunwayResult, RunwaysResult,
    WithinCrosswindLimitRequest,
    helpers::{
        best_headwind, min_crosswind, prefer_unless_crosswind, prefer_unless_tailwind,
        within_crosswind_limit,
    },
};

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

/// Return the runway with the greatest headwind advantage.
///
/// The winner must beat the runner-up by strictly more than `advantage_threshold_kt`.
/// Returns `null` when no runway has METAR wind data or no clear winner exists.
#[utoipa::path(
    post,
    path = "/helpers/best-headwind",
    tag = "Helpers API",
    request_body = BestHeadwindRequest,
    responses((status = 200, body = RunwayResult))
)]
async fn handle_best_headwind(Json(req): Json<BestHeadwindRequest>) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: best_headwind(&req.runways, req.advantage_threshold_kt)
            .map(|r| r.identifier.clone()),
    })
}

/// Return `preferred_id` unless its tailwind exceeds `max_tailwind_kt`.
///
/// Falls back to `best_headwind` (threshold = 0) when the preferred runway is unsuitable.
/// Returns `null` when no METAR wind data is available.
#[utoipa::path(
    post,
    path = "/helpers/prefer-unless-tailwind",
    tag = "Helpers API",
    request_body = PreferUnlessTailwindRequest,
    responses((status = 200, body = RunwayResult))
)]
async fn handle_prefer_unless_tailwind(
    Json(req): Json<PreferUnlessTailwindRequest>,
) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: prefer_unless_tailwind(&req.runways, &req.preferred_id, req.max_tailwind_kt)
            .map(|r| r.identifier.clone()),
    })
}

/// Return `preferred_id` unless its crosswind exceeds `max_crosswind_kt`.
///
/// Falls back to `best_headwind` (threshold = 0) when the preferred runway is unsuitable.
/// Returns `null` when no METAR wind data is available.
#[utoipa::path(
    post,
    path = "/helpers/prefer-unless-crosswind",
    tag = "Helpers API",
    request_body = PreferUnlessCrosswindRequest,
    responses((status = 200, body = RunwayResult))
)]
async fn handle_prefer_unless_crosswind(
    Json(req): Json<PreferUnlessCrosswindRequest>,
) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: prefer_unless_crosswind(&req.runways, &req.preferred_id, req.max_crosswind_kt)
            .map(|r| r.identifier.clone()),
    })
}

/// Return the runway with the smallest crosswind component.
///
/// Returns `null` when no runway has METAR wind data.
#[utoipa::path(
    post,
    path = "/helpers/min-crosswind",
    tag = "Helpers API",
    request_body = MinCrosswindRequest,
    responses((status = 200, body = RunwayResult))
)]
async fn handle_min_crosswind(Json(req): Json<MinCrosswindRequest>) -> Json<RunwayResult> {
    Json(RunwayResult {
        runway: min_crosswind(&req.runways).map(|r| r.identifier.clone()),
    })
}

/// Return all runways whose crosswind is at or below `max_kt`.
#[utoipa::path(
    post,
    path = "/helpers/within-crosswind-limit",
    tag = "Helpers API",
    request_body = WithinCrosswindLimitRequest,
    responses((status = 200, body = RunwaysResult))
)]
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

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Helpers API",
        description = "
Endpoints hosted by `es_runway_selector` on the port passed as `--helpers-port`.
Call these from your plugin to use built-in runway selection algorithms.
",
        version = "1"
    ),
    paths(
        handle_best_headwind,
        handle_prefer_unless_tailwind,
        handle_prefer_unless_crosswind,
        handle_min_crosswind,
        handle_within_crosswind_limit,
    ),
    components(schemas(
        BestHeadwindRequest,
        PreferUnlessTailwindRequest,
        PreferUnlessCrosswindRequest,
        MinCrosswindRequest,
        WithinCrosswindLimitRequest,
        RunwayResult,
        RunwaysResult,
        RunwayInfo,
        runway_plugin_api::CrosswindDirection,
    )),
    tags(
        (name = "Helpers API", description = "Endpoints hosted by es_runway_selector for plugins to call"),
    )
)]
pub struct HelpersApiDoc;
