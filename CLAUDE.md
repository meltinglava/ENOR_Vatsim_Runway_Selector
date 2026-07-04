# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Runway Selector — a Rust tool that automatically selects active runways for [EuroScope](https://www.euroscope.hu/) (a flight simulation radar client) on the VATSIM network. It ingests live METAR data and ATIS broadcasts, applies area-specific selection logic, and writes the result to EuroScope's `.rwy` format. **Not for real-world operations.**

Originally hardcoded for the Polaris area (Norway/ENOR). Per-FIR selection logic now lives in installable area-plugin packages that the host spawns as subprocesses and drives over HTTP/JSON on localhost; `area_enor` is the first one. Plugins can be written in any language that can serve HTTP (Rust natively; Python/Node/Deno via `mise`).

## Workspace Structure

Nine crates in a Cargo workspace (`resolver = "3"`, edition 2024), plus example plugins under `examples/`:

### Host

- **`es_runway_selector/`** — Main application binary. Handles EuroScope config discovery, sector-file loading, METAR + ATIS fetching, `.rwy` writing, app launchers, the first-run wizard, and the `area …` subcommand for installing/updating areas. `plugin_runner` spawns every installed area's subprocess (disjoint ICAO ownership from each `manifest.toml supported_icaos` — the single authoritative airport list), sends one batch `POST /runway-selections`, and merges the results back onto `Airports::runways_in_use`. ATIS is applied host-side; airports already decided by ATIS are not sent to plugins. Plugin failures are logged, surfaced to the user, and degrade to defaults.

### Core libraries

- **`runway_selector_core/`** — Area-agnostic selection types and logic. Owns sector file decoding, METAR fetching, ATIS regex parsing, runway wind component math, the runway-source priority model, the host-side converter (`plugin_convert`) that lowers parsed METARs and pre-computed wind components into the HTTP/JSON plugin contract, the `.rwy` writer, and the HTML runway report (including plugin `SelectionTag` rendering). Area-specific runway-selection rules no longer live here.
- **`runway_plugin_api/`** — The single contract crate: serde wire types for `POST /runway-selections` (request-level UTC `timestamp_utc` + `area_timezone`, pre-computed per-runway wind, parsed METAR, per-airport `handled` opt-out flag, `SelectionTag`s) plus tested high-level selection helpers (`helpers::best_headwind`, `prefer_unless_tailwind`, `prefer_unless_crosswind`, `min_crosswind`, `within_crosswind_limit`). The OpenAPI spec is generated code-first from these types (`cargo run -p runway_plugin_api --features openapi --bin generate_openapi > openapi.json`) so it cannot drift; the committed copy lives at the workspace root.
- **`runway_selector_plugin_host/`** — Lifecycle for the subprocess plugins. `build_command` constructs the right `tokio::process::Command` (Rust runtimes exec the entry directly; Python/Node/Deno route through `mise exec`). `spawn_plugin` reserves a free localhost port, spawns the child, and polls `GET /health` until 200; `PluginHandle::select_runways` posts the batch request with an HTTP-status check, and `PluginHandle::shutdown` escalates `POST /shutdown` → SIGTERM (Unix) → kill, which makes graceful shutdown work on Windows too. Startup failures capture a stderr tail.
- **`runway_selector_areas/`** — Area registry, install, and removal. Fetches the registry JSON, downloads area tarballs, verifies SHA-256, and extracts to `<install_dir>/<name>/`. `list_installed_areas` enumerates `manifest.toml`s on disk.

### Area plugins

- **`area_enor/`** — First concrete area implementation. Rust binary (axum) serving `GET /health`, `POST /runway-selections`, `POST /shutdown`. Implements generic max-headwind selection (with a 2 kt margin; ambiguous wind answers `handled: false`), and the hand-tuned ENGM (Mixed/Segregated/Single ops driven by Europe/Oslo local time + METAR LVP triggers) and ENZV (15-kt crosswind switch from 18/36 to 10/28) rules, attaching `SelectionTag`s explaining each choice. Deterministic: time comes from the request's `timestamp_utc`, never the wall clock, and bad input returns HTTP 400 instead of panicking. Ships its package layout under `area_enor/package/` (manifest, area.toml, profiles/).

### Misc

- **`metar_decoder/`** — Library crate that parses raw METAR strings into a structured `Metar` value using `nom` parser combinators. Public API is intentionally unstable.
- **`find_bad_metar_job/`** — Utility binary that bulk-fetches all European METARs (`https://metar.vatsim.net/E` + `/L`) and accumulates ones the decoder fails on into `failed_metars.json`, used as a regression corpus for `metar_decoder`.

## Commands

```bash
# Format
cargo fmt --all

# Lint (CI enforces -D warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Test all crates
cargo test --workspace --all-features --locked

# Run a single test
cargo test -p <crate-name> <test_name>

# Build the host binary
cargo build --release --locked --bin es_runway_selector

# Build the ENOR area plugin
cargo build --release --locked --bin area_enor

# Compile-check against the CI target (Alpine/musl)
cargo build --target x86_64-unknown-linux-musl --locked --bin es_runway_selector
```

