pub(crate) mod airport;
pub(crate) mod airports;
pub(crate) mod metar;
pub(crate) mod atis;
pub(crate) mod runway;
pub(crate) mod output;
pub(crate) mod util;

use airports::Airports;
use output::write_runways_to_euroscope_rwy_file;

#[tokio::main]
async fn main() {
    let mut airports = Airports::new();
    airports.fill_known_airports();
    airports.add_metars().await;
    airports.read_atises().await.unwrap();
    airports.select_runways_in_use();
    airports.apply_default_runways();
    write_runways_to_euroscope_rwy_file("ouput.rwy", &airports).await.unwrap();

    let no_runways_in_use = airports.airports_without_runway_config();
    dbg!(no_runways_in_use);
}

