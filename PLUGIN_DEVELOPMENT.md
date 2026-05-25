# Plugin development

This guide is for people writing a new area plugin. For instructions on
*configuring* an already-built plugin, see the [main README](README.md).

A plugin is an HTTP server that the runway selector starts on launch and talks
to over `localhost`. Plugins can be written in any language — the protocol is
OpenAPI 3, and Rust, Python, and TypeScript starter skeletons are provided.

---

## Protocol

The plugin must expose four endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/health`   | Return `200 OK` when ready to serve requests |
| `GET`  | `/airports` | Return `{"airports": ["ICAO", ...]}` |
| `POST` | `/atis`     | Parse ATIS texts and return runway assignments |
| `POST` | `/runways`  | Select active runways from airport info + METAR |

The parent process also exposes helper endpoints that plugins can call back to:

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/health`      | Parent health check |
| `POST` | `/parse-atis`  | Built-in regex ATIS parser |
| `POST` | `/parse-metar` | Parse a raw METAR string |

The full request/response schema lives in [`openapi.json`](openapi.json) at the
workspace root. It is regenerated automatically whenever
`es_runway_selector_area_enor` is built (via its `build.rs`).

---

## Startup sequence

1. Parent allocates a free port and starts the plugin process with two
   environment variables set:
   - `ES_RUNWAY_SELECTOR_PLUGIN_PORT` — the TCP port the plugin must listen on
   - `ES_RUNWAY_SELECTOR_PORT` — the parent's own helper API port
2. Parent polls `GET /health` up to 30 times (200 ms apart) until it returns `200`.
3. Parent calls `GET /airports` to learn which ICAO codes this plugin handles.
4. During each cycle, parent routes ATIS and METAR data to the appropriate plugin.

---

## Generating types from the OpenAPI spec

You can either use the spec file generated during a normal build, or regenerate
it on demand without compiling the plugin:

```sh
cargo run --bin es_runway_selector -- --generate-openapi
```

Then feed it to your preferred code generator:

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

---

## Example plugins

Ready-to-use skeletons are in the [`examples/`](examples/) directory. Both use
[mise](https://mise.jdx.dev/) to manage their runtime and install packages into
a project-local directory — nothing touches your global environment.

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

Both skeletons delegate ATIS parsing to the parent's `/parse-atis` helper and
include `TODO` stubs for per-airport runway selection logic.

---

## Reference implementation: ENOR plugin

The `es_runway_selector_area_enor` crate is the reference Rust plugin for
Norway. It handles:

- **ENGM** (Oslo Gardermoen) — Mixed/Segregated/Single modes based on Oslo
  local time and LVP weather criteria.
- **ENZV** (Stavanger Sola) — Main runway 18/36 with crosswind fallback to
  secondary 10/28.

Its `build.rs` regenerates `openapi.json` at the workspace root whenever the
protocol types change, ensuring the examples always have an up-to-date spec.
