use std::{
    borrow::Cow,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

use config::{Config, ConfigError};
use directories::{BaseDirs, ProjectDirs, UserDirs};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jiff::{
    civil::{DateTime, datetime},
    tz::TimeZone,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use sysinfo::{ProcessesToUpdate, System};
use tokio::{process::Command, time::sleep};
use tracing::{debug, info, warn};
use tracing_unwrap::ResultExt;
use walkdir::WalkDir;

use crate::{airport::RunwayInUseSource, airports::Airports, error::ApplicationResult};

const DEFAULT_SECTOR_FILE_PREFIX: &str = "ENOR";

pub(crate) fn es_runway_selector_project_dir() -> ProjectDirs {
    ProjectDirs::from("", "meltinglava", "es_runway_selector")
        .expect("Failed to get project directories")
}

// ─── Plugin config ────────────────────────────────────────────────────────────

/// Configuration for one external area plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[skip_serializing_none]
pub(crate) struct PluginConfig {
    /// Unique name referenced from profiles.
    pub name: String,
    /// Command to run (e.g. `"es_runway_selector_area_enor"` or `"python main.py"`).
    pub command: String,
    /// Optional mise runtime spec (e.g. `"python@3.11"`).
    /// When set the plugin is launched as `mise exec <runtime> -- <command>`.
    pub runtime: Option<String>,
    /// Working directory for the plugin process.
    pub working_dir: Option<PathBuf>,
}

fn load_plugins(dir: &Path) -> Vec<PluginConfig> {
    let path = dir.join("plugins.toml");
    if !path.exists() {
        debug!(dir = %dir.display(), "No plugins.toml found");
        return Vec::new();
    }
    debug!(path = %path.display(), "Loading plugins");
    let raw = fs::read_to_string(&path).unwrap_or_log();
    let value: toml::Value = toml::from_str(&raw).unwrap_or_log();
    let Some(toml::Value::Array(arr)) = value.get("plugins") else {
        warn!(path = %path.display(), "plugins.toml has no [[plugins]] array");
        return Vec::new();
    };
    let mut plugins: Vec<PluginConfig> = arr
        .iter()
        .filter_map(|v| toml::Value::try_into(v.clone()).ok())
        .collect();
    // Resolve bare command filenames (no path separator) against the plugin dir.
    for plugin in &mut plugins {
        let cmd = Path::new(&plugin.command);
        if cmd.parent().map(|p| p == Path::new("")).unwrap_or(true) {
            let local = dir.join(&plugin.command);
            if local.exists() {
                let resolved = local.to_string_lossy().into_owned();
                debug!(
                    plugin = %plugin.name,
                    original = %plugin.command,
                    resolved = %resolved,
                    "Resolved plugin command to local path"
                );
                plugin.command = resolved;
            } else {
                debug!(
                    plugin = %plugin.name,
                    command = %plugin.command,
                    "Plugin command not found locally; will rely on PATH"
                );
            }
        }
    }
    debug!(count = plugins.len(), dir = %dir.display(), "Loaded plugins");
    plugins
}

// ─── Profile config ───────────────────────────────────────────────────────────

/// One named profile (sector file + per-FIR tuning).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[skip_serializing_none]
pub(crate) struct ProfileConfig {
    pub name: String,
    /// Filter for sector-file auto-detection. Defaults to `"ENOR"`.
    pub sector_file_prefix: Option<String>,
    /// Explicit folder containing the `.sct` / `.rwy` files.
    pub sector_file_dir: Option<PathBuf>,
    /// Airport ICAO codes to skip entirely.
    #[serde(default)]
    pub ignore_airports: IndexSet<String>,
    /// Default runway number per airport (e.g. `ENGM = 1` → runway "01").
    #[serde(default)]
    pub default_runways: IndexMap<String, u8>,
    /// Optional reference to a downloaded area config folder name.
    pub area: Option<String>,
}

fn load_profiles(config_dir: &Path) -> Vec<ProfileConfig> {
    let path = config_dir.join("profiles.toml");
    if !path.exists() {
        debug!(dir = %config_dir.display(), "No profiles.toml found");
        return Vec::new();
    }
    debug!(path = %path.display(), "Loading flat profiles");
    let raw = fs::read_to_string(&path).unwrap_or_log();
    let value: toml::Value = toml::from_str(&raw).unwrap_or_log();
    let Some(toml::Value::Array(arr)) = value.get("profiles") else {
        warn!(path = %path.display(), "profiles.toml has no [[profiles]] array");
        return Vec::new();
    };
    let profiles: Vec<ProfileConfig> = arr
        .iter()
        .filter_map(|v| toml::Value::try_into(v.clone()).ok())
        .collect();
    debug!(count = profiles.len(), "Loaded profiles from profiles.toml");
    profiles
}

// ─── Area-folder config ───────────────────────────────────────────────────────

/// Shared settings for an area, loaded from `config/<AREA>/area.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
#[skip_serializing_none]
struct AreaFileConfig {
    sector_file_prefix: Option<String>,
    sector_file_dir: Option<PathBuf>,
    #[serde(default)]
    ignore_airports: IndexSet<String>,
    #[serde(default)]
    default_runways: IndexMap<String, u8>,
}

/// One named profile override, from `[[profiles]]` in `config/<AREA>/profiles.toml`.
#[derive(Debug, Clone, Deserialize)]
struct AreaProfileEntry {
    name: String,
    /// Additional airports to ignore (merged with the area list).
    #[serde(default)]
    ignore_airports: IndexSet<String>,
    /// Per-profile default runway overrides (override area defaults).
    #[serde(default)]
    default_runways: IndexMap<String, u8>,
    /// Override the sector file directory for this profile only.
    sector_file_dir: Option<PathBuf>,
}

/// Returns `true` if `config_dir` contains at least one area subdirectory
/// (a subdirectory that has an `area.toml` file inside it).
fn has_area_directories(config_dir: &Path) -> bool {
    fs::read_dir(config_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .any(|e| e.path().is_dir() && e.path().join("area.toml").exists())
}

/// Scan area subdirectories under `config_dir` and build a flat list of `ProfileConfig` values.
///
/// If `only_area` is given, only that one area directory is processed (fast path when the
/// requested profile is already known).
///
/// - An area with no `profiles.toml` → one profile named after the area folder.
/// - An area with `profiles.toml` → one profile per entry, named `"<AREA>/<profile>"`.
fn load_area_based_profiles(config_dir: &Path, only_area: Option<&str>) -> Vec<ProfileConfig> {
    let mut profiles = Vec::new();

    let Ok(entries) = fs::read_dir(config_dir) else {
        warn!(dir = %config_dir.display(), "Cannot read config directory");
        return profiles;
    };

    let mut area_dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().is_dir()
                && e.path().join("area.toml").exists()
                && only_area
                    .map(|a| e.file_name().to_string_lossy().eq_ignore_ascii_case(a))
                    .unwrap_or(true)
        })
        .collect();
    area_dirs.sort_by_key(|e| e.file_name());

    debug!(count = area_dirs.len(), "Found area directories");

    for dir_entry in area_dirs {
        let area_dir = dir_entry.path();
        let area_name = dir_entry.file_name().to_string_lossy().to_string();

        debug!(area = %area_name, path = %area_dir.display(), "Processing area directory");

        let area_cfg: AreaFileConfig = match fs::read_to_string(area_dir.join("area.toml"))
            .ok()
            .and_then(|raw| toml::from_str(&raw).ok())
        {
            Some(cfg) => cfg,
            None => {
                warn!(area = %area_name, "area.toml missing or could not be parsed; using defaults");
                AreaFileConfig::default()
            }
        };

        let profiles_path = area_dir.join("profiles.toml");
        if profiles_path.exists() {
            // Multi-profile area: each entry gets a qualified name "<AREA>/<profile>".
            let area_profiles: Vec<AreaProfileEntry> = fs::read_to_string(&profiles_path)
                .ok()
                .and_then(|raw| toml::from_str::<toml::Value>(&raw).ok())
                .and_then(|v| v.get("profiles").cloned())
                .and_then(|arr| toml::Value::try_into(arr).ok())
                .unwrap_or_default();

            debug!(area = %area_name, count = area_profiles.len(), "Found profiles.toml with profile entries");

            for entry in area_profiles {
                let sector_file_dir = entry
                    .sector_file_dir
                    .or_else(|| area_cfg.sector_file_dir.clone());

                let mut ignore_airports = area_cfg.ignore_airports.clone();
                ignore_airports.extend(entry.ignore_airports);

                // Profile overrides take precedence over area defaults.
                let mut default_runways = area_cfg.default_runways.clone();
                for (icao, rwy) in entry.default_runways {
                    default_runways.insert(icao, rwy);
                }

                let profile_name = format!("{}/{}", area_name, entry.name);
                debug!(
                    profile = %profile_name,
                    sector_file_dir = ?sector_file_dir,
                    "Built profile"
                );
                profiles.push(ProfileConfig {
                    name: profile_name,
                    sector_file_prefix: area_cfg.sector_file_prefix.clone(),
                    sector_file_dir,
                    ignore_airports,
                    default_runways,
                    area: None,
                });
            }
        } else {
            // Single-profile area: profile is named after the area folder.
            debug!(
                profile = %area_name,
                sector_file_dir = ?area_cfg.sector_file_dir,
                "Built single profile for area"
            );
            profiles.push(ProfileConfig {
                name: area_name,
                sector_file_prefix: area_cfg.sector_file_prefix,
                sector_file_dir: area_cfg.sector_file_dir,
                ignore_airports: area_cfg.ignore_airports,
                default_runways: area_cfg.default_runways,
                area: None,
            });
        }
    }

    debug!(
        count = profiles.len(),
        "Built profiles from area directories"
    );
    profiles
}

// ─── Global (per-installation) config ─────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[skip_serializing_none]
struct GlobalConfigurable {
    // Legacy flat-profile fields – used when profiles.toml is absent.
    #[serde(default)]
    ignore_airports: IndexSet<String>,
    #[serde(default)]
    default_runways: IndexMap<String, u8>,
    euroscope_config_folder: Option<PathBuf>,

    euroscope_executable_path: Option<IndexMap<String, PathBuf>>,
    es_main_window_delay_ms: Option<u64>,

    /// Port for the parent HTTP API server. `0` picks a random free port.
    #[serde(default = "default_api_port")]
    pub api_port: u16,

    /// Name of the active plugin (must match a `name` entry in `plugins.toml`).
    /// Only one plugin is supported at a time. Leave unset to run without a plugin.
    pub plugin: Option<String>,
}

fn default_api_port() -> u16 {
    0
}

impl GlobalConfigurable {
    fn find_from_config(&self) -> Option<(PathBuf, String)> {
        let path = self.euroscope_config_folder.as_ref()?;
        search_for_sct_with_possibilities(&[path])
    }
}

// ─── App launchers ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Clone)]
pub(crate) struct AppLauncher {
    pub name: String,
    pub args: Vec<String>,
    pub prf: Option<PathBuf>,
}

// ─── ESConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct ESConfig {
    euroscope_config_folder: PathBuf,
    sector_file_prefix: String,
    #[allow(dead_code)]
    config_file_path: PathBuf,
    global: GlobalConfigurable,
    profile: ProfileConfig,
    pub(crate) all_plugins: Vec<PluginConfig>,
    app_launchers: IndexSet<AppLauncher>,
}

