mod engm;
mod enzv;
mod handler;

use std::net::SocketAddr;

use axum::routing::{get, post};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(about = "ENOR runway selector plugin server")]
struct Args {
    /// TCP port to listen on
    #[arg(long)]
    port: u16,
    /// Port of the caller's helpers server (available but not used by this plugin,
    /// which accesses helpers natively via runway_plugin_api::helpers)
    #[arg(long)]
    helpers_port: Option<u16>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    let app = axum::Router::new()
        .route("/health", get(handler::health))
        .route("/runway-selections", post(handler::runway_selections));

    tracing::info!("enor_plugin listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");
    axum::serve(listener, app).await.expect("Server error");
}
