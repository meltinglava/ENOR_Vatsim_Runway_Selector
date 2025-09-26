use indexmap::{IndexMap, IndexSet};
use encoding::{
    all::{ISO_8859_1, UTF_8},
    DecoderTrap, Encoding,
};

use std::{io::Read, ops::{Index, IndexMut}};

use crate::{airport::Airport, config::ESConfig, error::ApplicationResult};
use crate::atis::find_runway_in_use_from_atis;
use crate::runway::{Runway, RunwayDirection, RunwayUse};
use crate::metar::get_metars;

pub struct Airports {
    pub airports: IndexMap<String, Airport>,
}

impl Airports {
    pub fn new() -> Self {
        Self {
            airports: IndexMap::new(),
        }
    }

    #[allow(dead_code)] // used in tests
    pub fn add_airport(&mut self, airport: Airport) {
        self.airports.insert(airport.icao.clone(), airport);
    }

    pub fn fill_known_airports<R: Read>(&mut self, reader: &mut R, config: &ESConfig) -> ApplicationResult<()> {
        let sct_file = read_with_encoings(reader).expect("Failed to read SCT file");
        let ignored_set = config.get_ignore_airports();

        for line in sct_file.lines().skip_while(|line| *line != "[RUNWAY]").skip(1).take_while(|line| !line.is_empty()) {
            let parts: Vec<_> = line.split_whitespace().collect();
            let icao = *parts.last().unwrap();
            if ignored_set.contains(icao) {
                continue;
            }

            let airport = self.airports.entry(icao.to_string()).or_insert_with(|| Airport {
                icao: icao.to_string(),
                metar: None,
                runways: Vec::new(),
                runways_in_use: IndexMap::new(),
            });

            let runway = Runway {
                runways: [
                    RunwayDirection { degrees: parts[2].parse()?, identifier: parts[0].into() },
                    RunwayDirection { degrees: parts[3].parse()?, identifier: parts[1].into() },
                ],
            };
            airport.runways.push(runway);
        }
        Ok(())
    }

    pub async fn add_metars(&mut self) {
        let metars = get_metars().await.unwrap();
        for metar in metars {
            if let Some(airport) = self.airports.get_mut(&metar.icao) {
                airport.metar = Some(metar);
            }
        }
    }

    pub async fn read_atises(&mut self) -> ApplicationResult<()> {
        let icaos = self.identifiers();
        let data = reqwest::get("https://data.vatsim.net/v3/vatsim-data.json")
            .await?
            .json::<serde_json::Value>()
            .await?;

        let serde_json::Value::Object(map) = data else { return Ok(()); };
        let Some(serde_json::Value::Array(atises)) = map.get("atis") else { return Ok(()); };

        for atis in atises {
            let serde_json::Value::Object(atis) = atis else { continue; };
            let Some(serde_json::Value::String(callsign)) = atis.get("callsign") else { continue; };
            let icao = &callsign[0..4];

            if !icaos.contains(icao) {
                continue;
            }

            let Some(airport) = self.airports.get_mut(icao) else { continue; };
            let Some(serde_json::Value::Array(atis_lines)) = atis.get("text_atis") else { continue; };

            let text = atis_lines.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ");
            for (runway, config) in find_runway_in_use_from_atis(&text) {
                airport.runways_in_use.entry(runway).and_modify(|e| {
                    *e = match (*e, config) {
                        (RunwayUse::Both, _) | (_, RunwayUse::Both) => RunwayUse::Both,
                        (RunwayUse::Arriving, RunwayUse::Departing) |
                        (RunwayUse::Departing, RunwayUse::Arriving) => RunwayUse::Both,
                        (e, _) => e,
                    }
                }).or_insert(config);
            }
        }

        Ok(())
    }

    pub fn select_runways_in_use(&mut self, config: &ESConfig) {
        for airport in self.airports.values_mut() {
            if !airport.runways_in_use.is_empty() {
                continue; // Already set by ATIS
            }
            if let Ok(runways_in_use) = airport.set_runway_based_on_metar_wind(&config) {
                airport.runways_in_use = runways_in_use;
            }
        }
    }

    pub fn apply_default_runways(&mut self, config: &ESConfig) {
        let defaults = config.get_default_runways();
        for airport in self.airports.values_mut() {
            if airport.runways_in_use.is_empty() {
                if let Some(runway) = defaults.get(airport.icao.as_str()) {
                    airport.runways_in_use.insert(format!("{runway:02}"), RunwayUse::Both);
                }
            }
        }
    }

    pub fn airports_without_runway_config(&self) -> Vec<&Airport> {
        self.airports.values().filter(|a| a.runways_in_use.is_empty()).collect()
    }

    pub fn identifiers(&self) -> IndexSet<String> {
        self.airports.keys().cloned().collect()
    }

    pub fn sort(&mut self) {
        self.airports.sort_unstable_keys();
    }
}

impl Index<&str> for Airports {
    type Output = Airport;

    fn index(&self, index: &str) -> &Self::Output {
        &self.airports[index]
    }
}

impl IndexMut<&str> for Airports {
    fn index_mut(&mut self, index: &str) -> &mut Self::Output {
        &mut self.airports[index]
    }
}

fn read_with_encoings<R: Read>(reader: &mut R) -> ApplicationResult<String> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;

    let utf8_decoded = UTF_8.decode(&buffer, DecoderTrap::Strict);

    match utf8_decoded {
        Ok(text) => Ok(text),
        Err(e) => ISO_8859_1
            .decode(&buffer, DecoderTrap::Strict)
            .map_err(|_| crate::error::ApplicationError::EncodingError(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn make_test_airport() {
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.fill_known_airports(&mut reader, &config).unwrap();
        assert_eq!(ap.airports.len(), 51);
    }

    #[test]
    fn test_name() {

    }
}
