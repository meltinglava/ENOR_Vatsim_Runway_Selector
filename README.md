# Runway selector

[![pre-commit.ci status](https://results.pre-commit.ci/badge/github/meltinglava/ENOR_Vatsim_Runway_Selector/main.svg)](https://results.pre-commit.ci/latest/github/meltinglava/ENOR_Vatsim_Runway_Selector/main)

Picks active runways for [EuroScope](https://www.euroscope.hu/) on VATSIM
and writes them straight into your sector folder's `.rwy` file. Can also
launch EuroScope (and TrackAudio, vACS, etc.) for you in one click, and
opens an HTML report so you can sanity-check the picks.

> **Not for real-world use.** VATSIM convenience tool only.

Based on earlier work by
[Adrian2k](https://github.com/Adrian2k/ENOR-autorwy).

---

## Get started

### 1. Download the binary

Grab the latest build from the
[releases page](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/releases):

- **Windows** — `es_runway_selector-windows-msvc.zip`
- **Linux (musl)** — `es_runway_selector-linux-musl.tar.gz`

Unpack it anywhere. The binary self-updates from GitHub on launch, so
you don't have to come back here for new versions.

### 2. Install your area

An *area* is the per-FIR plugin that knows the local rules. Run:

```bash
es_runway_selector area install enor       # Polaris / Norway FIR
```

The first time you run the tool without one installed, it'll print the
right command for the sector file it found.

### 3. Tell it what to launch (optional)

If you want one click to bring up EuroScope + TrackAudio + whatever
else, drop an `app_launchers.toml` into your config folder:

- **Windows** — paste `%APPDATA%\meltinglava\es_runway_selector\config`
  into Explorer's address bar.
- **Linux** — `~/.config/es_runway_selector/`.

Start from
[`es_runway_selector/app_launchers.toml`](es_runway_selector/app_launchers.toml)
in this repo. Format:

```toml
[[executable]]
name = "EuroScope"
prf  = "enor_rads.prf"     # path relative to your sector folder

[[executable]]
name = "EuroScope"
prf  = "enor_gnd.prf"      # second window — opens after the first

[[executable]]
name = "TrackAudio"        # no prf needed
```

If an app isn't found automatically, point at it in `config.toml`:

```toml
[euroscope_executable_path]
TrackAudio = "C:/Program Files/TrackAudio/TrackAudio.exe"
```

### 4. Run it

```bash
es_runway_selector
```

That's the whole loop. Re-run whenever you want fresh picks.

---

## Changing the defaults

Anywhere the tool ships a `foo.toml`, you can drop a `foo.local.toml`
next to it with your overrides. `.local.toml` files are yours — they
survive area updates and `--clean-config`.

Example — make ENZV default to runway 36:

```toml
# <install_dir>/enor/area.local.toml
[default_runways]
ENZV = 36
```

Where the files live:

|         | Config (`config.toml`, `app_launchers.toml`)           | Areas (`<area>/area.toml`, profiles)               |
| ------- | ------------------------------------------------------ | -------------------------------------------------- |
| Windows | `%APPDATA%\meltinglava\es_runway_selector\config`      | `%APPDATA%\meltinglava\es_runway_selector\data\areas` |
| Linux   | `~/.config/es_runway_selector`                         | `~/.local/share/es_runway_selector/areas`           |
| macOS   | `~/Library/Application Support/meltinglava.es_runway_selector` | same, under `areas/`                       |

Tables merge key-by-key; scalars and arrays are replaced wholesale.

---

## Managing areas

| Command                                  | Purpose                              |
| ---------------------------------------- | ------------------------------------ |
| `es_runway_selector area install <name>` | Install (or update) an area          |
| `es_runway_selector area list`           | List installed areas                 |
| `es_runway_selector area available`      | List installable areas               |
| `es_runway_selector area remove <name>`  | Uninstall                            |

To pull from a non-default registry (private FIR, staging, etc.), add
it to your `config.local.toml`:

```toml
extra_registries = ["https://example.org/my-areas.json"]
```

---

## Trouble?

Start with the JSON log file under `<data_dir>/logs/` — that's the
first thing to attach to a bug report. `RUST_LOG=debug
es_runway_selector` adds verbose stdout output.

| Symptom                                        | Likely cause                                                                  |
| ---------------------------------------------- | ----------------------------------------------------------------------------- |
| "No areas installed" every run                 | You haven't run `area install <name>` yet.                                    |
| Sector-folder picker never appears (Linux musl)| Expected. Set `euroscope_config_folder` in `config.toml`.                     |
| Plugin spawn error mentioning `mise`           | Install [mise](https://mise.jdx.dev/getting-started.html); non-Rust areas only. |
| Everything shows DEFAULT-source picks          | The area plugin failed. Check the JSON log.                                   |

---

## Build from source

Rust 1.85+ (edition 2024):

```bash
git clone https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector
cd ENOR_Vatsim_Runway_Selector
cargo build --release --bin es_runway_selector
```

Binary lands at `target/release/es_runway_selector`.

---

## Writing your own area

To add a new FIR — or fork an existing area for your group — see
[**runway_plugin_api/README.md**](runway_plugin_api/README.md).

---

## Issues

[GitHub issues](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/issues).
Check existing reports before filing.

---

## License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