impl ESConfig {
    pub fn find_euroscope_config_folder(
        clean_config: bool,
        requested_profile: Option<&str>,
        plugin_override: Option<&str>,
    ) -> Option<Self> {
        let (mut global, config_file_path) = setup_configuration(clean_config).unwrap_or_log();
        let config_dir = config_file_path
            .parent()
            .expect("config file has no parent")
            .to_path_buf();

        info!(config_dir = %config_dir.display(), "Using config directory");

        // On fresh installs, auto-create an area folder for each sector file found.
        if !has_area_directories(&config_dir) && !config_dir.join("profiles.toml").exists() {
            info!(
                "No area config found; scanning EuroScope directories to auto-create area configs"
            );
            auto_create_area_configs(&config_dir);
        }

        let using_area_dirs = has_area_directories(&config_dir);
        let profiles = if using_area_dirs {
            // If the requested area is known, only load that one directory.
            let area_hint = requested_profile.map(|p| p.split('/').next().unwrap_or(p));
            let only_area = area_hint.filter(|a| config_dir.join(a).join("area.toml").exists());
            if let Some(area) = only_area {
                debug!(area = %area, "Loading profile for requested area only");
            } else {
                debug!("Loading all area profiles");
            }
            load_area_based_profiles(&config_dir, only_area)
        } else {
            debug!("Using flat profiles.toml");
            load_profiles(&config_dir)
        };

        let profile = if profiles.is_empty() {
            // No profiles configured – synthesise one from the legacy flat config.
            debug!("No profiles configured; falling back to legacy flat config");
            ProfileConfig {
                name: "default".to_string(),
                sector_file_prefix: None,
                sector_file_dir: global.euroscope_config_folder.clone(),
                ignore_airports: global.ignore_airports.clone(),
                default_runways: global.default_runways.clone(),
                area: None,
            }
        } else {
            select_profile(profiles, requested_profile)?
        };

        info!(
            profile = %profile.name,
            ignore_airports = ?profile.ignore_airports,
            "Active profile"
        );

        // Merge in downloaded area config if an `area` is referenced.
        let profile = merge_area_config(profile, &config_dir);

        // Derive the area directory from the profile name ("ENOR/TWR" → "ENOR").
        let area_name = profile
            .name
            .split('/')
            .next()
            .unwrap_or(&profile.name)
            .to_string();
        let area_dir = config_dir.join(&area_name);
        debug!(area_dir = %area_dir.display(), "Area directory");

        // Plugins: area-local plugins.toml takes precedence; fall back to root.
        let all_plugins = if area_dir.join("plugins.toml").exists() {
            debug!(area = %area_name, "Loading plugins from area directory");
            load_plugins(&area_dir)
        } else {
            debug!("No area-local plugins.toml; loading plugins from root config dir");
            load_plugins(&config_dir)
        };

        // Apply CLI override: --plugin takes precedence over config.toml.
        if let Some(name) = plugin_override {
            debug!(plugin = %name, "Plugin overridden from CLI");
            global.plugin = Some(name.to_string());
        }

        let active_plugin_name = global.plugin.as_deref();
        if all_plugins.is_empty() {
            debug!("No plugin definitions found in plugins.toml");
        } else {
            let names: Vec<&str> = all_plugins.iter().map(|p| p.name.as_str()).collect();
            debug!(defined = ?names, "Plugin definitions loaded from plugins.toml");
        }
        match active_plugin_name {
            Some(name) if all_plugins.iter().any(|p| p.name == name) => {
                info!(plugin = %name, "Active plugin");
            }
            Some(name) => {
                warn!(
                    plugin = %name,
                    "Plugin '{}' not found in plugins.toml — will run without a plugin",
                    name
                );
                global.plugin = None;
            }
            None => {
                if !all_plugins.is_empty() {
                    let names: Vec<&str> = all_plugins.iter().map(|p| p.name.as_str()).collect();
                    info!(
                        available = ?names,
                        "No plugin active. To activate one, set `plugin = \"<name>\"` in config.toml or pass `--plugin <name>`"
                    );
                } else {
                    debug!("Running without a plugin");
                }
            }
        }

        // Sector file discovery: profile's explicit dir > auto search (prefix filter).
        let prefix = profile
            .sector_file_prefix
            .clone()
            .unwrap_or_else(|| DEFAULT_SECTOR_FILE_PREFIX.to_string());

        debug!(prefix = %prefix, "Searching for sector file");

        let sct_result = if let Some(d) = &profile.sector_file_dir {
            debug!(dir = %d.display(), "Trying explicit sector_file_dir from profile");
            search_for_sct_with_possibilities(&[d])
        } else {
            None
        }
        .or_else(|| {
            debug!(
                "Searching standard EuroScope directories for prefix '{}'",
                prefix
            );
            search_for_newest_sct_file_with_prefix(&prefix)
        })
        .or_else(|| {
            if let Some(d) = &global.euroscope_config_folder {
                debug!(dir = %d.display(), "Trying euroscope_config_folder from global config");
            }
            global.find_from_config()
        });

        let (sct_path, sector_file_prefix) = match sct_result {
            Some(r) => {
                info!(
                    path = %r.0.display(),
                    prefix = %r.1,
                    "Sector file found"
                );
                r
            }
            None if using_area_dirs => {
                warn!(
                    profile = %profile.name,
                    area = %area_name,
                    "No sector file found. Add 'sector_file_dir' to {}/area.toml \
                     or place your EuroScope files in Documents/Euroscope.",
                    area_name
                );
                return None;
            }
            None => query_user_euroscope_config_folder(&mut global, &config_file_path)?,
        };

        // App launchers: area-local takes precedence; fall back to root config dir.
        let app_launchers = if area_dir.join("app_launchers.toml").exists() {
            debug!(area = %area_name, "Loading app launchers from area directory");
            get_app_launchers_from_dir(&area_dir)
        } else {
            debug!("Loading app launchers from root config dir");
            get_app_launchers_from_dir(&config_dir)
        };

        debug!(count = app_launchers.len(), "Loaded app launchers");

        Some(Self {
            euroscope_config_folder: sct_path,
            sector_file_prefix,
            global,
            profile,
            all_plugins,
            config_file_path,
            app_launchers,
        })
    }

