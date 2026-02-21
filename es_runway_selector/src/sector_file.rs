use std::io::Read;

use encoding::{
    DecoderTrap, Encoding,
    all::{ISO_8859_1, UTF_8},
};
use indexmap::{IndexMap, IndexSet};

use crate::{
    airport::Airport,
    error::{ApplicationError, ApplicationResult},
    runway::{Runway, RunwayDirection},
};

pub(crate) fn load_airports_from_sct_runway_section<R: Read>(
    reader: &mut R,
    ignored_airports: &IndexSet<String>,
) -> ApplicationResult<IndexMap<String, Airport>> {
    let sct_file = read_with_encodings(reader)?;
    let mut airports = IndexMap::new();

    for line in sct_file
        .lines()
        .skip_while(|line| *line != "[RUNWAY]")
        .skip(1)
        .take_while(|line| !line.is_empty())
    {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }

        let icao = parts[parts.len() - 1];
        if ignored_airports.contains(icao) {
            continue;
        }

        let airport = airports.entry(icao.to_string()).or_insert_with(|| Airport {
            icao: icao.to_string(),
            metar: None,
            runways: Vec::new(),
            runways_in_use: IndexMap::new(),
        });

        let runway = Runway {
            runways: [
                RunwayDirection {
                    degrees: parts[2].parse()?,
                    identifier: parts[0].into(),
                },
                RunwayDirection {
                    degrees: parts[3].parse()?,
                    identifier: parts[1].into(),
                },
            ],
        };
        airport.runways.push(runway);
    }

    Ok(airports)
}

fn read_with_encodings<R: Read>(reader: &mut R) -> ApplicationResult<String> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;

    let utf8_decoded = UTF_8.decode(&buffer, DecoderTrap::Strict);

    match utf8_decoded {
        Ok(text) => Ok(text),
        Err(e) => ISO_8859_1
            .decode(&buffer, DecoderTrap::Strict)
            .map_err(|_| ApplicationError::EncodingError(e.to_string())),
    }
}
