//! gRPC protocol definitions for runway-selector area plugins.
//!
//! Plugins implement the [`v1::runway_selector_server::RunwaySelector`] service
//! and the standard `grpc.health.v1.Health` service for readiness checks.
//!
//! Generated code lives under [`v1`].

#[allow(clippy::all, clippy::pedantic, missing_docs, unreachable_pub)]
pub mod v1 {
    tonic::include_proto!("runway_selector.v1");
}

pub use v1::*;

#[cfg(test)]
mod tests {
    use super::v1::*;
    use prost::Message;

    /// Builds a representative request that exercises tagged unions, the
    /// present-but-unknown wrapper, and nested optionality.
    fn sample_request() -> SelectRunwaysRequest {
        let metar = Metar {
            raw: "ENGM 251420Z VRB03KT 9999 -SHSN OVC009 M09/M12 Q1024 NOSIG".into(),
            icao: "ENGM".into(),
            observation_time: Some(prost_types::Timestamp {
                seconds: 1_777_400_400,
                nanos: 0,
            }),
            corrected: false,
            auto: false,
            nosig: true,
            wind: Some(Wind {
                direction: Some(wind::Direction::Variable(())),
                speed: Some(OptionalVelocity {
                    value: Some(3),
                    unit: VelocityUnit::Knots as i32,
                }),
                gust: None,
                variation: None,
            }),
            obscuration: Some(Obscuration {
                variant: Some(obscuration::Variant::Described(DescribedObscuration {
                    visibility: Some(Visibility {
                        value: Some(visibility::Value::Meters(OptionalMeters {
                            value: Some(9999),
                        })),
                        direction: None,
                        no_directional_variation: false,
                    }),
                    directional_visibility: vec![],
                    rvr: vec![],
                    present_weather: vec![PresentWeather {
                        intensity: Some(WeatherIntensity::Light as i32),
                        descriptor: Some(WeatherDescriptor::Showers as i32),
                        phenomena: vec![WeatherPhenomenonValue {
                            code: Some(WeatherPhenomenon::Sn as i32),
                        }],
                    }],
                    clouds: vec![Cloud {
                        variant: Some(cloud::Variant::Data(CloudData {
                            coverage: Some(CloudCoverage::Overcast as i32),
                            height_hundreds_ft: Some(9),
                            cloud_type: None,
                        })),
                    }],
                    vertical_visibility: None,
                })),
            }),
            temperature: Some(Temperature {
                celsius: Some(-9),
                dew_point_celsius: Some(-12),
            }),
            pressure: Some(Pressure {
                qnh: Some(PressureReading {
                    unit: PressureUnit::Hpa as i32,
                    value: Some(1024),
                }),
                altimeter: None,
            }),
            recent_weather: vec![],
            remarks: None,
        };

        SelectRunwaysRequest {
            now_utc: Some(prost_types::Timestamp {
                seconds: 1_777_400_400,
                nanos: 0,
            }),
            area_timezone: "Europe/Oslo".into(),
            airports: vec![AirportRequest {
                icao: "ENGM".into(),
                runways: vec![
                    RunwayInfo {
                        identifier: "01L".into(),
                        heading_degrees_true: 14,
                        wind_components: Some(WindComponents {
                            headwind_kt: 0,
                            crosswind_kt: 3,
                            crosswind_direction: CrosswindDirection::Variable as i32,
                        }),
                    },
                    RunwayInfo {
                        identifier: "01R".into(),
                        heading_degrees_true: 14,
                        wind_components: Some(WindComponents {
                            headwind_kt: 0,
                            crosswind_kt: 3,
                            crosswind_direction: CrosswindDirection::Variable as i32,
                        }),
                    },
                ],
                metar: Some(metar),
                atis_runways: vec![],
            }],
        }
    }

    #[test]
    fn request_round_trips_through_protobuf_encoding() {
        let original = sample_request();
        let encoded = original.encode_to_vec();
        let decoded = SelectRunwaysRequest::decode(&encoded[..]).expect("decode");
        assert_eq!(original, decoded);
    }

    #[test]
    fn optional_velocity_can_express_unknown_value() {
        let unknown = OptionalVelocity {
            value: None, // "//"
            unit: VelocityUnit::Knots as i32,
        };
        let known_zero = OptionalVelocity {
            value: Some(0), // calm wind, value known to be 0
            unit: VelocityUnit::Knots as i32,
        };
        assert_ne!(unknown, known_zero);
    }

    #[test]
    fn enum_round_trip_via_i32() {
        let n = CrosswindDirection::Right as i32;
        let parsed = CrosswindDirection::try_from(n).expect("known variant");
        assert_eq!(parsed, CrosswindDirection::Right);
    }

    /// The generated service traits should be importable and namable.
    #[allow(dead_code)]
    fn _service_traits_exist() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<runway_selector_client::RunwaySelectorClient<tonic::transport::Channel>>(
        );
    }
}
