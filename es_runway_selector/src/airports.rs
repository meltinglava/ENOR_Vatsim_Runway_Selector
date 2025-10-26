use encoding::{
    DecoderTrap, Encoding,
    all::{ISO_8859_1, UTF_8},
};
use indexmap::{IndexMap, IndexSet};
use tracing_unwrap::ResultExt;

use std::{
    io::{self, Read, Write},
    ops::{Index, IndexMut},
};

use crate::{
    airport::{Airport, RunwayInUseSource},
    atis::find_runway_in_use_from_atis,
    config::ESConfig,
    error::ApplicationResult,
    metar::get_metars,
    runway::{Runway, RunwayDirection, RunwayUse},
};

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

    pub fn fill_known_airports<R: Read>(
        &mut self,
        reader: &mut R,
        config: &ESConfig,
    ) -> ApplicationResult<()> {
        let sct_file = read_with_encoings(reader).expect("Failed to read SCT file");
        let ignored_set = config.get_ignore_airports();

        for line in sct_file
            .lines()
            .skip_while(|line| *line != "[RUNWAY]")
            .skip(1)
            .take_while(|line| !line.is_empty())
        {
            let parts: Vec<_> = line.split_whitespace().collect();
            let icao = *parts.last().unwrap();
            if ignored_set.contains(icao) {
                continue;
            }

            let airport = self
                .airports
                .entry(icao.to_string())
                .or_insert_with(|| Airport {
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
        Ok(())
    }

    pub async fn add_metars(&mut self, conf: &ESConfig) {
        let metars = get_metars(conf).await.unwrap_or_log();
        for metar in metars {
            if let Some(airport) = self.airports.get_mut(&metar.icao) {
                airport.metar = Some(metar);
            }
        }
    }

    pub async fn read_atises_and_apply_runways(&mut self) -> ApplicationResult<()> {
        let icaos = self.identifiers();
        let data = reqwest::get("https://data.vatsim.net/v3/vatsim-data.json")
            .await?
            .json::<serde_json::Value>()
            .await?;

        let serde_json::Value::Object(map) = data else {
            return Ok(());
        };
        let Some(serde_json::Value::Array(atises)) = map.get("atis") else {
            return Ok(());
        };

        for atis in atises {
            let serde_json::Value::Object(atis) = atis else {
                continue;
            };
            let Some(serde_json::Value::String(callsign)) = atis.get("callsign") else {
                continue;
            };
            let icao = &callsign[0..4];

            if !icaos.contains(icao) {
                continue;
            }

            let Some(airport) = self.airports.get_mut(icao) else {
                continue;
            };
            let Some(serde_json::Value::Array(atis_lines)) = atis.get("text_atis") else {
                continue;
            };

            let text = atis_lines
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            for (runway, config) in find_runway_in_use_from_atis(&text) {
                airport
                    .runways_in_use
                    .entry(RunwayInUseSource::Atis)
                    .or_default()
                    .entry(runway)
                    .and_modify(|e| {
                        *e = match (*e, config) {
                            (RunwayUse::Both, _) | (_, RunwayUse::Both) => RunwayUse::Both,
                            (RunwayUse::Arriving, RunwayUse::Departing)
                            | (RunwayUse::Departing, RunwayUse::Arriving) => RunwayUse::Both,
                            (e, _) => e,
                        }
                    })
                    .or_insert(config);
            }
        }

        Ok(())
    }

    pub fn runway_in_use_based_on_metar(&mut self, config: &ESConfig) {
        for airport in self.airports.values_mut() {
            if let Ok(runways_in_use) = airport.set_runway_based_on_metar_wind(config)
                && !runways_in_use.is_empty()
            {
                airport
                    .runways_in_use
                    .insert(RunwayInUseSource::Metar, runways_in_use);
            }
        }
    }

    pub fn apply_default_runways(&mut self, config: &ESConfig) {
        let defaults = config.get_default_runways();
        for airport in self.airports.values_mut() {
            if airport.runways_in_use.is_empty()
                && let Some(runway) = defaults.get(airport.icao.as_str())
            {
                airport.runways_in_use.insert(
                    RunwayInUseSource::Default,
                    [(format!("{runway:02}"), RunwayUse::Both)].into(),
                );
            }
        }
    }

    pub fn airports_without_runway_config(&self) -> Vec<&Airport> {
        self.airports
            .values()
            .filter(|a| a.runways_in_use.is_empty())
            .collect()
    }

    pub fn identifiers(&self) -> IndexSet<String> {
        self.airports.keys().cloned().collect()
    }

    pub fn sort(&mut self) {
        self.airports.sort_unstable_keys();
    }

    pub(crate) fn make_runway_report(&self) {
        let mut stdout = std::io::stdout().lock();
        self.make_runway_report_with_writer(&mut stdout)
            .expect("Failed to write runway report");
    }

    fn make_runway_report_with_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut counter = IndexMap::new();
        'airport_loop: for airport in self.airports.values() {
            for selection_source in RunwayInUseSource::default_sort_order() {
                if airport.runways_in_use.contains_key(&selection_source) {
                    *counter.entry(Some(selection_source)).or_insert(0) += 1;
                    continue 'airport_loop;
                }
            }
            *counter.entry(None).or_insert(0) += 1;
        }
        counter.sort_unstable_by(|k1, _v1, k2, _v2| match (k1, k2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(k1), Some(k2)) => k1.cmp(k2),
        });
        writeln!(writer, "Runway selection report:")?;
        for (source, count) in counter {
            match source {
                Some(RunwayInUseSource::Atis) => writeln!(
                    writer,
                    "{:<46} {}",
                    "Airports runway config selected from ATIS:", count
                )?,
                Some(RunwayInUseSource::Metar) => writeln!(
                    writer,
                    "{:<46} {}",
                    "Airports runway config selected from METAR:", count
                )?,
                Some(RunwayInUseSource::Default) => writeln!(
                    writer,
                    "{:<46} {}",
                    "Airports runway config selected from fallback:", count
                )?,
                None => writeln!(
                    writer,
                    "{:<46} {}",
                    "Airports without selected runway config:", count
                )?,
            };
        }
        Ok(())
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
pub(crate) mod tests {
    use metar_decoder::metar::Metar;

    use super::*;

    #[test]
    fn test_airport_gen() {
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.fill_known_airports(&mut reader, &config).unwrap();
        assert_eq!(ap.airports.len(), 50);
    }

    pub(crate) fn make_test_airport(metar_str: &str) -> Airport {
        let icao = metar_str[0..4].to_string();
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.fill_known_airports(&mut reader, &config).unwrap();
        let airport = ap.airports.swap_remove(&icao).unwrap();
        let metar: Metar = metar_str.parse().unwrap();
        Airport {
            icao: airport.icao,
            metar: Some(metar),
            runways: airport.runways,
            runways_in_use: IndexMap::new(),
        }
    }
}
