use std::{io::Write, ops::{Index, IndexMut}};

use indexmap::{IndexMap, IndexSet};
use itertools::{Itertools, MinMaxResult::{MinMax, NoElements, OneElement}};
use once_cell::sync::Lazy;
use regex::Regex;
use rust_flightweather::{metar::Metar, types::{Data, Wind, WindDirection, WindSpeed}};
use serde_json::Value::{self, Object};
use tracing::warn;

async fn get_metars() -> Result<Vec<Metar>, Box<dyn std::error::Error>> {
    let ignore = ["ENSF"];
    let values = reqwest::get("https://metar.vatsim.net/EN")
        .await?
        .text()
        .await?
        .lines()
        .filter(|line| {
            let icao = line.split_whitespace().next().unwrap();
            !ignore.contains(&icao)
        })
        .map(Metar::parse)
        .collect::<Result<Vec<Metar>, _>>()?;
    Ok(values)
}

#[derive(Debug)]
struct RunwayDirection {
    degrees: u16,
    identifier: String,
}

#[derive(Debug)]
struct Runway {
    runways: [RunwayDirection; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunwayUse {
    Departing,
    Arriving,
    Both,
}

#[derive(Debug)]
struct Airport {
    icao: String,
    metar: Option<Metar>,
    runways: Vec<Runway>,
    runways_in_use: IndexMap<String, RunwayUse>,
}

fn diff_angle(a: u16, b: u16) -> u16 {
    let diff = (a as i16 - b as i16).abs();
    if diff > 180 {
        360 - diff as u16
    } else {
        diff as u16
    }
}

fn diff_rotation(a: u16, b: u16) -> u16 {
    let diff = a as i16 - b as i16;
    if diff < 0 {
        (diff + 360) as u16
    } else {
        diff as u16
    }
}

#[allow(dead_code)]
fn calculate_max_crosswind(runway: &RunwayDirection, wind: Wind) -> Option<WindSpeed> {
    let runway_track = runway.degrees;
    let wind_strength = wind.gusting.or(wind.speed.as_option().cloned())?;

    let factor = if let Some((Data::Known(start), Data::Known(end))) = wind.varying {
        let crosswind_angles = [
            (runway_track + 90) % 360,
            (runway_track + 270) % 360,
        ];

        // Normalize angles into [0, 360)
        let start = start % 360;
        let end = end % 360;

        let includes = |angle| {
            if start <= end {
                angle >= start && angle <= end
            } else {
                // wraparound arc
                angle >= start || angle <= end
            }
        };

        if crosswind_angles.iter().any(|&a| includes(a)) {
            1.0 // maximum crosswind possible
        } else {
            let diff1 = diff_angle(runway_track, start);
            let diff2 = diff_angle(runway_track, end);
            let sin1 = (f64::from(diff1).to_radians()).sin().abs();
            let sin2 = (f64::from(diff2).to_radians()).sin().abs();
            sin1.max(sin2)
        }
    } else if let Data::Known(WindDirection::Heading(dir)) = wind.dir {
        let diff = diff_angle(runway_track, dir);
        (f64::from(diff).to_radians()).sin().abs()
    } else {
        1.0
    };

    Some(match wind_strength {
        WindSpeed::Calm => WindSpeed::Calm,
        WindSpeed::Knot(knots) => WindSpeed::Knot((knots as f64 * factor).round() as u16),
        WindSpeed::MetresPerSecond(mps) => WindSpeed::MetresPerSecond((mps as f64 * factor).round() as u16),
        WindSpeed::KilometresPerHour(kph) => WindSpeed::KilometresPerHour((kph as f64 * factor).round() as u16),
    })
}

fn calculate_max_headwind(runway: &RunwayDirection, wind: Wind) -> Option<WindSpeed> {
    let runway_track = runway.degrees;
    let wind_strength = wind.gusting.or(wind.speed.as_option().cloned())?;

    let factor = if let Some((Data::Known(start), Data::Known(end))) = wind.varying {
        let headwind_directions = [
            runway_track % 360,
            (runway_track + 180) % 360, // the opposite runway also counts for headwind component
        ];

        let start = start % 360;
        let end = end % 360;

        let includes = |angle| {
            if start <= end {
                angle >= start && angle <= end
            } else {
                // wraparound arc
                angle >= start || angle <= end
            }
        };

        if headwind_directions.iter().any(|&a| includes(a)) {
            1.0 // full headwind possible
        } else {
            let diff1 = diff_angle(runway_track, start);
            let diff2 = diff_angle(runway_track, end);
            let cos1 = (f64::from(diff1).to_radians()).cos();
            let cos2 = (f64::from(diff2).to_radians()).cos();
            cos1.max(cos2).max(0.0)
        }
    } else if let Data::Known(WindDirection::Heading(dir)) = wind.dir {
        let diff = diff_angle(runway_track, dir);
        (f64::from(diff).to_radians()).cos().max(0.0)
    } else {
        1.0 // No direction = assume worst-case
    };

    Some(match wind_strength {
        WindSpeed::Calm => WindSpeed::Calm,
        WindSpeed::Knot(knots) => WindSpeed::Knot((knots as f64 * factor).round() as u16),
        WindSpeed::MetresPerSecond(mps) => WindSpeed::MetresPerSecond((mps as f64 * factor).round() as u16),
        WindSpeed::KilometresPerHour(kph) => WindSpeed::KilometresPerHour((kph as f64 * factor).round() as u16),
    })
}

impl Airport {
    fn set_runway_based_on_metar_wind(&self) -> Option<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" || self.icao == "ENZV" {
            return None; // TODO: Special logic for these airports
        }

        let metar = self.metar.as_ref()?;

        let headwinds = self.runways.iter().flat_map(|runway| {
            runway.runways.iter().map(|dir| {
                let headwind = calculate_max_headwind(dir, metar.wind.clone());
                (dir.identifier.clone(), headwind)
            })
        }).collect::<IndexMap<_, _>>();

        let valid_headwind_values = headwinds.values().filter_map(|v| v.as_ref().map(|w| w.as_knots())).collect::<Vec<_>>();

        if valid_headwind_values.is_empty() {
            return None;
        }

        let (min, max) = match valid_headwind_values.iter().cloned().minmax() {
            MinMax(min, max) => (min, max),
            NoElements => return None,
            OneElement(value) => (value, value),
        };
        if (max - min) > 2.0 {
            let selected = headwinds.iter().find(|(_, v)| {
                v.as_ref().map(|w| (w.as_knots() - max).abs() < f64::EPSILON).unwrap_or(false)
            });
            if let Some((ident, _)) = selected {
                let mut map = IndexMap::new();
                map.insert(ident.clone(), RunwayUse::Both);
                return Some(map);
            }
        }

        None
    }
}

