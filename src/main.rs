pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod metar;
pub(crate) mod atis;
pub(crate) mod runway;
pub(crate) mod output;
pub(crate) mod util;
mod config;

use std::fs::File;

use airports::Airports;
use config::Config;
use output::write_runways_to_euroscope_rwy_file;

#[tokio::main]
async fn main() {
    let config = Config::find_euroscope_config_folder().unwrap();
    let mut airports = Airports::new();
    let mut sct_file = File::open(config.get_sct_file_path()).unwrap();
    airports.fill_known_airports(&mut sct_file);
    airports.add_metars().await;
    airports.read_atises().await.unwrap();
    airports.select_runways_in_use();
    airports.apply_default_runways();
    write_runways_to_euroscope_rwy_file("ouput.rwy", &airports).await.unwrap();

    let no_runways_in_use = airports.airports_without_runway_config();
    dbg!(no_runways_in_use);
}

