use metar_decoder::{metar::{nom_parse_metar, Metar}, optional_data::OptionalData, units::{track::Track, velocity::WindVelocity}, wind::Wind};
use nom::{Finish, IResult};

use crate::{error::ApplicationResult, util::diff_angle};

pub async fn get_metars() -> ApplicationResult<Vec<Metar>> {
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
        .map(nom_parse_metar)
        .map(IResult::finish)
        .map(|result| {
            result.map_err(|e| e.cloned())
        })
        .map(|result| {
            result.map(|(_, metar)| metar)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

pub fn calculate_max_crosswind(runway: &crate::runway::RunwayDirection, wind: &Wind) -> Option<u32> {
    let track: u32 = runway.degrees as u32;

    let factor = if let Some((Track(OptionalData::Data(start)), Track(OptionalData::Data(end)))) = wind.varying {
        let cross = [(track + 90) % 360, (track + 270) % 360];
        let (start, end): (u32, u32) = (start % 360, end % 360);

        let includes = |a| if start <= end { a >= start && a <= end } else { a >= start || a <= end };

        if cross.iter().any(|&a| includes(a)) {
            1.0
        } else {
            let s: f64 = diff_angle(track, start).into();
            let e: f64 = diff_angle(track, end).into();
            s.to_radians().sin().abs().max(e.to_radians().sin().abs())
        }
    } else {
        match &wind.dir {
            metar_decoder::wind::WindDirection::Heading(wind_dir) => {
                wind_dir
                    .0
                    .to_option()
                    .map(|wind_dir| (diff_angle(track, wind_dir) as f64).to_radians().sin().abs())
                    .unwrap_or(1.0)
            },
            metar_decoder::wind::WindDirection::Variable => 1.0,
        }
    };

    scale_speed(wind.speed, factor)
}

pub fn calculate_max_headwind(runway: &crate::runway::RunwayDirection, wind: Wind) -> Option<u32> {
    let track = runway.degrees as u32;

    let factor = if let Some((Track(OptionalData::Data(start)), Track(OptionalData::Data(end)))) = wind.varying {
        let heads = [track % 360, (track + 180) % 360];
        let (start, end) = (start % 360, end % 360);
        let includes = |a| if start <= end { a >= start && a <= end } else { a >= start || a <= end };

        if heads.iter().any(|&a| includes(a)) {
            1.0
        } else {
            let s = f64::from(diff_angle(track, start));
            let e = f64::from(diff_angle(track, end));
            s.to_radians().cos().max(e.to_radians().cos()).max(0.0)
        }
    } else {
        match &wind.dir {
            metar_decoder::wind::WindDirection::Heading(wind_dir) => {
                wind_dir
                    .0
                    .to_option()
                    .map(|wind_dir| (diff_angle(track, wind_dir) as f64).to_radians().cos().abs())
                    .unwrap_or(1.0)
            },
            metar_decoder::wind::WindDirection::Variable => 1.0,
        }
    };

    scale_speed(wind.speed, factor)
}

fn scale_speed(speed: WindVelocity, factor: f64) -> Option<u32> {
    speed.get_max_wind_speed().map(|s| (f64::from(s) * factor).ceil() as u32)
}

#[cfg(test)]
mod tests {
    use crate::{airport::Airport, airports::Airports, config::ESConfig, runway::{RunwayDirection, RunwayUse}};

    use super::*;
    use indexmap::IndexMap;
    use metar_decoder::{units::velocity::VelocityUnit, wind::WindDirection};

    fn make_test_airport(icao: &str, metar: &str) -> Airport {
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.test"));
        let config = ESConfig::new_for_test();
        ap.fill_known_airports(&mut reader, &config).unwrap();
        let mut ap = ap.airports.swap_remove(icao).unwrap();
        let metar = Metar::parse(metar).unwrap();
        ap.metar = Some(metar);
        ap
    }

    #[test]
    fn test_calculate_max_crosswind() {
        let wind = Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data(360))),
            speed: WindVelocity { velocity: OptionalData::Data(10), gust: None, unit: VelocityUnit::Knots },
            varying: None,
        };
        let runway = RunwayDirection { degrees: 360, identifier: "36".into() };
        let crosswind = calculate_max_crosswind(&runway, &wind).unwrap();
        match crosswind {
            WindSpeed::Knot(val) => assert!((val as f64 - 10.0).abs() < 0.1),
            _ => panic!("Expected knots"),
        }
    }

    #[test]
    fn test_calculate_max_headwind() {
        let wind = Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data(360))),
            speed: WindVelocity { velocity: OptionalData::Data(10), gust: None, unit: VelocityUnit::Knots },
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
    fn test_calculate_max_headwind_with_varying_direction_should_not_return_full_strength() {
        let wind = Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data(300))),
            speed: WindVelocity { velocity: OptionalData::Data(90), gust: None, unit: VelocityUnit::Knots },
            varying: Some((Track(OptionalData::Data(250)), Track(OptionalData::Data(330)))),
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
        airports.select_runways_in_use(&ESConfig::new_for_test());
        assert_eq!(airports["ENMH"].runways_in_use, IndexMap::from([
            ("35".to_string(), RunwayUse::Both),
        ]));
    }

    fn wind_kts_dir_knots(dir: u32, knots: u32) -> Wind {
        Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data(dir))),
            speed: WindVelocity { velocity: OptionalData::Data(knots), gust: None, unit: VelocityUnit::Knots },
            varying: None,
        }
    }

    fn wind_kts_varying_knots(start: u32, end: u32, knots: u32) -> Wind {
        Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data((start + end) / 2))),
            speed: WindVelocity { velocity: OptionalData::Data(knots), gust: None, unit: VelocityUnit::Knots },
            varying: Some((Track(OptionalData::Data(start)), Track(OptionalData::Data(end)))),
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
