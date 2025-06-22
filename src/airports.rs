use indexmap::{IndexMap, IndexSet};
use std::ops::{Index, IndexMut};

use crate::airport::Airport;
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

    pub fn add_airport(&mut self, airport: Airport) {
        self.airports.insert(airport.icao.clone(), airport);
    }

    pub fn fill_known_airports(&mut self) {
        let known = include_str!("../runway.txt");
        let ignored = include_str!("../ignore_airports.txt");
        let ignored_set: IndexSet<_> = ignored.lines().collect();

        for line in known.lines().skip(1) {
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
                    RunwayDirection { degrees: parts[2].parse().unwrap(), identifier: parts[0].into() },
                    RunwayDirection { degrees: parts[3].parse().unwrap(), identifier: parts[1].into() },
                ],
            };
            airport.runways.push(runway);
        }
    }

    pub async fn add_metars(&mut self) {
        let metars = get_metars().await.unwrap();
        for metar in metars {
            if let Some(airport) = self.airports.get_mut(&metar.station) {
                airport.metar = Some(metar);
            }
        }
    }

    pub async fn read_atises(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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

    pub fn select_runways_in_use(&mut self) {
        for airport in self.airports.values_mut() {
            if let Some(runways_in_use) = airport.set_runway_based_on_metar_wind() {
                airport.runways_in_use = runways_in_use;
            }
        }
    }

    pub fn apply_default_runways(&mut self) {
        let defaults = include_str!("../default_runways.txt");
        let default_map: IndexMap<_, _> = defaults.lines().filter_map(|line| line.split_once(':')).collect();
        for airport in self.airports.values_mut() {
            if airport.runways_in_use.is_empty() {
                if let Some(runway) = default_map.get(airport.icao.as_str()) {
                    airport.runways_in_use.insert(runway.to_string(), RunwayUse::Both);
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
