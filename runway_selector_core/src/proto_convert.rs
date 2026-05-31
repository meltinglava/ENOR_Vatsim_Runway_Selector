//! Conversions from [`metar_decoder`] domain types into the gRPC protocol
//! types in [`runway_selector_protocol::v1`].
//!
//! The host parses METARs locally with `metar_decoder`, then ships them to
//! area plugins over gRPC. This module is the bridge.
//!
//! Fields the plugin contract intentionally omits (BECMG, TEMPO, NATO mil
//! code, sea-surface indicator) are dropped on the floor here — no plugin
//! currently needs them.

use metar_decoder::{
    metar::Metar as DecodedMetar,
    obscuration::{
        Cloud as DecodedCloud, CloudCoverage as DecodedCloudCoverage,
        CloudData as DecodedCloudData, DescribedObscuration as DecodedDescribedObscuration,
        Direction as DecodedDirection, DistanceModifier as DecodedDistanceModifier,
        Obscuration as DecodedObscuration, PresentWeather as DecodedPresentWeather,
        Qualifier as DecodedQualifier, Rvr as DecodedRvr,
        StatuteMilesVisibility as DecodedStatuteMilesVisibility, Trend as DecodedTrend,
        VerticalVisibility as DecodedVerticalVisibility, Visibility as DecodedVisibility,
        VisibilityUnit as DecodedVisibilityUnit, WeatherIntensity as DecodedWeatherIntensity,
        WeatherPhenomenon as DecodedWeatherPhenomenon,
    },
    optional_data::OptionalData,
    pressure::{
        Pressure as DecodedPressure, PressureSingle as DecodedPressureSingle,
        PressureUnit as DecodedPressureUnit,
    },
    temperature::TemperatureInfo,
    units::{
        timestamp::Timestamp as DecodedTimestamp,
        track::Track,
        velocity::{VelocityUnit as DecodedVelocityUnit, WindVelocity as DecodedWindVelocity},
    },
    wind::{Wind as DecodedWind, WindDirection as DecodedWindDirection},
};

use crate::airport::{Airport, CrosswindDirection, RunwayInUseSource};
use crate::runway::{RunwayDirection, RunwayUse};

use runway_selector_protocol::v1 as proto;

pub fn timestamp_to_proto(ts: &DecodedTimestamp) -> prost_types::Timestamp {
    let zoned = ts.zoned();
    prost_types::Timestamp {
        seconds: zoned.timestamp().as_second(),
        nanos: zoned.timestamp().subsec_nanosecond(),
    }
}

pub fn metar_to_proto(m: &DecodedMetar) -> proto::Metar {
    proto::Metar {
        raw: m.raw.clone(),
        icao: m.icao.clone(),
        observation_time: Some(timestamp_to_proto(&m.timestamp)),
        corrected: m.corrected,
        auto: m.auto,
        nosig: m.nosig,
        wind: Some(wind_to_proto(&m.wind)),
        obscuration: Some(obscuration_to_proto(&m.obscuration)),
        temperature: Some(temperature_to_proto(&m.temperature)),
        pressure: Some(pressure_to_proto(&m.pressure)),
        recent_weather: m
            .recent_weather
            .as_ref()
            .map(|list| list.iter().map(present_weather_to_proto).collect())
            .unwrap_or_default(),
        remarks: m.remarks.clone(),
    }
}

fn wind_to_proto(w: &DecodedWind) -> proto::Wind {
    let direction = match &w.dir {
        DecodedWindDirection::Variable => Some(proto::wind::Direction::Variable(())),
        DecodedWindDirection::Heading(track) => Some(proto::wind::Direction::Heading(
            track_to_optional_degrees(track),
        )),
    };
    proto::Wind {
        direction,
        speed: Some(wind_speed_to_proto(&w.speed)),
        gust: w
            .speed
            .gust
            .map(|g| gust_to_optional_velocity(g, w.speed.unit)),
        variation: w.varying.as_ref().map(|(from, to)| proto::WindVariation {
            from: Some(track_to_optional_degrees(from)),
            to: Some(track_to_optional_degrees(to)),
        }),
    }
}

fn track_to_optional_degrees(track: &Track) -> proto::OptionalDegrees {
    proto::OptionalDegrees {
        degrees: optional_u32(track.0),
    }
}

fn wind_speed_to_proto(v: &DecodedWindVelocity) -> proto::OptionalVelocity {
    proto::OptionalVelocity {
        value: optional_u32(v.velocity),
        unit: velocity_unit_to_proto(v.unit) as i32,
    }
}

