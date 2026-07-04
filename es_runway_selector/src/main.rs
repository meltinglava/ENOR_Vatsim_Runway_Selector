pub(crate) mod area_cli;
pub(crate) mod area_runtime;
pub(crate) mod config;
pub(crate) mod plugin_runner;
pub(crate) mod wizard;

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use config::ESConfig;
use indexmap::{IndexMap, IndexSet};
use jiff::{Zoned, tz::TimeZone};
use runway_selector_core::{Airports, output::write_runways_to_rwy_file};
use self_update::{
    Status::{UpToDate, Updated},
    cargo_crate_version,
};
use tracing::{info, trace, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[clap(long, short)]
    /// Resets the config file (but keeps the es folder information)
    clean_config: bool,
    #[clap(long, short)]
    /// Sets custom logging level for debugging for the json logs.
    /// (RUST_LOG env var still controls stdout)
    log_level: Option<String>,
    #[clap(long, hide = true)]
    previous_log_path: Option<PathBuf>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Manage installable area plugins (per-FIR runway selection logic)
    Area {
        #[command(subcommand)]
        cmd: area_cli::AreaCommand,
    },
}

fn get_target() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows-msvc"
    } else if cfg!(target_env = "musl") {
        "linux-musl"
    } else if cfg!(target_os = "linux") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin"
    } else {
        "unknown"
    }
}

