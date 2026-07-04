//! Layered area-package configuration.
//!
//! An *area* is the unit of distribution that knows how to pick runways for a
//! specific FIR (Polaris/ENOR, Stockholm/ESOS, …). On disk:
//!
//! ```text
//! areas/<name>/
//!     manifest.toml         # area identity — never edited by users
//!     area.toml             # area defaults — replaced by area updates
//!     area.local.toml       # user sparse overrides (never touched by updates)
//!     plugin/               # gRPC server entry point
//!     profiles/
//!         <profile>.toml          # ships with the area
//!         <profile>.local.toml    # user sparse overrides
//!     test_fixtures/
//! ```
//!
//! The user-facing rule is: **anything ending in `.local.toml` belongs to you
//! and survives area updates.** [`merge_local_overrides`] implements that —
//! tables are merged key-by-key, every other value is replaced wholesale.
//!
//! This crate exists as its own workspace member so that area plugins and the
//! registry crate can depend on the small config surface without pulling in
//! the rest of `runway_selector_core` (METAR / ATIS / HTML report / `.rwy`
//! writer transitively bring askama, vatsim_utils, open, tempfile, …).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use indexmap::{IndexMap, IndexSet};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AreaConfigError {
    #[error("I/O error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

pub type AreaConfigResult<T> = Result<T, AreaConfigError>;

/// Immutable area identity. Lives in `manifest.toml` and is not subject to
/// `.local.toml` overrides — the area's identity is fixed by its publisher.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AreaManifest {
    pub name: String,
    pub version: Version,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub runtime: Runtime,
    /// Entry path relative to the area's `plugin/` directory. Spawned as a
    /// subprocess that speaks the gRPC protocol.
    pub entry: String,
    #[serde(default)]
    pub supported_icaos: Vec<String>,
    /// Minimum host (`runway_selector_core`) semver required. Hosts older
    /// than this refuse to spawn the plugin.
    #[serde(default)]
    pub min_core_version: Option<Version>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    Rust,
    Python,
    Node,
    Deno,
}

/// Area-level runtime configuration. Ships in `area.toml`; users override
/// fields via `area.local.toml`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[skip_serializing_none]
#[serde(default)]
pub struct AreaConfig {
    /// VATSIM METAR feed URLs (e.g. `https://metar.vatsim.net/EN`).
    pub metar_urls: Vec<String>,
    /// Airports whose METARs are known-bad and should be dropped before parsing.
    pub ignore_airports: IndexSet<String>,
    /// Fallback runway selection used when neither ATIS nor METAR-derived wind
    /// logic produced a runway. Value is the runway heading in tens of
    /// degrees: `1` means runway 01, `28` means runway 28.
    pub default_runways: IndexMap<String, u8>,
    /// IANA timezone for "local time" logic inside the plugin (e.g.
    /// `Europe/Oslo`).
    pub time_zone: Option<String>,
    /// Prefix of the EuroScope sector file (`.sct`) that belongs to this area,
    /// e.g. `ENOR` for the Polaris FIR.
    pub sector_file_prefix: Option<String>,
}

/// A profile within an area — typically a controller position (TWR, APP,
/// RADAR) that picks which `.prf` file EuroScope opens and which extra
/// processes (TrackAudio, vACS, …) should be launched alongside.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[skip_serializing_none]
#[serde(default)]
pub struct ProfileConfig {
    pub name: String,
    pub display_name: String,
    /// `.prf` files inside the EuroScope config folder to open.
    pub prf_files: Vec<PathBuf>,
    /// Names of entries from `app_launchers.toml` to spawn alongside.
    pub default_apps: Vec<String>,
}

/// Top-level user configuration. Lives in `config.toml` directly under the
/// user's config dir — it controls how areas are installed, not how the
/// runway-selection logic behaves.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[skip_serializing_none]
#[serde(default)]
pub struct TopLevelConfig {
    pub area_registry_url: String,
    pub extra_registries: Vec<String>,
    pub auto_update_areas: bool,
    pub auto_install_mise_runtimes: bool,
    /// Override the directory where area packages are unpacked. Defaults to
    /// `<data_dir>/areas`.
    pub areas_install_dir: Option<PathBuf>,
}