Pre-commit hooks (via `.pre-commit-config.yaml`) run `fmt`, `clippy`, `cargo check`, and `actionlint`. CI (`.github/workflows/ci.yml`) does PR checks inside `rust:alpine` (musl) and produces release artifacts for Windows MSVC and Linux musl when a `v*` tag is pushed.

## Architecture

### Runway selection model

Each `Airport` stores **all** known runway-selection sources, not just the winning one:

```rust
runways_in_use: IndexMap<RunwayInUseSource, IndexMap<String, RunwayUse>>
// source → { "01L" -> Departing, "01R" -> Arriving, ... }
```

`RunwayInUseSource` is `Atis | Metar | Default`, with `default_sort_order()` defining priority **ATIS > METAR > Default**. Consumers (the `.rwy` writer, the HTML report) walk that order and use the first source present. `RunwayUse` is `Departing | Arriving | Both`; `merged_with` is a lattice where any conflicting pair collapses upward to `Both`, used both by the ATIS regex parser and during aggregation.

### Area packages

An area package is a directory with this layout:

```text
<install_dir>/<name>/
    manifest.toml          # immutable area identity (name, version, runtime, entry)
    area.toml              # runtime defaults: METAR URLs, ignore ICAOs, default runways, IANA tz
    area.local.toml        # user sparse overrides (preserved across area updates)
    plugin/<entry>         # the binary/script spawned as the HTTP/JSON subprocess
    profiles/<profile>.toml         # controller positions (prf files, app launchers)
    profiles/<profile>.local.toml   # user sparse overrides
    test_fixtures/         # optional
```

The user-facing rule: **anything ending in `.local.toml` belongs to you and survives area updates.** `runway_selector_core::area_config::merge_local_overrides` does the layered merge — tables merge key-by-key, scalars/arrays are replaced wholesale.

### Output (`.rwy`)

`runway_selector_core::output::write_runways_to_rwy_file` first calls `read_active_airport`, which preserves the existing `ACTIVE_AIRPORT:` prefix block from the user's `.rwy`, then truncates and re-writes the file with that prefix followed by `ACTIVE_RUNWAY:ICAO:RUNWAY:FLAG` lines (flag `1` = departure, `0` = arrival; `Both` emits both rows).

### Data flow (`es_runway_selector/src/main.rs`)

1. `ESConfig::find_euroscope_config_folder` locates the newest `ENOR*.sct` (Euroscope config dir / Documents / a hardcoded WSL path `/mnt/c/Users/<user>/Documents/Euroscope/Euroscope_dev`), falling back to an `rfd` folder picker on non-musl builds. Loads/seeds `config.toml` and `app_launchers.toml` under `directories::ProjectDirs("", "meltinglava", "es_runway_selector")`.
2. First-run wizard (`wizard::detect_setup_state`) — checks for installed areas and prints guidance if there are none (or one with no profiles). Non-interactive; never blocks.
3. Spawn non-Euroscope `app_launchers` (e.g. TrackAudio) in parallel with the rest.
4. Parse the `.sct` `[RUNWAY]` section (UTF-8, then ISO-8859-1 fallback) into `Airport` + `Runway` records.
5. Fetch METARs from the configured URLs (`https://metar.vatsim.net/EN` + `/ESKS` currently hardcoded; moves to `area.toml`) and parse via `metar_decoder`.
6. Fetch VATSIM v3 data and parse `text_atis` per relevant ICAO via `runway_selector_core::atis::find_runway_in_use_from_atis` — a regex stack that recognizes `RUNWAY XX IN USE`, `APPROACH RUNWAY XX`, `DEPARTURE RUNWAY XX`, `RUNWAYS XX AND YY IN USE`, and the split `ARRIVAL/DEPARTURE INFORMATION` bulletin form.
7. `plugin_runner::run_area_selections` runs every installed area: ownership is assigned from each `manifest.toml supported_icaos` (first claim wins), airports already decided by ATIS are excluded, the plugin is spawned via `runway_selector_plugin_host::spawn_plugin`, one batch `POST /runway-selections` (with request-level `timestamp_utc` + `area_timezone`) is sent, and `handled: true` results are written into `Airports::runways_in_use` under the source the plugin attributes (METAR / DEFAULT) with the response `tags` stored on the airport for the report. If no area is installed (or a plugin errors), the host logs, surfaces a warning, and continues with defaults only.
8. `Airports::apply_default_runways` fills the `Default` source from the area's `default_runways` for airports still without any selection.
9. Spawn EuroScope launchers (`prf` paths joined onto the sector-file folder; the first instance launches immediately, subsequent ones wait `es_main_window_delay_ms`, default 2000 ms, so the first window becomes the main one).
10. Write the `.rwy` file and open a temp HTML runway report (`make_runway_report_html`, askama template `runway_selector_core/templates/runway_report.html`) via `open::that_detached`.