    pub fn get_ignore_airports(&self) -> &IndexSet<String> {
        &self.profile.ignore_airports
    }

    pub fn get_default_runways(&self) -> &IndexMap<String, u8> {
        &self.profile.default_runways
    }

    /// Returns the active plugin config, if one is configured in `config.toml`.
    pub fn active_plugin_configs(&self) -> Vec<&PluginConfig> {
        match &self.global.plugin {
            Some(name) => match self.all_plugins.iter().find(|p| &p.name == name) {
                Some(plugin) => vec![plugin],
                None => vec![],
            },
            None => vec![],
        }
    }

    pub fn get_sct_file_path(&self) -> PathBuf {
        self.euroscope_config_folder
            .join(format!("{}.sct", self.sector_file_prefix))
    }

    pub fn get_rwy_file_path(&self) -> PathBuf {
        self.euroscope_config_folder
            .join(format!("{}.rwy", self.sector_file_prefix))
    }

    pub fn api_port(&self) -> u16 {
        self.global.api_port
    }

    pub fn write_runways_to_euroscope_rwy_file(
        &self,
        airports: &Airports,
    ) -> ApplicationResult<()> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .truncate(false)
            .open(self.get_rwy_file_path())?;

        let start_of_file = read_active_airport(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        write_runway_file(&mut file, airports, &start_of_file)
    }

