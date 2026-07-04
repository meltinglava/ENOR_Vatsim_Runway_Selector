//! End-to-end integration test: spawn the real `area_enor` binary through
//! the real `runway_selector_plugin_host` lifecycle (free port, health gate,
//! HTTP request, graceful shutdown) and assert the selections that come back.
//!
//! This is the test none of the three prototype branches had: it exercises
//! the actual wire format and the actual subprocess plumbing, not just the
//! selector functions.

use std::fs;
use std::path::Path;

use runway_plugin_api::{
    AirportSelectionRequest, CrosswindDirection, RunwayInfo, RunwaySelectionsRequest, RunwayUse,
    SelectionSource,
};
use runway_selector_area_config::{AreaManifest, Runtime};
use runway_selector_plugin_host::spawn_plugin;
use semver::Version;

/// Stage a minimal installed-area package in a temp dir: the compiled
/// `area_enor` binary under `plugin/`, plus `manifest.toml` and `area.toml`.
fn stage_area_package(dir: &Path) -> AreaManifest {
    let plugin_dir = dir.join("plugin");
    fs::create_dir_all(&plugin_dir).unwrap();

    let built_binary = env!("CARGO_BIN_EXE_area_enor");
    let entry_name = Path::new(built_binary).file_name().unwrap();
    // fs::copy preserves the executable bit on Unix.
    fs::copy(built_binary, plugin_dir.join(entry_name)).unwrap();

    fs::write(
        dir.join("area.toml"),
        r#"
time_zone = "Europe/Oslo"
sector_file_prefix = "ENOR"

[default_runways]
ENGM = 1
ENZV = 18
"#,
    )
    .unwrap();

    let manifest = AreaManifest {
        name: "enor-e2e".into(),
        version: Version::new(0, 1, 0),
        display_name: "ENOR e2e".into(),
        description: None,
        runtime: Runtime::Rust,
        entry: entry_name.to_string_lossy().into_owned(),
        supported_icaos: vec!["ENGM".into(), "ENBR".into()],
        min_core_version: None,
    };
    // The host normally reads manifest.toml from disk; write it too so the
    // staged package is complete and loadable.
    fs::write(
        dir.join("manifest.toml"),
        toml::to_string(&manifest).unwrap(),
    )
    .unwrap();
    manifest
}

fn runway(identifier: &str, heading: u16, headwind: i32, crosswind: i32) -> RunwayInfo {
    RunwayInfo {
        identifier: identifier.into(),
        heading,
        headwind_kt: Some(headwind),
        tailwind_kt: Some((-headwind).max(0)),
        crosswind_kt: Some(crosswind),
        crosswind_direction: Some(CrosswindDirection::Left),
    }
}

#[tokio::test]
async fn spawns_real_plugin_and_gets_selections_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = stage_area_package(dir.path());

    let handle = spawn_plugin(&manifest, dir.path(), &Version::new(99, 0, 0))
        .await
        .expect("plugin should spawn and report healthy");

    // Daytime Oslo (12:00 local on 2026-05-31 is 10:00 UTC) → ENGM mixed ops.
    let request = RunwaySelectionsRequest {
        timestamp_utc: "2026-05-31T10:00:00Z".into(),
        area_timezone: "Europe/Oslo".into(),
        airports: vec![
            AirportSelectionRequest {
                icao: "ENGM".into(),
                runways: vec![
                    runway("01L", 7, 10, 0),
                    runway("01R", 7, 10, 0),
                    runway("19L", 187, -10, 0),
                    runway("19R", 187, -10, 0),
                ],
                metar: Some(runway_plugin_api::MetarData {
                    raw: "ENGM 311050Z 01010KT CAVOK 15/05 Q1013".into(),
                    parsed: Some(runway_plugin_api::ParsedMetar {
                        is_cavok: true,
                        wind: None,
                        visibility_meters: None,
                        rvr: vec![],
                        clouds: vec![],
                        vertical_visibility_hundreds_ft: None,
                        weather_phenomena: vec![],
                        temperature_c: Some(15),
                        dew_point_c: Some(5),
                        qnh_hpa: Some(1013),
                    }),
                }),
            },
            // Generic airport with a clear headwind winner.
            AirportSelectionRequest {
                icao: "ENBR".into(),
                runways: vec![runway("17", 166, 12, 2), runway("35", 346, -12, 2)],
                metar: Some(runway_plugin_api::MetarData {
                    raw: "ENBR 311050Z 17012KT CAVOK 14/06 Q1014".into(),
                    parsed: None,
                }),
            },
            // No METAR: the plugin must defer with handled=false.
            AirportSelectionRequest {
                icao: "ENVA".into(),
                runways: vec![runway("09", 81, 0, 0)],
                metar: None,
            },
        ],
    };

    let response = handle
        .select_runways(&request)
        .await
        .expect("POST /runway-selections should succeed");

    assert_eq!(response.results.len(), 3);

    let engm = &response.results[0];
    assert_eq!(engm.icao, "ENGM");
    assert!(engm.handled);
    assert_eq!(engm.source, SelectionSource::Metar);
    let ids: Vec<&str> = engm.runway_uses.iter().map(|r| r.runway.as_str()).collect();
    assert_eq!(ids, vec!["01L", "01R"], "daytime calm CAVOK → mixed ops");
    assert!(engm.runway_uses.iter().all(|r| r.use_ == RunwayUse::Both));
    assert!(
        engm.tags.iter().any(|t| t.id == "engm-mixed"),
        "mixed-mode tag should explain the selection, got {:?}",
        engm.tags
    );

    let enbr = &response.results[1];
    assert!(enbr.handled);
    assert_eq!(enbr.runway_uses[0].runway, "17");

    let enva = &response.results[2];
    assert!(!enva.handled, "no METAR → defer to host defaults");
    assert!(enva.runway_uses.is_empty());

    let status = handle
        .shutdown()
        .await
        .expect("graceful shutdown should succeed");
    assert!(
        status.success(),
        "plugin should exit cleanly after POST /shutdown, got {status}"
    );
}

#[tokio::test]
async fn plugin_returns_http_400_for_bad_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = stage_area_package(dir.path());

    let handle = spawn_plugin(&manifest, dir.path(), &Version::new(99, 0, 0))
        .await
        .expect("plugin should spawn and report healthy");

    let request = RunwaySelectionsRequest {
        timestamp_utc: "garbage".into(),
        area_timezone: "Europe/Oslo".into(),
        airports: vec![],
    };

    let err = handle
        .select_runways(&request)
        .await
        .expect_err("bad timestamp must be rejected, not panic the plugin");
    let message = err.to_string();
    assert!(
        message.contains("400"),
        "expected an HTTP 400 error, got: {message}"
    );

    // The plugin must survive the bad request and still shut down cleanly.
    let status = handle.shutdown().await.unwrap();
    assert!(status.success());
}