#[derive(Debug)]
struct Airports {
    airports: IndexMap<String, Airport>,
}

impl Airports {
    fn new() -> Self {
        Self {
            airports: IndexMap::new(),
        }
    }

    fn add_airport(&mut self, airport: Airport) {
        self.airports.insert(airport.icao.clone(), airport);
    }

    fn get_mut(&mut self, icao: &str) -> Option<&mut Airport> {
        self.airports.get_mut(icao)
    }


    fn entry(&mut self, icao: String) -> indexmap::map::Entry<String, Airport> {
        self.airports.entry(icao)
    }

    fn identifiers(&self) -> IndexSet<String> {
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

async fn add_metars(airports: &mut Airports) {
    let metars = get_metars().await.unwrap();
    for metar in metars {
        if let Some(airport) = airports.get_mut(&metar.station) {
            airport.metar = Some(metar);
        }
    }
}

fn fill_known_airports(airports: &mut Airports) {
    // example of runways
    // 01L 19R 012 192 N060.11.06.000 E011.04.25.478 N060.12.57.841 E011.05.29.990 ENGM
    // 01R 19L 012 192 N060.10.32.721 E011.06.28.018 N060.12.04.348 E011.07.20.949 ENGM
    let know_runways = include_str!("../runway.txt");
    let ignore_str = include_str!("../ignore_airports.txt");
    let ignore_airports: IndexSet<_> = ignore_str.lines().collect();
    for line in know_runways.lines().skip(1) {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let icao = parts.last().unwrap();
        if ignore_airports.contains(icao) {
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
                    degrees: parts[2].parse().unwrap(),
                    identifier: parts[0].to_string(),
                },
                RunwayDirection {
                    degrees: parts[3].parse().unwrap(),
                    identifier: parts[1].to_string(),
                },
            ],
        };
        airport.runways.push(runway);
    }
}