    pub async fn run_apps(&self, euroscope_ready: bool) -> Vec<tokio::task::JoinHandle<()>> {
        let mut already_running = IndexMap::new();
        let mut first_euroscope_started = false;
        let mut handles = Vec::new();
        for app in &self.app_launchers {
            if (app.name == "EuroScope") == euroscope_ready {
                let entry = already_running
                    .entry(app.name.clone())
                    .or_insert_with(|| is_process_running(&app.name));
                if *entry {
                    debug!("{} is already running, skipping launch", app.name);
                    continue;
                }
                let app = app.clone();
                let exe_path = self
                    .global
                    .euroscope_executable_path
                    .clone()
                    .unwrap_or_default()
                    .get(&app.name)
                    .cloned()
                    .or_else(|| find_exe_path(&app.name));

                debug!("Found executable path for {}: {:?}", app.name, exe_path);
                let exe_path = match exe_path {
                    Some(p) => p,
                    None => {
                        warn!("Could not find executable path for {}", app.name);
                        continue;
                    }
                };
                let prf_path = self.euroscope_config_folder.clone();
                let es = app.name == "EuroScope";
                let pre_wait = if es && first_euroscope_started {
                    true
                } else {
                    first_euroscope_started = true;
                    false
                };
                let sleep_duration =
                    Duration::from_millis(self.global.es_main_window_delay_ms.unwrap_or(2000));
                handles.push(tokio::spawn(async move {
                    if pre_wait {
                        sleep(sleep_duration).await;
                    }
                    app.run(&exe_path, prf_path).await;
                }));
            }
        }
        handles
    }
}

