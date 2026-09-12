//! End-to-end integration test for the offline pipeline flags
//! (`--sector-file`, `--metar-fixture`, `--skip-atis`, `--skip-app-launchers`,
//! `--rwy-out`, `--report-out`): spawns the real `es_runway_selector` binary against a
//! fixture `.sct` and a fixture METAR file, with no live EuroScope install,
//! no network METAR/ATIS fetch, and no app launching. This is the
//! host-level counterpart to `area_enor/tests/e2e_spawn_select.rs`, which
//! only exercises the plugin in isolation.

use std::{fs, process::Command};

/// `directories::ProjectDirs::from("", "meltinglava", "es_runway_selector")`
/// resolves under `$HOME` on Linux/macOS; pointing `HOME` at a tempdir
/// isolates config/data dirs from the developer's real ones.
fn isolated_home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn offline_flags_run_the_real_pipeline_without_network_or_euroscope() {
    let home = isolated_home();

    // `directories` on Linux/macOS: config -> $HOME/.config/<app>,
    // data -> $HOME/.local/share/<app> (matches README.md's documented paths).
    let config_dir = home.path().join(".config/es_runway_selector");
    let data_dir = home.path().join(".local/share/es_runway_selector");
    let areas_dir = data_dir.join("areas");
    let area_dir = areas_dir.join("testarea");
    fs::create_dir_all(&area_dir).unwrap();
    fs::create_dir_all(&config_dir).unwrap();

    // A "plugin" whose entry binary doesn't exist: `spawn_plugin` fails fast
    // with `EntryMissing` and the host falls back to `default_runways` —
    // exactly what CLAUDE.md documents as graceful degradation. No working
    // plugin binary is needed to exercise the offline flags end to end.
    fs::write(
        area_dir.join("manifest.toml"),
        r#"
name = "testarea"
version = "0.1.0"
display_name = "Test Area"
runtime = "rust"
entry = "does-not-exist"
"#,
    )
    .unwrap();
    fs::write(
        area_dir.join("area.toml"),
        r#"
sector_file_prefix = "TEST"

[default_runways]
ENXX = 9
"#,
    )
    .unwrap();

    let fixtures_dir = home.path().join("fixtures");
    fs::create_dir_all(&fixtures_dir).unwrap();

    let sct_path = fixtures_dir.join("TEST.sct");
    fs::write(&sct_path, "[RUNWAY]\n09 27 090 270 ENXX\n\n").unwrap();

    let metar_path = fixtures_dir.join("metar.txt");
    fs::write(&metar_path, "ENXX 010800Z 09010KT CAVOK 15/05 Q1013\n").unwrap();

    // `write_runways_to_rwy_file` reads the existing file first (to preserve
    // any `ACTIVE_AIRPORT:` prefix), so the output path must already exist.
    let rwy_out = home.path().join("output.rwy");
    fs::write(&rwy_out, "").unwrap();

    let report_out = home.path().join("report.html");

    let output = Command::new(env!("CARGO_BIN_EXE_es_runway_selector"))
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .arg("--sector-file")
        .arg(&sct_path)
        .arg("--metar-fixture")
        .arg(&metar_path)
        .arg("--skip-atis")
        .arg("--skip-app-launchers")
        .arg("--rwy-out")
        .arg(&rwy_out)
        // Keep the test headless: write the report to a file instead of
        // opening it in the developer's browser.
        .arg("--report-out")
        .arg(&report_out)
        .output()
        .expect("failed to run es_runway_selector");

    assert!(
        output.status.success(),
        "offline run should exit successfully\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let rwy_contents = fs::read_to_string(&rwy_out).unwrap();
    assert!(
        rwy_contents.contains("ACTIVE_RUNWAY:ENXX:09:1"),
        "expected a departure line for the default-runway fallback, got:\n{rwy_contents}"
    );
    assert!(
        rwy_contents.contains("ACTIVE_RUNWAY:ENXX:09:0"),
        "expected an arrival line for the default-runway fallback, got:\n{rwy_contents}"
    );

    let report = fs::read_to_string(&report_out).unwrap();
    assert!(
        report.contains("ENXX"),
        "expected the HTML report to mention the airport, got:\n{report}"
    );
}