fn update() -> Result<bool> {
    let update_status = self_update::backends::github::Update::configure()
        .repo_owner("meltinglava")
        .repo_name("ENOR_Vatsim_Runway_Selector")
        .bin_name("es_runway_selector")
        .show_output(true)
        .target(get_target())
        .show_download_progress(false)
        .current_version(cargo_crate_version!())
        .build()
        .context("Building self-update configuration")?
        .update()
        .context("Performing self-update")?;
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

/// Everything that must happen *before* the Tokio runtime exists: config
/// discovery can open a native folder-picker dialog, and blocking UI inside
/// the runtime freezes on Windows. Keep this synchronous.
struct PreparedStartup {
    config: Arc<ESConfig>,
    installed_areas: Vec<area_runtime::InstalledArea>,
}

fn prepare_startup(cli: &Cli) -> Result<PreparedStartup> {
    let top_level = area_cli::load_top_level_config().ok();
    let install_dir = top_level.as_ref().map(area_cli::resolved_install_dir);

    // Load installed areas first — they own the sector file prefix, METAR
    // URLs, ignore list, and default runways. The host no longer hardcodes
    // any of that.
    let installed_areas = match install_dir.as_deref() {
        Some(dir) => area_runtime::load_installed_areas(dir)
            .with_context(|| format!("Loading installed areas from {}", dir.display()))?,
        None => Vec::new(),
    };
    let installed_prefixes = area_runtime::installed_sector_file_prefixes(&installed_areas);

    let config = Arc::new(
        ESConfig::find_euroscope_config_folder(cli.clean_config, &installed_prefixes).ok_or_else(
            || {
                anyhow!(
                    "Could not locate a EuroScope sector file (looked for prefixes: {:?}). \
                     Install an area with `es_runway_selector area install <name>` or set \
                     `euroscope_config_folder` in your config.toml.",
                    installed_prefixes
                )
            },
        )?,
    );

    // First-run wizard: tell the user what to install if they haven't yet.
    // Always informational — never blocks the main flow.
    if let Some(dir) = install_dir.as_deref() {
        match wizard::detect_setup_state(dir, Some(config.get_sector_file_prefix())) {
            Ok(state) => wizard::print_setup_state(&state),
            Err(e) => warn!(error = ?e, "Setup-state detection failed"),
        }
    }

    Ok(PreparedStartup {
        config,
        installed_areas,
    })
}

async fn run(prepared: PreparedStartup) -> Result<()> {
    let PreparedStartup {
        config,
        installed_areas,
    } = prepared;

    // Host-side configuration (METAR feeds, ignore list, defaults) comes from
    // the area whose sector_file_prefix owns this sector file. Runway
    // *selection* below runs through every installed area plugin.
    let active_area =
        area_runtime::match_area_for_prefix(&installed_areas, config.get_sector_file_prefix());
    if active_area.is_none() {
        warn!(
            sector_file_prefix = config.get_sector_file_prefix(),
            "No installed area declares a sector_file_prefix that matches; \
             selections will use defaults only"
        );
    }

    // Anything area-derived comes from the active area's area.toml, never
    // from host config.toml.
    static EMPTY_IGNORE: std::sync::OnceLock<IndexSet<String>> = std::sync::OnceLock::new();
    static EMPTY_DEFAULTS: std::sync::OnceLock<IndexMap<String, u8>> = std::sync::OnceLock::new();
    let ignore_airports = active_area
        .map(|a| &a.config.ignore_airports)
        .unwrap_or_else(|| EMPTY_IGNORE.get_or_init(IndexSet::new));
    let default_runways = active_area
        .map(|a| &a.config.default_runways)
        .unwrap_or_else(|| EMPTY_DEFAULTS.get_or_init(IndexMap::new));
    let metar_urls: Vec<&str> = active_area
        .map(|a| a.config.metar_urls.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let config_task1 = config.clone();
    let task1 = tokio::spawn(async move {
        let handles = config_task1.run_apps(false).await;
        for handle in handles {
            handle.await.unwrap();
        }
    });
    let mut airports = Airports::new();
    let sct_path = config.get_sct_file_path();
    let mut sct_file = File::open(&sct_path)
        .with_context(|| format!("Opening sector file {}", sct_path.display()))?;
    airports
        .load_airports_from_sector_file(&mut sct_file, ignore_airports)
        .with_context(|| format!("Parsing sector file {}", sct_path.display()))?;
    if metar_urls.is_empty() {
        warn!("Active area declares no METAR URLs; skipping METAR fetch");
    } else if let Err(e) = airports.add_metars(&metar_urls, ignore_airports).await {
        warn!(error = ?e, "METAR fetch failed; continuing without METAR-derived selections");
    }
    if let Err(e) = airports.read_atis_and_apply_runways().await {
        warn!(error = ?e, "ATIS fetch failed; continuing without ATIS-derived selections");
    }

    // Hand selection off to the installed area plugins. ATIS-derived runways
    // are already applied host-side; plugins only see the remaining airports.
    // Failures degrade to defaults and are surfaced to the user.
    let statuses = plugin_runner::run_area_selections(&mut airports, &installed_areas).await;
    for status in &statuses {
        if matches!(status.outcome, plugin_runner::AreaRunOutcome::Failed(_)) {
            eprintln!("WARNING: {}", status.user_message());
        }
    }

    airports.apply_default_runways(default_runways);
    airports.sort();
    let rwy_path = config.get_rwy_file_path();
    write_runways_to_rwy_file(&rwy_path, &airports)
        .with_context(|| format!("Writing runway file {}", rwy_path.display()))?;
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
    airports
        .make_runway_report_html()
        .context("Generating HTML runway report")?;

    for task in tasks {
        task.await.context("Joining background app-launcher task")?;
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
            // Clean up log files older than 14 days
            cleanup_old_logs(&log_dir, 14)?;

            // Timestamp in Zulu (UTC)
            let now_utc = Zoned::now().with_time_zone(TimeZone::UTC);
            let ts_str = now_utc.strftime("%Y%m%d-%H%M%SZ");

            let file_name = format!("es_runway_selector-{ts_str}.json");
            log_dir.join(file_name)
        }
    };

    let file = std::fs::File::create(&file_path)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    // Stdout logger controlled by RUST_LOG
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_thread_ids(true)
        .with_thread_names(true)
        // filter MUST be last so we still have access to the fmt::Layer methods above
        .with_filter(EnvFilter::from_default_env());

    // JSON logger to timestamped file, has its own filter
    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        // again, filter last
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

fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow!("Failed to install ring crypto provider"))?;
    let mut cli = Cli::parse();
    let (log_file_path, _guard) = setup_logging(&cli).context("Setting up logging")?;
    info!("ES Runway Selector version {}", cargo_crate_version!());
    if !cfg!(debug_assertions) && cli.previous_log_path.is_none() {
        match update() {
            Ok(true) => {
                info!("Update check completed, restarting application to new version");
                let mut args = std::env::args();
                let application = args
                    .next()
                    .context("argv[0] missing — cannot determine self path to restart")?;
                let mut args_for_next = vec![
                    "--previous-log-path".to_string(),
                    log_file_path.to_string_lossy().to_string(),
                ];
                args_for_next.extend(args);
                let mut result = std::process::Command::new(&application)
                    .args(&args_for_next)
                    .spawn()
                    .with_context(|| format!("Restarting {application} after self-update"))?;
                std::process::exit(
                    result
                        .wait()
                        .context("Waiting for restarted application to exit")?
                        .code()
                        .unwrap_or(-1),
                );
            }
            Ok(false) => {
                info!("Update check completed, application is up to date");
            }
            Err(e) => warn!("Update check failed: {0:#}", e),
        }
    }
    println!();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Building tokio runtime")?;
    let command = cli.command.take();
    match command {
        Some(Command::Area { cmd }) => runtime
            .block_on(area_cli::run_area_command(cmd))
            .context("Running area subcommand")?,
        None => {
            // Config discovery may open a folder-picker dialog; run it before
            // entering the runtime so blocking UI cannot freeze the reactor
            // (this deadlocked on Windows when done inside block_on).
            let prepared = prepare_startup(&cli).context("Preparing startup")?;
            runtime
                .block_on(run(prepared))
                .context("Running runway selector")?
        }
    }
    Ok(())
}