fn gust_to_optional_velocity(
    g: OptionalData<u32, 2>,
    unit: DecodedVelocityUnit,
) -> proto::OptionalVelocity {
    proto::OptionalVelocity {
        value: optional_u32(g),
        unit: velocity_unit_to_proto(unit) as i32,
    }
}

fn velocity_unit_to_proto(u: DecodedVelocityUnit) -> proto::VelocityUnit {
    match u {
        DecodedVelocityUnit::Knots => proto::VelocityUnit::Knots,
        DecodedVelocityUnit::MetersPerSecond => proto::VelocityUnit::MetersPerSecond,
    }
}

fn obscuration_to_proto(o: &DecodedObscuration) -> proto::Obscuration {
    let variant = match o {
        DecodedObscuration::Cavok => Some(proto::obscuration::Variant::Cavok(())),
        DecodedObscuration::Described(d) => Some(proto::obscuration::Variant::Described(
            described_obscuration_to_proto(d),
        )),
    };
    proto::Obscuration { variant }
}

fn described_obscuration_to_proto(d: &DecodedDescribedObscuration) -> proto::DescribedObscuration {
    proto::DescribedObscuration {
        visibility: Some(visibility_to_proto(&d.visibility)),
        directional_visibility: d
            .direction_visibility
            .as_ref()
            .map(|v| v.iter().map(visibility_to_proto).collect())
            .unwrap_or_default(),
        rvr: d.rvr.iter().map(rvr_to_proto).collect(),
        present_weather: d
            .present_weather
            .iter()
            .map(present_weather_to_proto)
            .collect(),
        clouds: d.clouds.iter().map(cloud_to_proto).collect(),
        vertical_visibility: d
            .vertical_visibility
            .as_ref()
            .map(vertical_visibility_to_proto),
    }
}

fn visibility_to_proto(v: &DecodedVisibility) -> proto::Visibility {
    let value = match &v.value {
        DecodedVisibilityUnit::Meters(opt) => {
            Some(proto::visibility::Value::Meters(proto::OptionalMeters {
                value: optional_u32(*opt),
            }))
        }
        DecodedVisibilityUnit::StatuteMiles(sm) => Some(proto::visibility::Value::StatuteMiles(
            statute_miles_to_proto(sm),
        )),
    };
    proto::Visibility {
        value,
        direction: v
            .direction
            .as_ref()
            .map(cardinal_direction_to_proto)
            .map(|d| d as i32),
        no_directional_variation: v.ndv,
    }
}

fn statute_miles_to_proto(s: &DecodedStatuteMilesVisibility) -> proto::StatuteMilesVisibility {
    proto::StatuteMilesVisibility {
        whole: s.whole,
        fraction: s.fraction.map(|(n, d)| proto::StatuteMilesFraction {
            numerator: n,
            denominator: d,
        }),
        modifier: s
            .modifier
            .as_ref()
            .map(distance_modifier_to_proto)
            .map(|m| m as i32),
    }
}

fn distance_modifier_to_proto(m: &DecodedDistanceModifier) -> proto::DistanceModifier {
    match m {
        DecodedDistanceModifier::LessThan => proto::DistanceModifier::LessThan,
        DecodedDistanceModifier::GreaterThan => proto::DistanceModifier::GreaterThan,
    }
}

fn cardinal_direction_to_proto(d: &DecodedDirection) -> proto::CardinalDirection {
    match d {
        DecodedDirection::N => proto::CardinalDirection::N,
        DecodedDirection::NE => proto::CardinalDirection::Ne,
        DecodedDirection::E => proto::CardinalDirection::E,
        DecodedDirection::SE => proto::CardinalDirection::Se,
        DecodedDirection::S => proto::CardinalDirection::S,
        DecodedDirection::SW => proto::CardinalDirection::Sw,
        DecodedDirection::W => proto::CardinalDirection::W,
        DecodedDirection::NW => proto::CardinalDirection::Nw,
    }
}

fn rvr_to_proto(r: &DecodedRvr) -> proto::Rvr {
    proto::Rvr {
        runway: r.runway.clone(),
        meters: optional_u32(r.value),
        modifier: r
            .distance_modifier
            .as_ref()
            .map(distance_modifier_to_proto)
            .map(|m| m as i32),
        trend: r
            .comment
            .as_ref()
            .map(visibility_trend_to_proto)
            .map(|t| t as i32),
    }
}