pub const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/meltinglava/ENOR_Vatsim_Runway_Selector/main/areas.json";

impl Default for TopLevelConfig {
    fn default() -> Self {
        Self {
            area_registry_url: DEFAULT_REGISTRY_URL.to_string(),
            extra_registries: Vec::new(),
            auto_update_areas: true,
            auto_install_mise_runtimes: true,
            areas_install_dir: None,
        }
    }
}

/// Read `area.toml` from `area_dir`, apply `area.local.toml` if present, and
/// parse the merged value.
pub fn load_area_config(area_dir: &Path) -> AreaConfigResult<AreaConfig> {
    load_with_local_override(&area_dir.join("area.toml"))
}

/// Read a profile TOML at `profile_path`, apply its sibling `*.local.toml`,
/// and parse the merged value.
pub fn load_profile_config(profile_path: &Path) -> AreaConfigResult<ProfileConfig> {
    load_with_local_override(profile_path)
}

/// Read `manifest.toml` from `area_dir`. Manifests are not subject to local
/// overrides — area identity is fixed by the publisher.
pub fn load_area_manifest(area_dir: &Path) -> AreaConfigResult<AreaManifest> {
    let path = area_dir.join("manifest.toml");
    let raw = fs::read_to_string(&path).map_err(|source| AreaConfigError::Io {
        path: path.clone(),
        source,
    })?;
    toml::from_str(&raw).map_err(|e| AreaConfigError::Parse {
        path,
        message: e.to_string(),
    })
}

/// Read `<base_path>` and overlay `<base_path with .local.toml suffix>` on it.
/// Generic across the area/profile/top-level config types.
pub fn load_with_local_override<T>(base_path: &Path) -> AreaConfigResult<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    let mut value = if base_path.exists() {
        let raw = fs::read_to_string(base_path).map_err(|source| AreaConfigError::Io {
            path: base_path.to_path_buf(),
            source,
        })?;
        toml::from_str::<toml::Value>(&raw).map_err(|e| AreaConfigError::Parse {
            path: base_path.to_path_buf(),
            message: e.to_string(),
        })?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let local_path = local_path_for(base_path);
    if local_path.exists() {
        let raw = fs::read_to_string(&local_path).map_err(|source| AreaConfigError::Io {
            path: local_path.clone(),
            source,
        })?;
        let overrides =
            toml::from_str::<toml::Value>(&raw).map_err(|e| AreaConfigError::Parse {
                path: local_path.clone(),
                message: e.to_string(),
            })?;
        merge_local_overrides(&mut value, overrides);
    }

    value
        .try_into()
        .map_err(|e: toml::de::Error| AreaConfigError::Parse {
            path: base_path.to_path_buf(),
            message: e.to_string(),
        })
}

/// Compute the `<base>.local.toml` path for a config file. `foo.toml` becomes
/// `foo.local.toml`; a bare `foo` becomes `foo.local.toml` too.
pub fn local_path_for(base_path: &Path) -> PathBuf {
    let parent = base_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = base_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    parent.join(format!("{stem}.local.toml"))
}