### Airport-specific runway logic (in `area_enor::selector`)

Every per-airport selection rule lives in `area_enor::selector` and operates on the `runway_plugin_api` request types (parsed METAR, pre-computed `headwind_kt`/`tailwind_kt`/`crosswind_kt` per direction). Dispatch is by ICAO.

- **Generic** — pick the runway whose `headwind_kt` is strictly the highest, with a ≥ 2 kt margin over the runner-up (via `runway_plugin_api::helpers::best_headwind`). Tied / ambiguous winds answer `handled: false` and let the host fall back to area defaults.
- **ENGM (Oslo Gardermoen)** — `select_for_engm` picks a direction prefix ("01" or "19") by grouping runways and picking the prefix with the highest max headwind (same 2 kt margin), then chooses **Mixed / Segregated / Single** ops based on the request's `now_utc` + `area_timezone` (segregated after 22:30 local, single before 06:30 local) and METAR-derived LVP triggers — cloud ceiling < 1500 ft, any RVR group, visibility < 5000 m, vertical visibility, freezing weather, possible-de-ice precipitation with temperature < 5 °C (or unknown). Mixed emits `XXL`/`XXR` as `Both`; Segregated splits dep/arr (`L`=Departing, `R`=Arriving); Single picks `01L` or `19R`.
- **ENZV (Stavanger)** — `select_for_enzv` defaults to `18/36` (whichever has the higher headwind). If that runway's pre-computed `crosswind_kt` is ≥ 15 and the perpendicular runway has a strictly lower crosswind, it switches to the secondary (10/28) runway.

Core (`runway_selector_core::airport`) owns the wind-component math (`runway_max_headwind` / `runway_max_crosswind` / `runway_wind_components`) — computed **once** on the host, used both for the HTML report's wind columns and to populate the per-runway wind fields shipped to plugins (`plugin_convert::airport_to_request`). Plugins never do wind trigonometry.

### `metar_decoder`

`lib.rs` re-exports modules `metar`, `wind`, `pressure`, `temperature`, `obscuration`, `nato_mil_code`, `trend`, `optional_data`, `sea_surface_indicator`, `units`. Each contributes a `nom` parser to the top-level `Metar` (fields include `raw`, `icao`, `timestamp`, `wind`, `obscuration`, `temperature`, `pressure`, optional `recent_weather` / `tempo` / `becoming` / `nato_mil_code` / `remarks`, plus `corrected` / `auto` / `nosig` flags). `OptionalData<T, N>` represents the `/`-padded "field present but value unknown" form that's common in military and automated reports.

### Runtime / CLI

`es_runway_selector` is a Tokio multi-thread binary. Flags:

- `--clean-config` / `-c` — rewrite the config from the embedded `config.toml` template (preserves `euroscope_config_folder` if previously set).
- `--log-level` / `-l` — env-filter string for the JSON file logger (default `info,es_runway_selector=trace,reqwest=debug`). `RUST_LOG` still controls the stdout layer.
- `--previous-log-path` (hidden) — used internally by the self-update restart path.

Subcommands:

- `area list` — list locally installed areas
- `area available` — list areas the registry advertises
- `area install <name>` — download, verify SHA-256, and extract
- `area remove <name>`
- `area profile list` — every profile in every installed area
- `area profile show <area> <profile>` — print the resolved profile contents

On non-debug builds, `main` checks GitHub releases via `self_update`. On a successful update it respawns the new binary with `--previous-log-path` pointing at the current JSON log file so the upgrade continues in one log.

### Logging

Two `tracing` layers: an ANSI stdout layer (filter from `RUST_LOG`) and a JSON file layer (`tracing-appender` non-blocking) writing to `<ProjectDirs::data_dir()>/logs/es_runway_selector-YYYYMMDD-HHMMSSZ.json`. Files older than 14 days are deleted at startup. `tracing_unwrap` is used liberally (`unwrap_or_log`, `expect_or_log`) instead of plain `unwrap`.

### Platform notes

- `rfd` (file-picker dialog) is gated out on `target_env = "musl"`, so the musl build cannot prompt for a sector folder — it must be auto-discoverable or already set in `config.toml`. CI exercises this implicitly by building under Alpine.
- `find_exe_path` and `AppLauncher::run` have Windows-specific branches: `.lnk` shortcuts are launched via `cmd /c start ""`, and processes use `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` creation flags so they outlive this binary.
- Non-Rust plugin runtimes (Python, Node, Deno) are routed through [`mise`](https://mise.jdx.dev/) so users don't have to install language runtimes manually.
- Startup work that can block on UI (config-folder discovery / `rfd` dialog) runs **before** the Tokio runtime is built (`prepare_startup` in `main.rs`); doing it inside `block_on` froze on Windows.
