pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod metar;
pub(crate) mod atis;
pub(crate) mod runway;
pub(crate) mod util;
pub(crate) mod config;
pub(crate) mod error;


use std::fs::File;
use airports::Airports;
use config::ESConfig;
use error::ApplicationResult;

#[tokio::main]
async fn main() -> ApplicationResult<()> {
    let config = ESConfig::find_euroscope_config_folder().unwrap();
    let mut airports = Airports::new();
    let mut sct_file = File::open(config.get_sct_file_path()).unwrap();
    airports.fill_known_airports(&mut sct_file, &config)?;
    airports.add_metars().await;
    airports.read_atises().await.unwrap();
    airports.select_runways_in_use(&config);
    airports.apply_default_runways(&config);
    airports.sort();
    config.write_runways_to_euroscope_rwy_file(&airports).unwrap();

    let no_runways_in_use = airports.airports_without_runway_config();
    dbg!(no_runways_in_use);
    dbg!(&airports["ENVA"]);
    Ok(())
}
