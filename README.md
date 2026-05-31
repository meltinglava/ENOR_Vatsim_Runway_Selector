# Runway selector

[![pre-commit.ci status](https://results.pre-commit.ci/badge/github/meltinglava/ENOR_Vatsim_Runway_Selector/main.svg)](https://results.pre-commit.ci/latest/github/meltinglava/ENOR_Vatsim_Runway_Selector/main)

A small tool that picks active runways for
[EuroScope](https://www.euroscope.hu/) on the VATSIM network and writes
them straight into the sector folder's `.rwy` file. It pulls live METARs
and ATIS broadcasts, hands them to an *area plugin* that knows the local
operating rules, and renders an HTML report of what it picked. Built
originally for the Polaris area (Norway/ENOR), but the per-FIR logic
now lives in installable area plugins — `area_enor` is the first one.

> **Not for real-world operations.** This is a VATSIM convenience tool.
> Don't use it for anything that affects real traffic.

Based on a lot of earlier work by
[Adrian2k](https://github.com/Adrian2k/ENOR-autorwy).

---

## Contents

- [Install](#install)
- [Quickstart](#quickstart)
- [What it does on each run](#what-it-does-on-each-run)
- [Areas](#areas)
- [Profiles](#profiles)
- [Configuration](#configuration)
- [App launcher](#app-launcher)
- [Logs and troubleshooting](#logs-and-troubleshooting)
- [Writing your own area](#writing-your-own-area)
- [Issues](#issues)
- [License](#license)

---

## Install

### Pre-built binaries

Grab the latest release for your OS from the
[GitHub releases page](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/releases):

- **Windows** — `es_runway_selector-windows-msvc.zip`
- **Linux (musl)** — `es_runway_selector-linux-musl.tar.gz`

Unpack and put `es_runway_selector` somewhere on your `PATH` (or just
run it from wherever — the path doesn't matter).

The binary self-updates against GitHub releases on every non-debug
launch. If a newer version is available it'll download, swap itself
out, and restart in place.

### Build from source

You need a recent Rust toolchain (edition 2024 — Rust 1.85+).

```bash
git clone https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector
cd ENOR_Vatsim_Runway_Selector
cargo build --release --bin es_runway_selector
```

The resulting binary is at `target/release/es_runway_selector`.

---

## Quickstart

```bash
# 1. List areas you can install
es_runway_selector area available

# 2. Install the ENOR (Norway) area
es_runway_selector area install enor

# 3. Run it. Picks runways, writes the .rwy file, opens the report.
es_runway_selector
```

On the very first run with no areas installed, the host will print
guidance about which area to install based on the sector file it
found.

---

## What it does on each run

1. **Finds your EuroScope sector folder.** It checks (in order): the
   `euroscope_config_folder` in your `config.toml`, the standard
   EuroScope config directory, your Documents folder, and a hardcoded
   WSL path. On the first run if none of these work, you'll get a folder
   picker (not on the musl build — set `euroscope_config_folder`
   explicitly there).
2. **Loads your `.sct`** to learn what airports and runway directions
   exist.
3. **Spawns any non-EuroScope apps** from your `app_launchers.toml` in
   the background (e.g. TrackAudio).
4. **Fetches METARs** from the URLs the installed area declares.
5. **Reads VATSIM ATIS broadcasts** and pulls runway assignments out of
   the text via a regex stack (`RUNWAY XX IN USE`,
   `APPROACH RUNWAY XX`, `DEPARTURE RUNWAY XX`, etc.).
6. **Spawns the area plugin** and asks it which runways to use, given
   the METAR + ATIS data. The plugin attributes each pick to a source
   (ATIS, METAR, or DEFAULT).
7. **Fills any remaining gaps** with the area's per-airport default
   runways.
8. **Launches your EuroScope instance(s)** as specified in
   `app_launchers.toml`. The first one launches immediately; subsequent
   ones wait briefly so the first window becomes EuroScope's main one.
9. **Writes the `.rwy` file** (preserving the `ACTIVE_AIRPORT:` block
   at the top of your existing file) and opens an HTML runway report in
   your browser so you can see what got picked and why.

The whole cycle is one-shot — the tool exits as soon as the report is
open. Run it whenever you want fresh runway picks.

---

## Areas

An *area* is a per-FIR plugin: it knows the local operating rules
(time-of-day patterns, crosswind switches, parallel-runway segregation,
etc.) and decides which runways to assign for the airports it claims.

Areas are managed via the `area` subcommand:

```bash
es_runway_selector area available             # list installable areas
es_runway_selector area install enor          # install one
es_runway_selector area list                  # list installed areas
es_runway_selector area remove enor           # uninstall
es_runway_selector area profile list          # list per-area profiles
es_runway_selector area profile show <area> <profile>   # print resolved profile
```

`area install` downloads the area's tarball from the registry,
verifies its SHA-256 checksum, and extracts it under your local
install directory (default: `<data_dir>/areas/<name>`). Updates work
the same way — `area install` overwrites the area's directory but
**never touches `*.local.toml` files**.

To install an area from outside the upstream registry (e.g. a private
or staging one), see [`extra_registries`](#top-level-configtoml) below.

---

## Profiles

A *profile* inside an area is a controller position — TWR, APP,
RADAR — that picks which EuroScope `.prf` file gets opened and which
extra apps launch alongside.

```bash
es_runway_selector area profile list
# enor:
#   rads                 Radar / Approach
#   twr                  Tower / GND

es_runway_selector area profile show enor twr
# name        : twr
# display_name: Tower / GND
# prf_files   : ["enor_twr.prf"]
# default_apps: ["EuroScope", "TrackAudio"]
```

The profile-picker integration is wired but not yet selecting a profile
at runtime — the current build launches whatever's in
[`app_launchers.toml`](#app-launcher). Watch this section as it
evolves.

---

## Configuration

There are **two layers** of config, plus the area's own `area.toml`:

1. **Top-level config** at `<config_dir>/config.toml` — covers
   registry URLs, the EuroScope folder, app launchers, etc.
2. **Per-area config** at
   `<install_dir>/<area_name>/area.toml` — shipped by the area
   author.

Both layers support a sibling `*.local.toml` for **your overrides**.
`.local.toml` files are never touched by area updates or the
`--clean-config` flag, so anything you put there is yours forever.

The merge is layered: tables merge key-by-key, scalars and arrays are
replaced wholesale.

Where the files live:

| OS | Config dir | Data dir |
| --- | --- | --- |
| Windows | `%APPDATA%\meltinglava\es_runway_selector\config` | `%APPDATA%\meltinglava\es_runway_selector\data` |
| Linux | `~/.config/es_runway_selector` | `~/.local/share/es_runway_selector` |
| macOS | `~/Library/Application Support/meltinglava.es_runway_selector` | same |

Areas are installed under `<data_dir>/areas/<name>` by default; logs
go to `<data_dir>/logs/`.

### Top-level `config.toml`

Created automatically on first run. The full schema:

```toml
# Optional: pin your EuroScope sector folder so auto-detection is skipped.
euroscope_config_folder = "C:/Users/you/Documents/Euroscope/MyConfig"

# Optional: how long to wait before launching the 2nd+ EuroScope window
# so the first one becomes the "main" one. Default 2000.
es_main_window_delay_ms = 2000

# Optional: override the discovered exe path per app name.
# [euroscope_executable_path]
# EuroScope  = "C:/Program Files (x86)/EuroScope/EuroScope.exe"
# TrackAudio = "C:/Program Files/TrackAudio/TrackAudio.exe"

# Optional: where areas get unpacked. Defaults to <data_dir>/areas.
# areas_install_dir = "D:/runways-areas"

# Optional: which registries to look at for `area install/available`.
# area_registry_url = "https://example.org/areas.json"
# extra_registries  = ["https://example.org/private-areas.json"]

# Optional: auto-update areas at startup (TBD; flag exists for the future).
# auto_update_areas         = true
# auto_install_mise_runtimes = true
```

### User overrides in `config.local.toml`

Whatever the upstream default `config.toml` looks like, put your
personal tweaks in `config.local.toml` next to it. Example:

```toml
# config.local.toml
extra_registries = ["https://example.org/my-private-areas.json"]

[default_runways]
ENZV = 36   # I really want to land the other way today
```

Run with `--clean-config` (or `-c`) any time to regenerate
`config.toml` from the template (your `euroscope_config_folder` is
preserved). Your `config.local.toml` is untouched.

### Per-area overrides

For an installed area at `<install_dir>/enor/`, drop overrides into
sibling `*.local.toml` files:

- `area.local.toml` overrides anything from `area.toml` — e.g. swap a
  default runway, remove an airport from the ignore list, add an extra
  METAR URL.
- `profiles/<name>.local.toml` overrides a specific profile — e.g.
  change which `.prf` files open, or which apps launch.

These files survive `area install` upgrades and `area remove` /
re-install cycles only if you keep them yourself (`area remove`
deletes the whole area directory).

---

## App launcher

`es_runway_selector` can launch EuroScope (and other apps) for you
when it runs. To configure this:

1. Open the config folder. On Windows, paste
   `%APPDATA%\meltinglava\es_runway_selector\config` into Explorer's
   address bar. If the folder doesn't exist, run `es_runway_selector`
   once first and it'll be created.
2. Copy
   [`app_launchers.toml`](es_runway_selector/app_launchers.toml) into
   that folder.
3. Edit it to list the apps and EuroScope instances you want. The
   format:

   ```toml
   [[executable]]
   name = "EuroScope"
   prf  = "enor_rads.prf"     # opened relative to your sector folder

   [[executable]]
   name = "EuroScope"
   prf  = "enor_gnd.prf"

   [[executable]]
   name = "TrackAudio"        # no prf needed; just launched

   [[executable]]
   name = "vacs"
   ```

Each `name` is matched against installed apps on your system. If
auto-discovery doesn't find one, set its exe path explicitly in
`config.toml`'s `[euroscope_executable_path]` table.

---

## Logs and troubleshooting

**Logs.** Two streams write at every run:

- **stdout** — controlled by the `RUST_LOG` environment variable
  (e.g. `RUST_LOG=debug es_runway_selector`).
- **JSON file** — `<data_dir>/logs/es_runway_selector-<UTC ts>.json`,
  controlled by `--log-level / -l` (default
  `info,es_runway_selector=trace,reqwest=debug`). Files older than 14
  days are cleaned up automatically at startup.

If the tool crashes or behaves unexpectedly, the JSON log file is the
first thing to attach to a bug report.

**Common issues:**

| Symptom | Likely cause |
| --- | --- |
| "No areas installed" guidance on every run | You haven't run `area install <name>` yet. |
| "Plugin host error: Entry missing" | The installed area is missing its `plugin/<entry>` binary. Reinstall. |
| "Plugin host error: Timed out waiting for SERVING" | The area's subprocess crashed on startup. Run it standalone to see why. |
| `area install` fails on a non-Rust area with "mise required" | Install [mise](https://mise.jdx.dev/getting-started.html); it's needed for Python/Node/Deno areas. |
| The sector folder picker doesn't appear on Linux musl | Expected — musl builds skip `rfd`. Set `euroscope_config_folder` in `config.toml`. |
| All airports get DEFAULT-source runways | The area plugin failed (or isn't installed) and the host fell back to defaults. Check the JSON log. |

**CLI flags:**

```text
es_runway_selector [OPTIONS] [COMMAND]

COMMANDS:
    area    Manage installable area plugins

OPTIONS:
    -c, --clean-config        Rewrite the default config (preserves euroscope_config_folder)
    -l, --log-level <FILTER>  env-filter for the JSON file logger
    -h, --help                Print help
    -V, --version             Print version
```

---

## Writing your own area

Want to add a new FIR — or fork the existing logic to behave
differently for your group? Areas are independent gRPC subprocesses;
you can write one in Rust, Python, Node, or Deno.

The full developer guide lives in
**[`runway_selector_protocol/README.md`](runway_selector_protocol/README.md)**.
It covers:

- The on-disk package layout (`manifest.toml`, `area.toml`,
  `plugin/<entry>`, `profiles/`).
- The gRPC contract (`GetAirports`, `SelectRunways`) and what to put
  in each response.
- Choosing a runtime — including how non-Rust runtimes are bootstrapped
  via [`mise`](https://mise.jdx.dev/) so users don't have to install
  Python / Node / Deno themselves.
- Publishing: tarball layout, SHA-256 verification, registry JSON
  shape, primary vs self-hosted distribution.

---

## Issues

Report bugs or feature requests on the
[GitHub issue board](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/issues).
Before filing, check that nobody else has reported the same thing.

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