fn select_profile(
    mut profiles: Vec<ProfileConfig>,
    requested: Option<&str>,
) -> Option<ProfileConfig> {
    if let Some(name) = requested {
        // Exact match first.
        if let Some(pos) = profiles
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
        {
            let p = profiles.swap_remove(pos);
            debug!(profile = %p.name, "Using requested profile");
            return Some(p);
        }
        // Area-prefix match: "ENOR" matches "ENOR/TWR", "ENOR/APP", etc.
        let prefix_lc = format!("{}/", name.to_ascii_lowercase());
        let mut area_matches: Vec<ProfileConfig> = profiles
            .into_iter()
            .filter(|p| p.name.to_ascii_lowercase().starts_with(&prefix_lc))
            .collect();
        if area_matches.is_empty() {
            warn!(profile = %name, "Requested profile not found");
            return None;
        }
        if area_matches.len() == 1 {
            let p = area_matches.pop().unwrap();
            debug!(profile = %p.name, "Auto-selected the only profile in requested area");
            return Some(p);
        }
        return select_from_multiple(area_matches);
    }

    if profiles.len() == 1 {
        let p = profiles.pop().unwrap();
        debug!(profile = %p.name, "Auto-selected the only configured profile");
        return Some(p);
    }

    select_from_multiple(profiles)
}

fn select_from_multiple(profiles: Vec<ProfileConfig>) -> Option<ProfileConfig> {
    let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();

    #[cfg(not(target_env = "musl"))]
    {
        use dialoguer::{Select, theme::ColorfulTheme};
        if let Ok(idx) = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Multiple profiles found — select one (↑↓ arrows, Enter to confirm)")
            .items(&names)
            .default(0)
            .interact()
        {
            let p = profiles.into_iter().nth(idx)?;
            debug!(profile = %p.name, "User selected profile interactively");
            return Some(p);
        }
    }

    // Fallback (musl builds or no TTY): pick the first and log it.
    let p = profiles.into_iter().next()?;
    debug!(profile = %p.name, "Auto-selected first available profile");
    Some(p)
}

/// Return the names of all configured profiles. Used by `--list-profiles`.
pub(crate) fn list_profiles(clean_config: bool) -> Vec<String> {
    let Ok((_, config_file_path)) = setup_configuration(clean_config) else {
        return Vec::new();
    };
    let config_dir = config_file_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let profiles = if has_area_directories(&config_dir) {
        load_area_based_profiles(&config_dir, None)
    } else {
        load_profiles(&config_dir)
    };
    profiles.into_iter().map(|p| p.name).collect()
}

/// Merge area config from `~/.config/es_runway_selector/areas/<name>/` into the profile.
fn merge_area_config(mut profile: ProfileConfig, config_dir: &Path) -> ProfileConfig {
    let area_name = match &profile.area {
        Some(a) => a.clone(),
        None => return profile,
    };
    let area_dir = config_dir.join("areas").join(&area_name);
    if !area_dir.exists() {
        warn!(area = %area_name, "Referenced area config directory not found; run --download-area to install it");
        return profile;
    }

    // Merge area's config.toml (ignore_airports + default_runways).
    let area_config_path = area_dir.join("config.toml");
    if area_config_path.exists() {
        #[derive(Deserialize, Default)]
        struct AreaConfig {
            #[serde(default)]
            ignore_airports: IndexSet<String>,
            #[serde(default)]
            default_runways: IndexMap<String, u8>,
        }
        if let Ok(raw) = fs::read_to_string(&area_config_path)
            && let Ok(ac) = toml::from_str::<AreaConfig>(&raw)
        {
            // Profile settings take precedence over area defaults.
            for icao in ac.ignore_airports {
                profile.ignore_airports.insert(icao);
            }
            for (icao, rwy) in ac.default_runways {
                profile.default_runways.entry(icao).or_insert(rwy);
            }
        }
    }

    profile
}

// ─── Process helpers ──────────────────────────────────────────────────────────

fn is_process_running(name: &str) -> bool {
    let mut sys = System::new_all();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let lower = name.to_lowercase();
    sys.processes_by_name(OsStr::new(name))
        .chain(sys.processes_by_name(OsStr::new(&lower)))
        .next()
        .is_some()
}

