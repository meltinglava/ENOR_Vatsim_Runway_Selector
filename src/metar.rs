use rust_flightweather::{
    metar::Metar,
    types::{Data, Wind, WindDirection, WindSpeed},
};

use crate::util::diff_angle;

pub async fn get_metars() -> Result<Vec<Metar>, Box<dyn std::error::Error>> {
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
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

#[allow(unused)]
pub fn calculate_max_crosswind(runway: &crate::runway::RunwayDirection, wind: Wind) -> Option<WindSpeed> {
    let track = runway.degrees;
    let strength = wind.gusting.or(wind.speed.as_option().cloned())?;

    let factor = if let Some((Data::Known(start), Data::Known(end))) = wind.varying {
        let cross = [(track + 90) % 360, (track + 270) % 360];
        let (start, end) = (start % 360, end % 360);

        let includes = |a| if start <= end { a >= start && a <= end } else { a >= start || a <= end };

        if cross.iter().any(|&a| includes(a)) {
            1.0
        } else {
            let s: f64 = diff_angle(track, start).into();
            let e: f64 = diff_angle(track, end).into();
            s.to_radians().sin().abs().max(e.to_radians().sin().abs())
        }
    } else if let Data::Known(WindDirection::Heading(dir)) = wind.dir {
        (f64::from(diff_angle(track, dir)).to_radians()).sin().abs()
    } else {
        1.0
    };

    Some(scale_speed(strength, factor))
}

pub fn calculate_max_headwind(runway: &crate::runway::RunwayDirection, wind: Wind) -> Option<WindSpeed> {
    let track = runway.degrees;
    let strength = wind.gusting.or(wind.speed.as_option().cloned())?;

    let factor = if let Some((Data::Known(start), Data::Known(end))) = wind.varying {
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
    } else if let Data::Known(WindDirection::Heading(dir)) = wind.dir {
        f64::from(diff_angle(track, dir)).to_radians().cos().max(0.0)
    } else {
        1.0
    };

    Some(scale_speed(strength, factor))
}

fn scale_speed(speed: WindSpeed, factor: f64) -> WindSpeed {
    match speed {
        WindSpeed::Calm => WindSpeed::Calm,
        WindSpeed::Knot(k) => WindSpeed::Knot((k as f64 * factor).round() as u16),
        WindSpeed::MetresPerSecond(m) => WindSpeed::MetresPerSecond((m as f64 * factor).round() as u16),
        WindSpeed::KilometresPerHour(kph) => WindSpeed::KilometresPerHour((kph as f64 * factor).round() as u16),
    }
}

#[cfg(test)]
mod tests {
    use crate::{airport::Airport, airports::Airports, runway::{RunwayDirection, RunwayUse}};

    use super::*;
    use indexmap::IndexMap;
    use rust_flightweather::types::{Data, WindDirection, WindSpeed};

    fn make_test_airport(icao: &str, metar: &str) -> Airport {
        let mut ap = Airports::new();
        let mut reader = std::io::Cursor::new(include_str!("../runway.txt"));
        ap.fill_known_airports(&mut reader);
        let mut ap = ap.airports.swap_remove(icao).unwrap();
        let metar = Metar::parse(metar).unwrap();
        ap.metar = Some(metar);
        ap
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
        airports.select_runways_in_use();
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
