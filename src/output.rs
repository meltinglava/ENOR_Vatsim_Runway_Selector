use std::io::{BufWriter, Write};

use crate::airports::Airports;
use crate::runway::RunwayUse;
use tracing::warn;

pub async fn write_runways_to_euroscope_rwy_file(path: &str, airports: &Airports) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = BufWriter::new(std::fs::File::create(path)?);

    for airport in airports.airports.values() {
        if airport.runways.is_empty() {
            warn!("No runways for airport {}", airport.icao);
            continue;
        }

        for (runway, usage) in &airport.runways_in_use {
            let flags = match usage {
                RunwayUse::Departing => vec![1],
                RunwayUse::Arriving => vec![0],
                RunwayUse::Both => vec![1, 0],
            };

            for flag in flags {
                writeln!(file, "ACTIVE_RUNWAY:{}:{}:{}", airport.icao, runway, flag)?;
            }
        }
    }

    Ok(())
}
