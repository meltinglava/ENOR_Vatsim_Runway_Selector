pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod atis;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod metar;
pub(crate) mod runway;
pub(crate) mod util;

use std::{fs::File, sync::Arc};

use airports::Airports;
use clap::Parser;
use config::ESConfig;
use error::ApplicationResult;
use self_update::{
    Status::{UpToDate, Updated},
    cargo_crate_version,
};
use tracing::{info, trace, warn};
use tracing_unwrap::OptionExt;

#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[clap(long, short)]
    /// Resets the config file (but keeps the es folder information)
    clean_config: bool,
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

async fn run() -> ApplicationResult<()> {
    let cli = Cli::parse();
    let config = Arc::new(ESConfig::find_euroscope_config_folder(cli.clean_config).unwrap_or_log());
    let config_task1 = config.clone();
    let task1 = tokio::spawn(async move {
        let handles = config_task1.run_apps(false).await;
        for handle in handles {
            handle.await.unwrap();
        }
    });
    let mut airports = Airports::new();
    let mut sct_file = File::open(config.get_sct_file_path()).unwrap();
    airports.fill_known_airports(&mut sct_file, &config)?;
    airports.add_metars(&config).await;
    airports.read_atises_and_apply_runways().await.unwrap();
    airports.runway_in_use_based_on_metar(&config);
    airports.apply_default_runways(&config);
    airports.sort();
    config
        .write_runways_to_euroscope_rwy_file(&airports)
        .unwrap();
    let task2 = tokio::spawn(async move {
        let handles = config.run_apps(true).await;
        for handle in handles {
            handle.await.unwrap();
        }
    });

    let tasks = [task1, task2];

    let no_runways_in_use = airports.airports_without_runway_config();
    for airport in no_runways_in_use {
        match &airport.metar {
            Some(metar) => {
                warn!(airport.icao, metar = ?metar.raw, ?airport.runways, "No runway selected for:")
            }
            None => {
                warn!(airport.icao, metar = "No METAR / unparsable metar", ?airport.runways, "No runway selected for:")
            }
        }
    }
    println!();

    airports.make_runway_report();

    for task in tasks {
        task.await?;
    }

    Ok(())
}

fn main() -> ApplicationResult<()> {
    tracing_subscriber::fmt::init();
    if !cfg!(debug_assertions) {
        match update() {
            Ok(_) => (),
            Err(e) => warn!("Update check failed: {0}, {0:?}", e),
        }
    }
    println!();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())?;
    Ok(())
}
