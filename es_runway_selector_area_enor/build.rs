use runway_selector_protocol::openapi::generate_openapi_json;
use std::path::Path;

fn main() {
    let spec = generate_openapi_json();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .expect("CARGO_MANIFEST_DIR has no parent");

    std::fs::write(workspace_root.join("openapi.json"), spec)
        .expect("failed to write openapi.json to workspace root");

    // Re-run whenever the protocol types or spec definition change.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../runway_selector_protocol/src/types.rs");
    println!("cargo:rerun-if-changed=../runway_selector_protocol/src/openapi.rs");
}
