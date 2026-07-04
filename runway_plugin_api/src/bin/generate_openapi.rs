//! Prints the plugin-API OpenAPI document as JSON on stdout:
//!
//! ```sh
//! cargo run -p runway_plugin_api --features openapi --bin generate_openapi > openapi.json
//! ```
//!
//! Deliberately a manual step (not a build script): generated specs written
//! into the workspace on every build dirty git and race under parallel
//! builds. Commit the output when the contract changes.

use utoipa::OpenApi;

fn main() {
    println!(
        "{}",
        runway_plugin_api::PluginApiDoc::openapi()
            .to_pretty_json()
            .expect("OpenAPI document serializes")
    );
}
