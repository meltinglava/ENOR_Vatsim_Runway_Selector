use std::str::FromStr;

use futures::future::try_join_all;
use metar_decoder::metar::Metar;
use tracing_unwrap::ResultExt;

use crate::{config::ESConfig, error::ApplicationResult};

pub async fn get_metars(conf: &ESConfig) -> ApplicationResult<Vec<Metar>> {
    let ignore = conf.get_ignore_airports();
    let urls = [
        "https://metar.vatsim.net/EN",
        "https://metar.vatsim.net/ESKS",
    ];

    let pages = try_join_all(urls.iter().map(async |url| get_metars_from_url(url).await)).await?;

    let values = pages
        .iter()
        .flat_map(|s| s.lines())
        .filter(|line| !ignore.contains(&line[0..4]))
        .map(Metar::from_str)
        .filter_map(Result::ok_or_log)
        .collect();
    Ok(values)
}

#[tracing::instrument]
async fn get_metars_from_url(url: &str) -> ApplicationResult<String> {
    let retries = 3;
    let mut first_error = None;
    for i in 0..retries {
        let resp = reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_log()
            .get(url)
            .send()
            .await;

        match resp {
            Ok(resp) => {
                let text = resp.text().await;
                match text {
                    Ok(text) => return Ok(text),
                    Err(e) => {
                        if i == retries {
                            tracing::error!("Failed to get text from {}: {}", url, e);
                        } else {
                            tracing::warn!("Failed to get text from {}: {}", url, e);
                        }
                        first_error.get_or_insert(e);
                    }
                }
            }
            Err(e) => {
                if i == retries {
                    tracing::error!("Failed to get {}: {}", url, e);
                } else {
                    tracing::warn!("Failed to get {}: {}", url, e);
                }
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap().into())
}

#[cfg(test)]
mod tests {
    use metar_decoder::{
        optional_data::OptionalData,
        units::{
            track::Track,
            velocity::{VelocityUnit, WindVelocity},
        },
        wind::{Wind, WindDirection},
    };

    // #[test]
    // fn test_calculate_max_crosswind() {
    //     let wind = Wind {
    //         dir: WindDirection::Heading(Track(OptionalData::Data(360))),
    //         speed: WindVelocity { velocity: OptionalData::Data(10), gust: None, unit: VelocityUnit::Knots },
    //         varying: None,
    //     };
    //     let runway = RunwayDirection { degrees: 360, identifier: "36".into() };
    //     let crosswind = calculate_max_crosswind(&runway, &wind).unwrap();
    //     match crosswind {
    //         WindSpeed::Knot(val) => assert!((val as f64 - 10.0).abs() < 0.1),
    //         _ => panic!("Expected knots"),
    //     }
    // }

    // #[test]
    // fn test_calculate_max_headwind() {
    //     let wind = Wind {
    //         dir: WindDirection::Heading(Track(OptionalData::Data(360))),
    //         speed: WindVelocity { velocity: OptionalData::Data(10), gust: None, unit: VelocityUnit::Knots },
    //         varying: None,
    //     };
    //     let runway = RunwayDirection { degrees: 360, identifier: "36".into() };
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     match headwind {
    //         WindVelocity::Knot(val) => assert_eq!(val, 10),
    //         _ => panic!("Expected knots"),
    //     }
    // }

    // #[test]
    // fn test_calculate_max_headwind_with_varying_direction_should_not_return_full_strength() {
    //     let wind = Wind {
    //         dir: WindDirection::Heading(Track(OptionalData::Data(300))),
    //         speed: WindVelocity { velocity: OptionalData::Data(90), gust: None, unit: VelocityUnit::Knots },
    //         varying: Some((Track(OptionalData::Data(250)), Track(OptionalData::Data(330)))),
    //     };

    //     let runway = RunwayDirection {
    //         degrees: 167, // Opposite direction to the wind (i.e., mostly tailwind)
    //         identifier: "17".into(),
    //     };

    //     let result = calculate_max_headwind(&runway, wind).unwrap();
    //     match result {
    //         WindSpeed::Knot(knots) => {
    //             // If the cross/headwind logic is working correctly, this should be < 9
    //             assert!(knots < 9, "Expected headwind < 9 knots, got {} knots", knots);
    //         }
    //         _ => panic!("Expected WindSpeed::Knot"),
    //     }
    // }

    #[allow(unused)]
    fn wind_kts_dir_knots(dir: u32, knots: u32) -> Wind {
        Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data(dir))),
            speed: WindVelocity {
                velocity: OptionalData::Data(knots),
                gust: None,
                unit: VelocityUnit::Knots,
            },
            varying: None,
        }
    }

    #[allow(unused)]
    fn wind_kts_varying_knots(start: u32, end: u32, knots: u32) -> Wind {
        Wind {
            dir: WindDirection::Heading(Track(OptionalData::Data((start + end) / 2))),
            speed: WindVelocity {
                velocity: OptionalData::Data(knots),
                gust: None,
                unit: VelocityUnit::Knots,
            },
            varying: Some((
                Track(OptionalData::Data(start)),
                Track(OptionalData::Data(end)),
            )),
        }
    }

    // #[test]
    // fn test_single_direction_headwind() {
    //     let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
    //     let wind = wind_kts_dir_knots(180, 10);
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     assert_eq!(headwind, WindSpeed::Knot(10));
    // }

    // #[test]
    // fn test_varying_crosses_runway_heading() {
    //     let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
    //     let wind = wind_kts_varying_knots(150, 210, 10); // crosses runway heading
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     assert_eq!(headwind, WindSpeed::Knot(10), "Should be full strength due to crossing heading");
    // }

    // #[test]
    // fn test_varying_does_not_cross_runway_heading() {
    //     let runway = RunwayDirection { degrees: 180, identifier: "18".into() };
    //     let wind = wind_kts_varying_knots(120, 150, 10); // arc is before the runway
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     match headwind {
    //         WindSpeed::Knot(knots) => {
    //             assert!(knots < 10, "Expected partial headwind, got {}", knots);
    //             assert!(knots > 0, "Expected nonzero headwind");
    //         }
    //         _ => panic!("Expected WindSpeed::Knot"),
    //     }
    // }

    // #[test]
    // fn test_varying_wraparound_crosses_runway_heading() {
    //     let runway = RunwayDirection { degrees: 10, identifier: "01".into() };
    //     let wind = wind_kts_varying_knots(350, 30, 12); // arc crosses 10°
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     assert_eq!(headwind, WindSpeed::Knot(12), "Should be full strength due to wraparound crossing");
    // }

    // #[test]
    // fn test_varying_wraparound_does_not_cross_runway_heading() {
    //     let runway = RunwayDirection { degrees: 270, identifier: "27".into() };
    //     let wind = wind_kts_varying_knots(300, 60, 12); // arc does not include 270
    //     let headwind = calculate_max_headwind(&runway, wind).unwrap();
    //     match headwind {
    //         WindSpeed::Knot(knots) => {
    //             assert!(knots < 12, "Expected partial headwind, got {}", knots);
    //         }
    //         _ => panic!("Expected WindSpeed::Knot"),
    //     }
    // }
}
