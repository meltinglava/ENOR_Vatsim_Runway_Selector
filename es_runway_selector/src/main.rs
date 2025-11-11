pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod atis;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod metar;
pub(crate) mod runway;
pub(crate) mod util;

use std::fs::File;

use airports::Airports;
use clap::Parser;
use config::ESConfig;
use error::ApplicationResult;
use self_update::{
    Status::{UpToDate, Updated},
    cargo_crate_version,
};
use tracing::{info, trace, warn};

#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[clap(long, short)]
    /// Resets the config file (but keeps the es folder information)
    clean_config: bool,
}

fn update() -> ApplicationResult<bool> {
    let update_status = self_update::backends::github::Update::configure()
        .repo_owner("meltinglava")
        .repo_name("ENOR_Vatsim_Runway_Selector")
        .bin_name("es_runway_selector")
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
    let config = ESConfig::find_euroscope_config_folder(cli.clean_config).unwrap();
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

    Ok(())
}

fn main() -> ApplicationResult<()> {
    tracing_subscriber::fmt::init();
    if !cfg!(debug_assertions) {
        let _ = update(); // dont fail if update fails
    }
    println!();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())?;
    Ok(())
}