fn visibility_trend_to_proto(t: &DecodedTrend) -> proto::VisibilityTrend {
    match t {
        DecodedTrend::Increasing => proto::VisibilityTrend::Upward,
        DecodedTrend::Decreasing => proto::VisibilityTrend::Downward,
        DecodedTrend::NoDistinctChange => proto::VisibilityTrend::Neutral,
    }
}

fn vertical_visibility_to_proto(v: &DecodedVerticalVisibility) -> proto::VerticalVisibility {
    proto::VerticalVisibility {
        hundreds_of_feet: optional_u32(v.visibility),
    }
}

fn cloud_to_proto(c: &DecodedCloud) -> proto::Cloud {
    let variant = match c {
        DecodedCloud::NCD => Some(proto::cloud::Variant::Ncd(())),
        DecodedCloud::NSC => Some(proto::cloud::Variant::Nsc(())),
        DecodedCloud::CLR => Some(proto::cloud::Variant::Clr(())),
        DecodedCloud::CloudData(d) => Some(proto::cloud::Variant::Data(cloud_data_to_proto(d))),
    };
    proto::Cloud { variant }
}

fn cloud_data_to_proto(d: &DecodedCloudData) -> proto::CloudData {
    proto::CloudData {
        coverage: match &d.coverage {
            OptionalData::Undefined => None,
            OptionalData::Data(c) => Some(cloud_coverage_to_proto(c) as i32),
        },
        height_hundreds_ft: match &d.height {
            OptionalData::Undefined => None,
            OptionalData::Data(h) => Some(h.height),
        },
        cloud_type: d.cloud_type.as_ref().map(|opt| proto::CloudType {
            known: match opt {
                OptionalData::Undefined => None,
                OptionalData::Data(s) => Some(s.clone()),
            },
        }),
    }
}

fn cloud_coverage_to_proto(c: &DecodedCloudCoverage) -> proto::CloudCoverage {
    match c {
        DecodedCloudCoverage::Few => proto::CloudCoverage::Few,
        DecodedCloudCoverage::Scattered => proto::CloudCoverage::Scattered,
        DecodedCloudCoverage::Broken => proto::CloudCoverage::Broken,
        DecodedCloudCoverage::Overcast => proto::CloudCoverage::Overcast,
    }
}

fn present_weather_to_proto(p: &DecodedPresentWeather) -> proto::PresentWeather {
    proto::PresentWeather {
        intensity: p
            .intensity
            .as_ref()
            .map(weather_intensity_to_proto)
            .map(|i| i as i32),
        descriptor: p
            .descriptor
            .as_ref()
            .map(weather_descriptor_to_proto)
            .map(|d| d as i32),
        phenomena: p
            .phenomena
            .iter()
            .map(|opt| proto::WeatherPhenomenonValue {
                code: match opt {
                    OptionalData::Undefined => None,
                    OptionalData::Data(p) => Some(weather_phenomenon_to_proto(p) as i32),
                },
            })
            .collect(),
    }
}

fn weather_intensity_to_proto(i: &DecodedWeatherIntensity) -> proto::WeatherIntensity {
    match i {
        DecodedWeatherIntensity::Light => proto::WeatherIntensity::Light,
        DecodedWeatherIntensity::Heavy => proto::WeatherIntensity::Heavy,
        DecodedWeatherIntensity::Vicinity => proto::WeatherIntensity::Vicinity,
    }
}

fn weather_descriptor_to_proto(d: &DecodedQualifier) -> proto::WeatherDescriptor {
    match d {
        DecodedQualifier::Shallow => proto::WeatherDescriptor::Shallow,
        DecodedQualifier::Patches => proto::WeatherDescriptor::Patches,
        DecodedQualifier::Partial => proto::WeatherDescriptor::Partial,
        DecodedQualifier::Low => proto::WeatherDescriptor::LowDrifting,
        DecodedQualifier::Blowing => proto::WeatherDescriptor::Blowing,
        DecodedQualifier::Showers => proto::WeatherDescriptor::Showers,
        DecodedQualifier::Thunderstorm => proto::WeatherDescriptor::Thunderstorm,
        DecodedQualifier::Freezing => proto::WeatherDescriptor::Freezing,
    }
}

