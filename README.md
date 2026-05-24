# ENOR Runway Selector

[![pre-commit.ci status](https://results.pre-commit.ci/badge/github/meltinglava/ENOR_Vatsim_Runway_Selector/main.svg)](https://results.pre-commit.ci/latest/github/meltinglava/ENOR_Vatsim_Runway_Selector/main)

Automatically selects active runways for [EuroScope](https://www.euroscope.hu/) using live METAR data and VATSIM ATIS broadcasts. Designed for VATSIM virtual ATC — **not for real-world operations**.

Based on earlier work by [Adrian2k](https://github.com/Adrian2k/ENOR-autorwy).

---

## How it works

```mermaid
flowchart TD
    SCT["EuroScope .sct file"]
    METAR["VATSIM METARs"]
    ATIS["VATSIM ATIS"]

    PR["Parse runways"]
    DW["Decode weather"]
    PA["Parse active RWY"]

    SEL["Runway selection\n(ATIS › METAR › default)"]
    PLUG["Area plugins (HTTP)\ncustom logic per FIR/airport"]
    OUT["Write ACTIVE_RUNWAY lines to .rwy file\n before staring EuroScope"]

    SCT --> PR
    METAR --> DW
    ATIS --> PA

    PR --> SEL
    DW --> SEL
    PA --> SEL

    SEL --> PLUG
    PLUG --> OUT
```

Runway sources are applied in priority order: **ATIS** > **METAR wind** > **fallbacks**.

---

## Quick start

1. Download the latest release for your platform from the [releases page](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/releases).
2. Run `es_runway_selector` once — it creates the config directory at `%APPDATA%\meltinglava\es_runway_selector\` (Windows) or the equivalent platform path, and automatically detects your EuroScope sector files.
3. Copy [es_runway_selector/app_launchers.toml](es_runway_selector/app_launchers.toml) into the `config/` subdirectory and edit it to point at your EuroScope `.prf` files.
4. Run `es_runway_selector` again — it will open EuroScope and begin selecting runways.

---

## Configuration

All config files live in the application config directory (printed on first run).

### Automatic setup

On first run the selector scans your EuroScope data folder and creates a config folder for every sector-file area it finds. For example, if your EuroScope folder contains `ENOR-Norway_20250101.sct` and `ESOS-Sweden_20250101.sct`, it will automatically create:

```
config/
  ENOR/
    area.toml
  ESOS/
    area.toml
```

In most cases you don't need to do anything else — the sector file location is found automatically.

### Adjusting area settings (optional)

Open `config/<AREA>/area.toml` to customise behaviour for that area. The main things you might want to set are:

- `ignore_airports` — ICAO codes the selector should skip entirely
- `default_runways` — fallback runway to use when neither ATIS nor METAR wind gives a clear answer
- `plugins` — area plugins to activate (see *Plugins* below)

```toml
# config/ENOR/area.toml

ignore_airports = ["ENNA"]

[default_runways]
ENZV = 18
```

### Multiple controller positions (optional)

If you connect to the same area from more than one EuroScope position — for example Tower and Approach — create `config/<AREA>/profiles.toml` to give each position its own name:

```toml
# config/ENOR/profiles.toml

[[profiles]]
name = "TWR"

[[profiles]]
name = "APP"
```

Both positions share the same sector file. A profile can override any setting from `area.toml` (ignored airports, default runways, plugins). The profile name becomes `ENOR/TWR` or `ENOR/APP` — pass it on the command line to select one:

```
es_runway_selector --profile ENOR/TWR
```

With only one profile configured, running without `--profile` selects it automatically. `--profile ENOR` also works and auto-selects when there is only one `ENOR/*` profile.

For multiple areas in the same installation, each area gets its own folder:

```
config/
  ENOR/
    area.toml
    profiles.toml   (optional)
  ESOS/
    area.toml
```

### Plugins (`config/plugins.toml`)

Each entry defines an external HTTP plugin that handles runway selection for specific airports:

```toml
[[plugins]]
name    = "enor"
command = "es_runway_selector_area_enor"

# Optional: use mise to manage the runtime (Python, Node.js, etc.)
[[plugins]]
name        = "egtt-py"
command     = "python main.py"
runtime     = "python@3.12"       # passed to `mise exec`
working_dir = "C:/plugins/egtt"
```

The parent passes two environment variables to every plugin:
- `ES_RUNWAY_SELECTOR_PLUGIN_PORT` — the TCP port the plugin must listen on
- `ES_RUNWAY_SELECTOR_PORT` — the parent's own helper API port

### Area config downloads (`config/areas.toml`)

Area configs bundle the sector file, plugin binary, and default settings for a FIR:

```toml
# From a dedicated GitHub repo (looks for a release asset matching *.tar.gz or *.zip)
[[areas]]
name   = "enor"
source = { type = "github", repo = "meltinglava/ENOR_Vatsim_Runway_Selector" }

# From a central manifest listing multiple areas
[[areas]]
name   = "egtt"
source = { type = "manifest", url = "https://example.com/areas.json", key = "egtt" }
```

```
es_runway_selector --list-areas          # show available areas
es_runway_selector --download-area enor  # download and install
```

---

## CLI reference

```
es_runway_selector [OPTIONS]

Options:
  -p, --profile <NAME>         Use a specific profile from profiles.toml
      --generate-openapi       Write openapi.json to the current directory and exit
      --download-area <AREA>   Download and install an area config package
      --list-areas             List available areas from areas.toml sources
      --clean-config           Re-initialise config directory
  -h, --help                   Print help
```

---

## Plugin development

A plugin is an HTTP server that the parent process starts and communicates with. It must implement four endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Return `200 OK` when ready to serve requests |
| `GET` | `/airports` | Return `{"airports": ["ICAO", ...]}` |
| `POST` | `/atis` | Parse ATIS texts and return runway assignments |
| `POST` | `/runways` | Select active runways from airport info + METAR |

The parent also exposes helper endpoints that plugins can call back:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Parent health check |
| `POST` | `/parse-atis` | Built-in regex ATIS parser |
| `POST` | `/parse-metar` | Parse a raw METAR string |

The full request/response schema is documented in [`openapi.json`](openapi.json) (auto-generated during build). Use it to generate typed client/server code in any language.

### Startup sequence

1. Parent allocates a free port and starts the plugin process with the two env vars set.
2. Parent polls `GET /health` up to 30 times (200 ms apart) until it returns `200`.
3. Parent calls `GET /airports` to learn which ICAO codes this plugin handles.
4. During each cycle, parent routes ATIS and METAR data to the appropriate plugin.

### Generating types from the OpenAPI spec

`openapi.json` at the workspace root is regenerated automatically whenever `es_runway_selector_area_enor` is built (via its `build.rs`). Use it with your preferred code generator:

**TypeScript** — `openapi-typescript`:
```sh
npx openapi-typescript openapi.json -o schema.ts
```

**Python** — `datamodel-codegen`:
```sh
pip install datamodel-code-generator
datamodel-codegen \
    --input openapi.json \
    --input-file-type openapi \
    --output models.py \
    --output-model-type pydantic_v2.BaseModel
```

**Any language** via [openapi-generator](https://openapi-generator.tech/):
```sh
openapi-generator generate -i openapi.json -g <language> -o ./generated
```

Or generate the spec directly without compiling the plugin:
```sh
cargo run --bin es_runway_selector -- --generate-openapi
```

---

## Example plugins

Ready-to-use skeletons are in the [`examples/`](examples/) directory.

Both examples use [mise](https://mise.jdx.dev/) to manage their runtime (Node/Python) and install packages into a project-local directory — nothing touches your global environment.

### TypeScript + Express

```sh
cd examples/typescript-plugin
mise run install    # installs Node 22 + npm packages into node_modules/
mise run generate   # writes src/generated/schema.ts from ../../openapi.json
mise run dev
```

See [`examples/typescript-plugin/src/index.ts`](examples/typescript-plugin/src/index.ts).

### Python + FastAPI

```sh
cd examples/python-plugin
mise run install    # installs Python 3.12 + pip packages into .venv/
mise run generate   # writes generated/models.py from ../../openapi.json
mise run dev
```

See [`examples/python-plugin/main.py`](examples/python-plugin/main.py).

Both skeletons delegate ATIS parsing to the parent's `/parse-atis` helper and include `TODO` stubs for per-airport runway selection logic.

---

## Reference implementation: ENOR plugin

The `es_runway_selector_area_enor` crate is the reference Rust plugin for Norway. It handles:

- **ENGM** (Oslo Gardermoen) — Mixed/Segregated/Single modes based on Oslo local time and LVP weather criteria.
- **ENZV** (Stavanger Sola) — Main runway 18/36 with crosswind fallback to secondary 10/28.

Its `build.rs` regenerates `openapi.json` at the workspace root whenever the protocol types change, ensuring the examples always have an up-to-date spec.

---

## Building from source

```sh
# Format
cargo fmt --all

# Lint
cargo clippy -- -D warnings

# Test
cargo test --all-features --locked

# Release binary
cargo build --release --locked --bin es_runway_selector
```

CI targets `x86_64-unknown-linux-musl` for checks and produces release artifacts for Windows (MSVC) and Linux (musl).

---

## App launcher

The application can open EuroScope (and other programs) at startup.

1. Open `%APPDATA%\meltinglava\es_runway_selector\config\` (run the app once to create it).
2. Copy [es_runway_selector/app_launchers.toml](es_runway_selector/app_launchers.toml) into that folder.
3. Edit the file to match your EuroScope `.prf` files and how many instances you want.

---

## Issues

Report bugs on the [GitHub issue board](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/issues).

---

#### License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you shall be dual licensed as above, without any additional terms or conditions.