fn find_exe_path(name: &str) -> Option<PathBuf> {
    let start_menu_sub_folder = "Microsoft\\Windows\\Start Menu\\Programs";
    let start_menu_program_data =
        PathBuf::from(format!("C:\\ProgramData\\{}", start_menu_sub_folder));
    let start_menu_folders: &[PathBuf] = match directories::BaseDirs::new() {
        Some(f) => &[
            f.config_dir().join(start_menu_sub_folder),
            start_menu_program_data,
        ],
        None => &[start_menu_program_data],
    };
    start_menu_folders
        .iter()
        .flat_map(|p| {
            WalkDir::new(p)
                .max_depth(3)
                .into_iter()
                .filter_map(Result::ok)
        })
        .find(|e| {
            let file_name = e.file_name().to_string_lossy();
            let Some((exe_name, extention)) = file_name.split_once('.') else {
                return false;
            };
            exe_name == name && ["lnk", "exe"].contains(&extention)
        })
        .map(|e| e.path().to_path_buf())
}

impl AppLauncher {
    async fn run(&self, exe_path: &Path, prf_folder: PathBuf) {
        #[cfg(target_os = "windows")]
        let mut command = {
            let is_lnk = exe_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false);

            if is_lnk {
                let mut cmd = Command::new("cmd");
                cmd.arg("/c").arg("start").arg("").arg(exe_path);
                cmd
            } else {
                Command::new(exe_path)
            }
        };

        #[cfg(not(target_os = "windows"))]
        let mut command = Command::new(exe_path);

        for arg in &self.args {
            command.arg(arg);
        }
        if let Some(prf) = &self.prf {
            command.arg((prf_folder.join(prf)).to_string_lossy().to_string());
        }

        #[cfg(target_os = "windows")]
        {
            const DETACHED_PROCESS: u32 = 0x00000008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }

        #[cfg(not(target_os = "windows"))]
        {
            use std::process::Stdio;
            command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
        }

        debug!("Starting application: {:?}", command);
        if let Err(e) = command.spawn() {
            warn!(
                "Failed to launch application {}: {:?}",
                exe_path.to_string_lossy(),
                e
            );
        }
    }
}

