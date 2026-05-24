pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod api_server;
pub(crate) mod area_config;
pub(crate) mod atis_parser;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod metar;
pub(crate) mod plugin_manager;
pub(crate) mod protocol_convert;
pub(crate) mod report_builder;
pub(crate) mod runway;
pub(crate) mod sector_file;
pub(crate) mod util;

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use airports::Airports;
use clap::Parser;
use config::ESConfig;
use error::ApplicationResult;
use jiff::{Zoned, tz::TimeZone};
use plugin_manager::PluginManager;
use self_update::{
    Status::{UpToDate, Updated},
    cargo_crate_version,
};
use tracing::{info, trace, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_unwrap::{OptionExt, ResultExt};

use crate::area_config::{download_area, list_manifest_areas, load_area_entries};

#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[clap(long, short)]
    /// Resets the config file (but keeps the es folder information).
    clean_config: bool,

    #[clap(long, short)]
    /// Sets custom logging level for debugging for the json logs.
    /// (RUST_LOG env var still controls stdout)
    log_level: Option<String>,

    #[clap(long, hide = true)]
    previous_log_path: Option<PathBuf>,

    /// Select a profile by name.
    /// Use the area name ("ENOR") or the qualified name ("ENOR/TWR").
    /// When omitted the single profile is used automatically.
    #[clap(long, short)]
    profile: Option<String>,

    /// Override the active plugin by name (must match a [[plugins]] entry in plugins.toml).
    /// Takes precedence over the `plugin` setting in config.toml.
    #[clap(long)]
    plugin: Option<String>,

    /// Write the combined plugin + parent OpenAPI spec to `openapi.json`
    /// in the current directory and exit.
    #[clap(long)]
    generate_openapi: bool,

    /// Download (or update) the named area config package and exit.
    /// The area must be listed in `areas.toml`.
    #[clap(long, value_name = "AREA")]
    download_area: Option<String>,

    /// List areas available in all configured manifest sources and exit.
    #[clap(long)]
    list_areas: bool,

    /// List all configured profiles and exit.
    #[clap(long)]
    list_profiles: bool,
}

fn get_target() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows-msvc"
    } else if cfg!(target_env = "musl") {
        "linux-musl"
    } else if cfg!(target_os = "linux") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "unknown"
    }
}

fn update() -> ApplicationResult<bool> {
    let update_status = self_update::backends::github::Update::configure()
        .repo_owner("meltinglava")
        .repo_name("ENOR_Vatsim_Runway_Selector")
        .bin_name("es_runway_selector")
        .show_output(true)
        .target(get_target())
        .show_download_progress(false)
        .current_version(cargo_crate_version!())
        .build()?
        .update()?;
    Ok(match update_status {
        UpToDate(v) => {
            trace!("Version: {} is up to date", v);
            false
        }
        Updated(v) => {
            info!("Updated to version: {}", v);
            true
        }
    })
}

async fn run(cli: Cli, config: Arc<ESConfig>) -> ApplicationResult<()> {
    let config_dir = config::es_runway_selector_project_dir()
        .config_dir()
        .to_path_buf();
    let area_entries = load_area_entries(&config_dir);

    // ── --list-areas ──────────────────────────────────────────────────────────
    if cli.list_areas {
        use area_config::AreaSource;
        let manifest_urls: Vec<&str> = area_entries
            .iter()
            .filter_map(|e| {
                if let AreaSource::Manifest { url, .. } = &e.source {
                    Some(url.as_str())
                } else {
                    None
                }
            })
            .collect();
        if manifest_urls.is_empty() {
            println!("No manifest sources configured in areas.toml.");
        }
        for url in manifest_urls {
            list_manifest_areas(url).await?;
        }
        println!("\nLocal entries in areas.toml:");
        for entry in &area_entries {
            println!("  {}", entry.name);
        }
        return Ok(());
    }

    // ── --download-area ───────────────────────────────────────────────────────
    if let Some(area_name) = &cli.download_area {
        let entry = area_entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(area_name))
            .ok_or_else(|| {
                error::ApplicationError::AreaConfigError(format!(
                    "Area '{area_name}' not found in areas.toml"
                ))
            })?;
        download_area(entry, &config_dir).await?;
        println!("Area '{}' downloaded successfully.", entry.name);
        return Ok(());
    }

    // ── Normal runway selection run ───────────────────────────────────────────

    // Start our helper API server first so plugins can call back to us.
    let parent_port = api_server::start(config.api_port()).await?;

    // Start configured plugins.
    let plugin_configs = config.active_plugin_configs();
    let plugins = PluginManager::start(&plugin_configs, parent_port).await?;

    // Launch non-EuroScope apps (TrackAudio, vacs, etc.) immediately.
    let config_task1 = config.clone();
    let task1 = tokio::spawn(async move {
        let handles = config_task1.run_apps(false).await;
        for handle in handles {
            handle.await.unwrap();
        }
    });

    let mut airports = Airports::new();
    let mut sct_file = File::open(config.get_sct_file_path()).unwrap();
    airports.load_airports_from_sector_file(&mut sct_file, &config)?;
    airports.add_metars(&config).await;
    airports
        .read_atis_and_apply_runways(&plugins)
        .await
        .unwrap();
    airports.select_runway_in_use(&config, &plugins).await;
    airports.sort();
    config
        .write_runways_to_euroscope_rwy_file(&airports)
        .unwrap_or_log();

    // Now launch EuroScope.
    let task2 = tokio::spawn(async move {
        let handles = config.run_apps(true).await;
        for handle in handles {
            handle.await.unwrap();
        }
    });

    let tasks = [task1, task2];

    let no_runways_in_use = airports.airports_without_runway_config();
    for airport in no_runways_in_use {
        if airport.metar.is_none() {
            warn!(airport.icao, metar = "No METAR / unparsable metar", ?airport.runways, "No runway selected for:")
        }
    }
    airports.make_runway_report_html()?;

    for task in tasks {
        task.await?;
    }

    Ok(())
}