fn find_runway_in_use_from_atis(atis: &str) -> IndexMap<String, RunwayUse> {
    static SINGLE_RUNWAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"RUNWAY IN USE ([0-9]{2}[LRC]*)").unwrap());
    static ARRIVAL_RUNWAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"APPROACH RWY ([0-9]{2}[LRC]*)").unwrap());
    static DEPARTURE_RUNWAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"DEPARTURE RUNWAY ([0-9]{2}[LRC]*)").unwrap());
    static MULTI_RUNWAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"RUNWAYS ([0-9]{2}[LRC]*) AND ([0-9]{2}[LRC]*) IN USE").unwrap());


    let mut runways = IndexMap::new();
    if let Some(captures) = SINGLE_RUNWAY.captures(atis) {
        runways.insert(captures[1].to_string(), RunwayUse::Both);
    } else if let Some(captures) = ARRIVAL_RUNWAY.captures(atis) {
        runways.insert(captures[1].to_string(), RunwayUse::Arriving);
    } else if let Some(captures) = DEPARTURE_RUNWAY.captures(atis) {
        runways.insert(captures[1].to_string(), RunwayUse::Departing);
    } else if let Some(captures) = MULTI_RUNWAY.captures(atis) {
        runways.insert(captures[1].to_string(), RunwayUse::Both);
        runways.insert(captures[2].to_string(), RunwayUse::Both);
    }
    runways
}

async fn read_atises(airports: &mut Airports) -> Result<(), Box<dyn std::error::Error>> {
    let icaos = airports.identifiers();
    let values = reqwest::get("https://data.vatsim.net/v3/vatsim-data.json")
        .await?
        .json::<serde_json::Value>()
        .await?;

    let Object(map) = values else { return Ok(()); };
    let Some(Value::Array(atises)) = map.get("atis") else { return Ok(()); };

    for atis in atises {
        let Object(atis) = atis else { continue; };
        let Some(Value::String(atis_callsign)) = atis.get("callsign") else { continue; };
        let icao = &atis_callsign[0..4];
        if !icaos.contains(icao) {
            continue;
        }

        let Some(airport) = airports.get_mut(icao) else { continue; };
        let Some(Value::Array(atis_text_lines)) = atis.get("text_atis") else { continue; };

        let atis_text = atis_text_lines.iter().filter_map(|e| e.as_str()).collect::<Vec<_>>().join(" ");
        for (runway, config) in find_runway_in_use_from_atis(&atis_text) {
            airport.runways_in_use.entry(runway).and_modify(|e| {
                *e = match (*e, config) {
                    (RunwayUse::Both, _) => RunwayUse::Both,
                    (_, RunwayUse::Both) => RunwayUse::Both,
                    (RunwayUse::Arriving, RunwayUse::Departing) => RunwayUse::Both,
                    (RunwayUse::Departing, RunwayUse::Arriving) => RunwayUse::Both,
                    (e, _) => e,
                }
            }).or_insert_with(|| config);
        }
    }
    Ok(())
}

fn select_runways_in_use(airports: &mut Airports) {
    for airport in airports.airports.values_mut() {
        if let Some(runways_in_use) = airport.set_runway_based_on_metar_wind() {
            airport.runways_in_use = runways_in_use;
        }
    }
}