fn weather_phenomenon_to_proto(p: &DecodedWeatherPhenomenon) -> proto::WeatherPhenomenon {
    use DecodedWeatherPhenomenon::*;
    match p {
        DZ => proto::WeatherPhenomenon::Dz,
        RA => proto::WeatherPhenomenon::Ra,
        SN => proto::WeatherPhenomenon::Sn,
        SG => proto::WeatherPhenomenon::Sg,
        PL => proto::WeatherPhenomenon::Pl,
        GR => proto::WeatherPhenomenon::Gr,
        GS => proto::WeatherPhenomenon::Gs,
        UP => proto::WeatherPhenomenon::Up,
        BR => proto::WeatherPhenomenon::Br,
        FG => proto::WeatherPhenomenon::Fg,
        FU => proto::WeatherPhenomenon::Fu,
        VA => proto::WeatherPhenomenon::Va,
        DU => proto::WeatherPhenomenon::Du,
        SA => proto::WeatherPhenomenon::Sa,
        HZ => proto::WeatherPhenomenon::Hz,
        PO => proto::WeatherPhenomenon::Po,
        SQ => proto::WeatherPhenomenon::Sq,
        FC => proto::WeatherPhenomenon::Fc,
        SS => proto::WeatherPhenomenon::Ss,
        DS => proto::WeatherPhenomenon::Ds,
    }
}

fn temperature_to_proto(t: &TemperatureInfo) -> proto::Temperature {
    proto::Temperature {
        celsius: optional_i32(t.temp),
        dew_point_celsius: optional_i32(t.dew_point),
    }
}

fn pressure_to_proto(p: &DecodedPressure) -> proto::Pressure {
    proto::Pressure {
        qnh: p.qnh.as_ref().map(pressure_single_to_proto),
        altimeter: p.altimeter.as_ref().map(pressure_single_to_proto),
    }
}

fn pressure_single_to_proto(p: &DecodedPressureSingle) -> proto::PressureReading {
    proto::PressureReading {
        unit: pressure_unit_to_proto(p.unit) as i32,
        value: optional_u32(p.value),
    }
}

fn pressure_unit_to_proto(u: DecodedPressureUnit) -> proto::PressureUnit {
    match u {
        DecodedPressureUnit::Hectopascals => proto::PressureUnit::Hpa,
        DecodedPressureUnit::InchesOfMercury => proto::PressureUnit::InhgHundredths,
    }
}

fn optional_u32<const N: usize>(o: OptionalData<u32, N>) -> Option<u32> {
    match o {
        OptionalData::Undefined => None,
        OptionalData::Data(v) => Some(v),
    }
}

fn optional_i32<const N: usize>(o: OptionalData<i32, N>) -> Option<i32> {
    match o {
        OptionalData::Undefined => None,
        OptionalData::Data(v) => Some(v),
    }
}

// ─── Airport / RunwayInfo / Selection ────────────────────────────────────────

/// Build a [`proto::AirportRequest`] from a parsed [`Airport`].
///
/// `atis_runways` is the per-airport ATIS-derived selection the host has
/// already produced (may be empty).
pub fn airport_to_request(
    airport: &Airport,
    atis_runways: &indexmap::IndexMap<String, RunwayUse>,
) -> proto::AirportRequest {
    let runways = airport
        .runways
        .iter()
        .flat_map(|rwy| rwy.runways.iter())
        .map(|dir| runway_direction_to_proto(airport, dir))
        .collect();

    let atis_runways = atis_runways
        .iter()
        .map(|(id, u)| proto::RunwayAssignment {
            identifier: id.clone(),
            r#use: runway_use_to_proto(*u) as i32,
        })
        .collect();

    proto::AirportRequest {
        icao: airport.icao.clone(),
        runways,
        metar: airport.metar.as_ref().map(metar_to_proto),
        atis_runways,
    }
}

fn runway_direction_to_proto(airport: &Airport, dir: &RunwayDirection) -> proto::RunwayInfo {
    let wind_components = airport
        .runway_wind_components(dir)
        .map(|c| proto::WindComponents {
            headwind_kt: c.headwind,
            crosswind_kt: c.crosswind.max(0) as u32,
            crosswind_direction: match c.crosswind_direction {
                CrosswindDirection::Left => proto::CrosswindDirection::Left as i32,
                CrosswindDirection::Right => proto::CrosswindDirection::Right as i32,
                CrosswindDirection::Variable => proto::CrosswindDirection::Variable as i32,
            },
        });
    proto::RunwayInfo {
        identifier: dir.identifier.clone(),
        heading_degrees_true: dir.degrees as u32,
        wind_components,
    }
}