/// Recursively overlay `overrides` onto `base`. Tables are merged key-by-key
/// so unrelated keys in `base` are preserved; every other value (scalar,
/// array, datetime) is replaced wholesale, since per-element list merging
/// would surprise users who expect their `.local.toml` value to win outright.
pub fn merge_local_overrides(base: &mut toml::Value, overrides: toml::Value) {
    match (base, overrides) {
        (toml::Value::Table(base_tbl), toml::Value::Table(override_tbl)) => {
            for (key, override_val) in override_tbl {
                match base_tbl.get_mut(&key) {
                    Some(base_val) => merge_local_overrides(base_val, override_val),
                    None => {
                        base_tbl.insert(key, override_val);
                    }
                }
            }
        }
        (slot, replacement) => *slot = replacement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_scalar() {
        let mut base = toml::from_str::<toml::Value>("x = 1\ny = 2").unwrap();
        let overrides = toml::from_str::<toml::Value>("x = 99").unwrap();
        merge_local_overrides(&mut base, overrides);
        assert_eq!(base["x"].as_integer(), Some(99));
        assert_eq!(base["y"].as_integer(), Some(2));
    }

    #[test]
    fn merge_replaces_array_wholesale() {
        let mut base = toml::from_str::<toml::Value>("list = [1, 2, 3]").unwrap();
        let overrides = toml::from_str::<toml::Value>("list = [10]").unwrap();
        merge_local_overrides(&mut base, overrides);
        let list: Vec<i64> = base["list"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_integer().unwrap())
            .collect();
        assert_eq!(list, vec![10]);
    }

    #[test]
    fn merge_recurses_into_nested_tables() {
        let mut base = toml::from_str::<toml::Value>(
            r#"
[default_runways]
ENGM = 1
ENBR = 17
"#,
        )
        .unwrap();
        let overrides = toml::from_str::<toml::Value>(
            r#"
[default_runways]
ENBR = 35
ENZV = 18
"#,
        )
        .unwrap();
        merge_local_overrides(&mut base, overrides);
        let merged = base["default_runways"].as_table().unwrap();
        assert_eq!(merged["ENGM"].as_integer(), Some(1));
        assert_eq!(merged["ENBR"].as_integer(), Some(35));
        assert_eq!(merged["ENZV"].as_integer(), Some(18));
    }

    #[test]
    fn area_config_round_trips_through_toml() {
        let raw = r#"
metar_urls = ["https://metar.vatsim.net/EN", "https://metar.vatsim.net/ESKS"]
ignore_airports = ["ENQC", "ENQR"]
sector_file_prefix = "ENOR"
time_zone = "Europe/Oslo"

[default_runways]
ENGM = 1
ENZV = 18
"#;
        let parsed: AreaConfig = toml::from_str(raw).unwrap();
        assert_eq!(parsed.metar_urls.len(), 2);
        assert!(parsed.ignore_airports.contains("ENQC"));
        assert_eq!(parsed.default_runways.get("ENGM").copied(), Some(1));
        assert_eq!(parsed.sector_file_prefix.as_deref(), Some("ENOR"));
    }

    #[test]
    fn local_path_for_appends_local_suffix() {
        assert_eq!(
            local_path_for(Path::new("/etc/area.toml")),
            PathBuf::from("/etc/area.local.toml"),
        );
        assert_eq!(
            local_path_for(Path::new("twr.toml")),
            PathBuf::from("twr.local.toml"),
        );
    }

    #[test]
    fn load_with_local_override_applies_overrides_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("area.toml");
        let local = dir.path().join("area.local.toml");
        fs::write(
            &base,
            r#"
metar_urls = ["https://metar.vatsim.net/EN"]
sector_file_prefix = "ENOR"

[default_runways]
ENGM = 1
ENZV = 18
"#,
        )
        .unwrap();
        fs::write(
            &local,
            r#"
[default_runways]
ENZV = 36
"#,
        )
        .unwrap();

        let cfg: AreaConfig = load_with_local_override(&base).unwrap();
        assert_eq!(cfg.metar_urls, vec!["https://metar.vatsim.net/EN"]);
        assert_eq!(cfg.default_runways.get("ENGM").copied(), Some(1));
        assert_eq!(cfg.default_runways.get("ENZV").copied(), Some(36));
    }

    #[test]
    fn load_with_local_override_works_without_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("area.toml");
        fs::write(&base, "metar_urls = []\n").unwrap();

        let cfg: AreaConfig = load_with_local_override(&base).unwrap();
        assert!(cfg.metar_urls.is_empty());
    }

    #[test]
    fn load_area_manifest_parses_well_formed_input() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("manifest.toml"),
            r#"
name = "enor"
version = "0.1.0"
display_name = "Polaris / ENOR"
runtime = "rust"
entry = "area_enor"
supported_icaos = ["ENGM", "ENZV"]
"#,
        )
        .unwrap();

        let manifest = load_area_manifest(dir.path()).unwrap();
        assert_eq!(manifest.name, "enor");
        assert_eq!(manifest.runtime, Runtime::Rust);
        assert_eq!(manifest.version, Version::new(0, 1, 0));
    }
}