fn apply_default_runways(airports: &mut Airports) {
    let dr = include_str!("../default_runways.txt");
    let defaults: IndexMap<_, _> = dr.lines().map(|line| {
        line.split_once(':').unwrap()
    }).collect();
    airports.airports.iter_mut().for_each(|(_, airport)| {
        if airport.runways_in_use.is_empty() {
            let runway = match defaults.get(airport.icao.as_str()) {
                Some(s) => s,
                None => return,
            };
            airport.runways_in_use.insert(runway.to_string(), RunwayUse::Both);
        }
    });
}

async fn write_runways_to_euroscope_rwy_file<F: Write>(airports: &Airports, file: &mut F) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = std::io::BufWriter::new(file);
    for airport in airports.airports.values() {
        if airport.runways.is_empty() {
            warn!("No runways for airport {}", airport.icao);
            continue;
        }
        for (runway, usage) in &airport.runways_in_use {
            let flags: &[u8] = match usage {
                RunwayUse::Departing => &[1],
                RunwayUse::Arriving => &[0],
                RunwayUse::Both => &[1, 0],
            };
            for flag in flags {
                writeln!(writer, "ACTIVE_RUNWAY:{}:{}:{}", &airport.icao, runway, flag)?;
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let mut airports = Airports::new();
    fill_known_airports(&mut airports);
    add_metars(&mut airports).await;
    read_atises(&mut airports).await.unwrap();
    select_runways_in_use(&mut airports);
    apply_default_runways(&mut airports);
    write_runways_to_euroscope_rwy_file(&airports, &mut std::fs::File::create("ouput.rwy").unwrap()).await.unwrap();
    let no_runways_in_use = airports.airports.values().filter(|a| a.runways_in_use.is_empty()).collect_vec();
    dbg!(no_runways_in_use);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_flightweather::types::{Data, WindDirection, WindSpeed};

    fn make_test_airport(icao: &str, metar: &str) -> Airport {
        let mut ap = Airports::new();
        fill_known_airports(&mut ap);
        let mut ap = ap.airports.swap_remove(icao).unwrap();
        let metar = Metar::parse(metar).unwrap();
        ap.metar = Some(metar);
        ap
    }

    #[test]
    fn test_diff_angle() {
        assert_eq!(diff_angle(10, 350), 20);
        assert_eq!(diff_angle(0, 180), 180);
        assert_eq!(diff_angle(270, 90), 180);
        assert_eq!(diff_angle(90, 270), 180);
        assert_eq!(diff_angle(0, 0), 0);
    }

    #[test]
    fn test_diff_rotation() {
        assert_eq!(diff_rotation(10, 350), 20);
        assert_eq!(diff_rotation(0, 180), 180);
        assert_eq!(diff_rotation(270, 90), 180);
        assert_eq!(diff_rotation(90, 270), 180);
        assert_eq!(diff_rotation(0, 0), 0);
    }

    #[test]
    fn test_calculate_max_crosswind() {
        let wind = Wind {
            dir: Data::Known(WindDirection::Heading(270)),
            speed: Data::Known(WindSpeed::Knot(10)),
            gusting: None,
            varying: None,
        };
        let runway = RunwayDirection { degrees: 360, identifier: "36".into() };
        let crosswind = calculate_max_crosswind(&runway, wind).unwrap();
        match crosswind {
            WindSpeed::Knot(val) => assert!((val as f64 - 10.0).abs() < 0.1),
            _ => panic!("Expected knots"),
        }
    }

    #[test]
    fn test_calculate_max_headwind() {
        let wind = Wind {
            dir: Data::Known(WindDirection::Heading(360)),
            speed: Data::Known(WindSpeed::Knot(10)),
            gusting: None,
            varying: None,
        };
        let runway = RunwayDirection { degrees: 360, identifier: "36".into() };
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        match headwind {
            WindSpeed::Knot(val) => assert_eq!(val, 10),
            _ => panic!("Expected knots"),
        }
    }

    #[test]
    fn test_find_runway_in_use_from_atis() {
        let text = "RUNWAY IN USE 19L";
        let map = find_runway_in_use_from_atis(text);
        assert_eq!(map.len(), 1);
        assert_eq!(map["19L"], RunwayUse::Both);
    }

    #[test]
    fn test_calculate_max_headwind_with_varying_direction_should_not_return_full_strength() {
        let wind = Wind {
            dir: Data::Known(WindDirection::Heading(300)),
            speed: Data::Known(WindSpeed::Knot(9)),
            gusting: None,
            varying: Some((Data::Known(250), Data::Known(330))),
        };

        let runway = RunwayDirection {
            degrees: 167, // Opposite direction to the wind (i.e., mostly tailwind)
            identifier: "17".into(),
        };

        let result = calculate_max_headwind(&runway, wind).unwrap();
        match result {
            WindSpeed::Knot(knots) => {
                // If the cross/headwind logic is working correctly, this should be < 9
                assert!(knots < 9, "Expected headwind < 9 knots, got {} knots", knots);
            }
            _ => panic!("Expected WindSpeed::Knot"),
        }
    }

    #[test]
    fn test_metar_enmh() {
        let metar = "ENMH 220550Z AUTO 30009KT 250V330 9999 BKN028/// OVC049/// 07/02 Q1016";
        let airport = make_test_airport("ENMH", metar);
        let mut airports = Airports::new();
        airports.add_airport(airport);
        select_runways_in_use(&mut airports);
        assert_eq!(airports["ENMH"].runways_in_use, IndexMap::from([
            ("35".to_string(), RunwayUse::Both),
        ]));
    }

    fn wind_kts_dir_knots(dir: u16, knots: u16) -> Wind {
        Wind {
            dir: Data::Known(WindDirection::Heading(dir)),
            speed: Data::Known(WindSpeed::Knot(knots)),
            gusting: None,
            varying: None,
        }
    }

    fn wind_kts_varying_knots(start: u16, end: u16, knots: u16) -> Wind {
        Wind {
            dir: Data::Known(WindDirection::Heading((start + end) / 2)),
            speed: Data::Known(WindSpeed::Knot(knots)),
            gusting: None,
            varying: Some((Data::Known(start), Data::Known(end))),
        }
    }

    #[test]
    fn test_single_direction_headwind() {
        let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
        let wind = wind_kts_dir_knots(180, 10);
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        assert_eq!(headwind, WindSpeed::Knot(10));
    }

    #[test]
    fn test_varying_crosses_runway_heading() {
        let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
        let wind = wind_kts_varying_knots(150, 210, 10); // crosses runway heading
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        assert_eq!(headwind, WindSpeed::Knot(10), "Should be full strength due to crossing heading");
    }

    #[test]
    fn test_varying_does_not_cross_runway_heading() {
        let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
        let wind = wind_kts_varying_knots(120, 150, 10); // arc is before the runway
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        match headwind {
            WindSpeed::Knot(knots) => {
                assert!(knots < 10, "Expected partial headwind, got {}", knots);
                assert!(knots > 0, "Expected nonzero headwind");
            }
            _ => panic!("Expected WindSpeed::Knot"),
        }
    }

    #[test]
    fn test_varying_wraparound_crosses_runway_heading() {
        let runway = RunwayDirection { degrees: 10, identifier: "01".into() };
        let wind = wind_kts_varying_knots(350, 30, 12); // arc crosses 10°
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        assert_eq!(headwind, WindSpeed::Knot(12), "Should be full strength due to wraparound crossing");
    }

    #[test]
    fn test_varying_wraparound_does_not_cross_runway_heading() {
        let runway = RunwayDirection { degrees: 270, identifier: "27".into() };
        let wind = wind_kts_varying_knots(300, 60, 12); // arc does not include 270
        let headwind = calculate_max_headwind(&runway, wind).unwrap();
        match headwind {
            WindSpeed::Knot(knots) => {
                assert!(knots < 12, "Expected partial headwind, got {}", knots);
            }
            _ => panic!("Expected WindSpeed::Knot"),
        }
    }
}