fn get_app_launchers_from_dir(dir: &Path) -> IndexSet<AppLauncher> {
    let app_launchers_file_path = dir.join("app_launchers.toml");

    if !app_launchers_file_path.exists() {
        debug!(path = %app_launchers_file_path.display(), "No app_launchers.toml found");
        return IndexSet::new();
    }
    debug!(path = %app_launchers_file_path.display(), "Loading app launchers");

    let raw_app_launchers_file = fs::read_to_string(&app_launchers_file_path).unwrap_or_log();
    let toml_file: toml::Value = toml::from_str(&raw_app_launchers_file).unwrap_or_log();
    let toml::Value::Table(map) = &toml_file else {
        warn!(
            "App launchers config file is not a table, it is: {:?}",
            toml_file
        );
        return IndexSet::new();
    };
    let Some(array) = map.get("executable") else {
        warn!(
            "App launchers config file is not an array, it is: {:?}",
            toml_file
        );
        return IndexSet::new();
    };
    let toml::Value::Array(executables) = array else {
        warn!(
            "App launchers config file 'executable' is not an array, it is: {:?}",
            array
        );
        return IndexSet::new();
    };

    let mut app_launchers = IndexSet::new();
    for exe in executables {
        let toml::Value::Table(exe_table) = exe else {
            warn!("App launcher entry is not a table, it is: {:?}", exe);
            continue;
        };
        let name = match exe_table.get("name") {
            Some(toml::Value::String(s)) => s.clone(),
            _ => {
                warn!(
                    "App launcher entry missing 'name' field or it is not a string: {:?}",
                    exe_table
                );
                continue;
            }
        };
        let args = match exe_table.get("args") {
            Some(toml::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| {
                    if let toml::Value::String(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        let prf = match exe_table.get("prf") {
            Some(toml::Value::String(s)) => Some(PathBuf::from(s)),
            _ => None,
        };
        let launcher = AppLauncher { name, args, prf };
        debug!(
            name = %launcher.name,
            prf = ?launcher.prf,
            "Loaded app launcher"
        );
        app_launchers.insert(launcher);
    }

    debug!(count = app_launchers.len(), "Loaded app launchers");
    app_launchers
}

// ─── Sector file discovery ────────────────────────────────────────────────────

#[cfg(not(target_env = "musl"))]
fn query_user_euroscope_config_folder<P: AsRef<Path>>(
    config: &mut GlobalConfigurable,
    config_file_path: &P,
) -> Option<(PathBuf, String)> {
    let bd = BaseDirs::new()?;
    let possibility = rfd::FileDialog::new()
        .set_title("Select Euroscope sector file folder (contains .sct/.rwy)")
        .set_directory(bd.config_dir())
        .add_filter("Euroscope Configuration", &["sct", "rwy"])
        .pick_folder()
        .inspect(|path: &PathBuf| {
            config.euroscope_config_folder = Some(path.clone());
            fs::write(config_file_path, toml::to_string_pretty(&config).unwrap())
                .expect("Failed to write config file");
        })?;
    search_for_sct_with_possibilities(&[possibility])
}

#[cfg(target_env = "musl")]
fn query_user_euroscope_config_folder<P: AsRef<Path>>(
    _config: &mut GlobalConfigurable,
    _config_file_path: &P,
) -> Option<(PathBuf, String)> {
    warn!("Running in a musl environment, cannot query user for Euroscope config folder.");
    None
}

#[allow(unstable_name_collisions)]
pub fn read_active_airport<T: Read>(rwy_file: &mut T) -> io::Result<String> {
    let reader = BufReader::new(rwy_file);
    reader
        .lines()
        .take_while(|l| match l {
            Ok(l) => l.starts_with("ACTIVE_AIRPORT:"),
            Err(_) => false,
        })
        .intersperse_with(|| Ok("\n".to_string()))
        .collect::<io::Result<String>>()
}

fn setup_configuration(clean_config: bool) -> Result<(GlobalConfigurable, PathBuf), ConfigError> {
    let config_dir = es_runway_selector_project_dir().config_dir().to_path_buf();

    let mut raw_config_file = Cow::Borrowed(include_str!("../config.toml"));
    let config_file = config_dir.join("config.toml");
    if !config_file.exists() {
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        std::fs::write(&config_file, raw_config_file.as_bytes())
            .expect("Failed to create config file");
    }
    let configurable = Config::builder()
        .add_source(config::File::from(config_file.clone()).required(true))
        .build()
        .expect("Failed to build configuration")
        .try_deserialize::<GlobalConfigurable>()?;
    if clean_config {
        if let Some(path) = &configurable.euroscope_config_folder {
            raw_config_file = format!(
                "euroscope_config_folder = '{}'\n\n{}",
                path.to_string_lossy(),
                raw_config_file
            )
            .into();
        }
        fs::write(&config_file, raw_config_file.as_bytes())
            .expect("Failed to write cleaned config file");
        self::setup_configuration(false)
    } else {
        Ok((configurable, config_file))
    }
}

fn write_runway_file<T: Write>(
    rwy_file: &mut T,
    airports: &Airports,
    start_of_file: &str,
) -> ApplicationResult<()> {
    let mut writer = BufWriter::new(rwy_file);
    writeln!(writer, "{}", start_of_file)?;

    for airport in airports.airports.values() {
        if let Some(selection) = RunwayInUseSource::default_sort_order()
            .iter()
            .find_map(|method| airport.runways_in_use.get(method))
        {
            for (runway, usage) in selection {
                for flag in usage.active_runway_flags() {
                    writeln!(writer, "ACTIVE_RUNWAY:{}:{}:{}", airport.icao, runway, flag)?;
                }
            }
        }
    }

    Ok(())
}

fn euroscope_data_dirs() -> Vec<PathBuf> {
    let bd = BaseDirs::new();
    let ud = UserDirs::new();
    let mut dirs: Vec<PathBuf> = [
        bd.map(|d| d.config_dir().join("Euroscope")),
        ud.and_then(|d| d.document_dir().map(|d| d.join("Euroscope"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    dirs.push(PathBuf::from(format!(
        "/mnt/c/Users/{}/Documents/Euroscope",
        whoami::username().unwrap_or_log(),
    )));
    dirs.retain(|p| p.exists() && p.is_dir());
    dirs
}

fn search_for_newest_sct_file_with_prefix(prefix: &str) -> Option<(PathBuf, String)> {
    let mut dirs = euroscope_data_dirs();
    // Also check the _dev subfolder used by some EuroScope installations.
    dirs.extend(
        euroscope_data_dirs()
            .into_iter()
            .map(|d| d.join("Euroscope_dev"))
            .filter(|p| p.exists() && p.is_dir()),
    );
    search_for_sct_with_prefix_in_possibilities(&dirs, prefix)
}

fn extract_area_prefix(stem: &str) -> Option<String> {
    let prefix: String = stem
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if prefix.len() >= 2 {
        Some(prefix)
    } else {
        None
    }
}

fn auto_create_area_configs(config_dir: &Path) {
    let search_dirs = euroscope_data_dirs();
    if search_dirs.is_empty() {
        debug!("No EuroScope data directories found; skipping area auto-creation");
        return;
    }

    debug!(
        dirs = ?search_dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "Scanning EuroScope directories for sector files"
    );

    let mut found_prefixes: IndexSet<String> = IndexSet::new();
    for dir in &search_dirs {
        for entry in WalkDir::new(dir)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sct") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Some(prefix) = extract_area_prefix(stem)
            {
                debug!(file = %path.display(), prefix = %prefix, "Detected area prefix from sector file");
                found_prefixes.insert(prefix);
            }
        }
    }

    if found_prefixes.is_empty() {
        debug!("No sector files found; no area configs created");
        return;
    }

    debug!(prefixes = ?found_prefixes, "Detected area prefixes");

    for prefix in &found_prefixes {
        let area_dir = config_dir.join(prefix);
        if area_dir.exists() {
            debug!(area = %prefix, "Area directory already exists; skipping");
            continue;
        }
        if let Err(e) = fs::create_dir_all(&area_dir) {
            warn!(area = %prefix, error = %e, "Failed to create area directory");
            continue;
        }
        let content = format!(
            "sector_file_prefix = \"{prefix}\"\n\
             # The sector file is found automatically from your EuroScope installation.\n\
             # Uncomment and edit the lines below as needed:\n\
             \n\
             # ignore_airports = []\n\
             \n\
             # [default_runways]\n\
             # ENZV = 18\n"
        );
        if let Err(e) = fs::write(area_dir.join("area.toml"), &content) {
            warn!(area = %prefix, error = %e, "Failed to write area.toml");
        } else {
            info!(area = %prefix, path = %area_dir.display(), "Created area config directory");
        }
    }
}

fn search_for_sct_with_possibilities<P: AsRef<Path>>(
    possibilities: &[P],
) -> Option<(PathBuf, String)> {
    search_for_sct_with_prefix_in_possibilities(possibilities, DEFAULT_SECTOR_FILE_PREFIX)
}

fn search_for_sct_with_prefix_in_possibilities<P: AsRef<Path>>(
    possibilities: &[P],
    prefix: &str,
) -> Option<(PathBuf, String)> {
    let sct_files = possibilities
        .iter()
        .flat_map(|p| {
            WalkDir::new(p)
                .max_depth(1)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| {
                    let name = e.file_name().to_string_lossy();
                    let Some(extension) = e.path().extension() else {
                        return false;
                    };
                    name.starts_with(prefix) && extension == "sct"
                })
                .map(|e| e.path().to_path_buf())
        })
        .collect::<Vec<_>>();
    let file = sct_files.iter().max_by_key(get_sector_file_name_time)?;
    Some((
        file.parent()?.to_owned(),
        Path::new(file.file_name()?)
            .file_stem()?
            .to_string_lossy()
            .to_string(),
    ))
}

fn get_sector_file_name<P: AsRef<Path>>(path: &P) -> Option<String> {
    path.as_ref()
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
}

fn get_sector_file_name_time<P: AsRef<Path>>(path: &P) -> DateTime {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?<time>\d{14})").unwrap());

    if let Some(caps) = RE.captures(get_sector_file_name(path).as_deref().unwrap_or(""))
        && let Some(time_str) = caps.name("time")
        && let Ok(dt) = DateTime::strptime("%Y%m%d%H%M%S", time_str.as_str())
    {
        return dt;
    }
    path.as_ref()
        .metadata()
        .and_then(|m| m.created())
        .map(systemtime_to_jiff_datetime)
        .unwrap_or(datetime(1970, 1, 1, 0, 0, 0, 0))
}

fn systemtime_to_jiff_datetime(st: SystemTime) -> DateTime {
    let duration = st.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let ts = jiff::Timestamp::from_second(duration.as_secs() as i64).unwrap();
    let mut zoned = ts.to_zoned(TimeZone::system());
    zoned = zoned.with_time_zone(TimeZone::UTC);
    zoned.datetime()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    impl ESConfig {
        pub fn new_for_test() -> Self {
            let config_file = PathBuf::from("config.toml");
            let global: GlobalConfigurable = Config::builder()
                .add_source(config::File::from(config_file.clone()).required(true))
                .build()
                .expect("Failed to build configuration")
                .try_deserialize::<GlobalConfigurable>()
                .expect("Failed to deserialize configuration");
            let profile = ProfileConfig {
                name: "test".to_string(),
                sector_file_prefix: None,
                sector_file_dir: None,
                ignore_airports: global.ignore_airports.clone(),
                default_runways: global.default_runways.clone(),
                area: None,
            };
            Self {
                euroscope_config_folder: PathBuf::from("/test/path"),
                sector_file_prefix: "ENOR-Test".to_string(),
                global,
                profile,
                all_plugins: Vec::new(),
                config_file_path: config_file,
                app_launchers: IndexSet::new(),
            }
        }
    }

    #[test]
    fn test_read_active_airports() {
        let data = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0\nACTIVE_RUNWAY:ENZV:18:1\nACTIVE_RUNWAY:ENZV:18:0\n";
        let mut cursor = io::Cursor::new(data);
        let result = read_active_airport(&mut cursor).unwrap();
        let expected = "ACTIVE_AIRPORT:ENVA:1\nACTIVE_AIRPORT:ENBR:1\nACTIVE_AIRPORT:ENBO:0";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_get_sector_file_name() {
        let p = "ENOR-Norway-NC-DEV_20230403191923-230301-0004.sct";
        let dt = get_sector_file_name_time(&p);
        let target = DateTime::strptime("%Y%m%d%H%M%S", "20230403191923").unwrap();
        assert_eq!(dt, target);
    }

    #[test]
    fn test_app_launcher_reader() {
        let config_file = get_app_launchers_from_dir(Path::new("."));
        let mut expected = IndexSet::new();
        expected.insert(AppLauncher {
            name: "EuroScope".to_string(),
            args: vec![],
            prf: Some(PathBuf::from("enor_rads.prf")),
        });
        expected.insert(AppLauncher {
            name: "EuroScope".to_string(),
            args: vec![],
            prf: Some(PathBuf::from("enor_gnd.prf")),
        });
        expected.insert(AppLauncher {
            name: "TrackAudio".to_string(),
            args: vec![],
            prf: None,
        });
        expected.insert(AppLauncher {
            name: "vacs".to_string(),
            args: vec![],
            prf: None,
        });
        assert_eq!(config_file, expected);
    }
}