pub fn runway_use_to_proto(u: RunwayUse) -> proto::RunwayUse {
    match u {
        RunwayUse::Departing => proto::RunwayUse::Departing,
        RunwayUse::Arriving => proto::RunwayUse::Arriving,
        RunwayUse::Both => proto::RunwayUse::Both,
    }
}

pub fn runway_use_from_proto(u: proto::RunwayUse) -> Option<RunwayUse> {
    match u {
        proto::RunwayUse::Departing => Some(RunwayUse::Departing),
        proto::RunwayUse::Arriving => Some(RunwayUse::Arriving),
        proto::RunwayUse::Both => Some(RunwayUse::Both),
        proto::RunwayUse::Unspecified => None,
    }
}

pub fn selection_source_from_proto(s: proto::SelectionSource) -> Option<RunwayInUseSource> {
    match s {
        proto::SelectionSource::Atis => Some(RunwayInUseSource::Atis),
        proto::SelectionSource::Metar => Some(RunwayInUseSource::Metar),
        proto::SelectionSource::Default => Some(RunwayInUseSource::Default),
        proto::SelectionSource::Unspecified => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metar_decoder::metar::Metar;
    use std::str::FromStr;

    #[test]
    fn converts_simple_metar() {
        let m = Metar::from_str("ENBR 111150Z 25006KT 9999 VCSH FEW005 SCT011 BKN014 12/10 Q1026")
            .unwrap();
        let p = metar_to_proto(&m);

        assert_eq!(p.icao, "ENBR");
        assert!(p.observation_time.is_some());
        let wind = p.wind.unwrap();
        match wind.direction.unwrap() {
            proto::wind::Direction::Heading(d) => assert_eq!(d.degrees, Some(250)),
            _ => panic!("expected heading"),
        }
        assert_eq!(wind.speed.unwrap().value, Some(6));

        let obs = p.obscuration.unwrap();
        match obs.variant.unwrap() {
            proto::obscuration::Variant::Described(d) => {
                let vis = d.visibility.unwrap().value.unwrap();
                match vis {
                    proto::visibility::Value::Meters(m) => assert_eq!(m.value, Some(9999)),
                    _ => panic!("expected meters"),
                }
                assert_eq!(d.clouds.len(), 3);
                assert_eq!(d.present_weather.len(), 1);
            }
            _ => panic!("expected described obscuration"),
        }

        let t = p.temperature.unwrap();
        assert_eq!(t.celsius, Some(12));
        assert_eq!(t.dew_point_celsius, Some(10));

        let pr = p.pressure.unwrap();
        assert_eq!(pr.qnh.unwrap().value, Some(1026));
    }

    #[test]
    fn cavok_round_trips() {
        let m = Metar::from_str("ENZV 011200Z 05010KT CAVOK 20/20 Q1013").unwrap();
        let p = metar_to_proto(&m);
        let obs = p.obscuration.unwrap();
        assert!(matches!(
            obs.variant.unwrap(),
            proto::obscuration::Variant::Cavok(())
        ));
    }

    #[test]
    fn variable_wind_maps_to_variable() {
        let m = Metar::from_str("ENGM 080920Z VRB03KT 9999 OVC009 M09/M12 Q1024").unwrap();
        let p = metar_to_proto(&m);
        match p.wind.unwrap().direction.unwrap() {
            proto::wind::Direction::Variable(()) => {}
            _ => panic!("expected variable wind"),
        }
    }

    #[test]
    fn vertical_visibility_present() {
        let m = Metar::from_str("ENZV 111920Z 30010KT 4000 -DZ BR VV007 13/12 Q1027").unwrap();
        let p = metar_to_proto(&m);
        let obs = p.obscuration.unwrap();
        match obs.variant.unwrap() {
            proto::obscuration::Variant::Described(d) => {
                let vv = d.vertical_visibility.expect("VV present");
                assert_eq!(vv.hundreds_of_feet, Some(7));
            }
            _ => panic!("expected described"),
        }
    }

    #[test]
    fn unknown_values_round_trip_as_none() {
        let m = Metar::from_str("EGWC 121350Z AUTO 05002KT //// ///////// ///// Q////").unwrap();
        let p = metar_to_proto(&m);
        let pr = p.pressure.unwrap();
        assert_eq!(pr.qnh.unwrap().value, None);
    }
}