fn setup_logging(cli: &Cli) -> std::io::Result<(PathBuf, WorkerGuard)> {
    let log_dir = config::es_runway_selector_project_dir()
        .data_dir()
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;

    let file_path = match &cli.previous_log_path {
        Some(path) => path.to_path_buf(),
        None => {
            cleanup_old_logs(&log_dir, 14)?;
            let now_utc = Zoned::now().with_time_zone(TimeZone::UTC);
            let ts_str = now_utc.strftime("%Y%m%d-%H%M%SZ");
            let file_name = format!("es_runway_selector-{ts_str}.json");
            log_dir.join(file_name)
        }
    };

    let file = std::fs::File::create(&file_path)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_filter(EnvFilter::from_default_env());

    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_filter(EnvFilter::new(
            cli.log_level
                .as_deref()
                .unwrap_or("info,es_runway_selector=trace,reqwest=debug"),
        ));

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(json_layer)
        .try_init()
        .expect("Failed to initialize logging subscriber");

    Ok((file_path, guard))
}

fn cleanup_old_logs(dir: &Path, max_age_days: u64) -> std::io::Result<()> {
    let max_age = Duration::from_secs(max_age_days * 24 * 60 * 60);
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let metadata = entry.metadata()?;
        if let Ok(modified) = metadata.modified()
            && modified < cutoff
        {
            let _ = std::fs::remove_file(&path);
        }
    }

    Ok(())
}

fn main() -> ApplicationResult<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install ring crypto provider");
    let cli = Cli::parse();
    let (log_file_path, _guard) = setup_logging(&cli).expect("failed to set up logging");
    info!("ES Runway Selector version {}", cargo_crate_version!());
    if !cfg!(debug_assertions) && cli.previous_log_path.is_none() {
        match update() {
            Ok(true) => {
                info!("Update check completed, restarting application to new version");
                let mut args = std::env::args();
                let application = args.next().unwrap_or_log();
                let mut args_for_next = vec![
                    "--previous-log-path".to_string(),
                    log_file_path.to_string_lossy().to_string(),
                ];
                args_for_next.extend(args);
                let mut result = std::process::Command::new(application)
                    .args(&args_for_next)
                    .spawn()
                    .expect("Failed to restart application");
                std::process::exit(
                    result
                        .wait()
                        .expect("Failed to wait for new application")
                        .code()
                        .unwrap_or(-1),
                );
            }
            Ok(false) => {
                info!("Update check completed, application is up to date");
            }
            Err(e) => warn!("Update check failed: {0}, {0:?}", e),
        }
    }
    // ── --generate-openapi ────────────────────────────────────────────────────
    if cli.generate_openapi {
        let json = api_server::generate_openapi_json();
        std::fs::write("openapi.json", &json)?;
        info!("OpenAPI spec written to openapi.json");
        println!("openapi.json written ({} bytes)", json.len());
        return Ok(());
    }

    // ── --list-profiles ───────────────────────────────────────────────────────
    if cli.list_profiles {
        let profiles = config::list_profiles(cli.clean_config);
        if profiles.is_empty() {
            println!("No profiles configured.");
            println!("Run the selector once to auto-detect areas from your EuroScope files,");
            println!("or create a config/<AREA>/area.toml manually.");
        } else {
            println!("Available profiles (use -p <NAME> to select):");
            for name in &profiles {
                println!("  {name}");
            }
        }
        return Ok(());
    }

    // ── Profile selection + config loading ────────────────────────────────────
    // Must happen before the Tokio runtime starts: dialoguer needs exclusive
    // access to the console, which Tokio's signal/IO setup on Windows disrupts.
    println!();
    let config = Arc::new(
        ESConfig::find_euroscope_config_folder(
            cli.clean_config,
            cli.profile.as_deref(),
            cli.plugin.as_deref(),
        )
        .unwrap_or_log(),
    );

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli, config))?;
    Ok(())
}
