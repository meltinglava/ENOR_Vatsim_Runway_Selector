//! Conversions from internal metar_decoder types to runway_selector_protocol types.

use metar_decoder::{
    metar::Metar,
    obscuration::{
        Cloud, CloudCoverage as InternalCoverage, Obscuration, PresentWeather, Qualifier,
        VisibilityUnit, WeatherIntensity, WeatherPhenomenon,
    },
    optional_data::OptionalData,
    units::velocity::VelocityUnit,
    wind::WindDirection as InternalWindDir,
};
use runway_selector_protocol::{
    AirportInfo, CloudCoverage, CloudLayer, MetarData, PhysicalRunway, RunwayInfo, RunwayUse,
    VariableWind, WindData, WindDirection,
};

use crate::runway::{Runway, RunwayUse as InternalRunwayUse};

pub(crate) fn metar_to_protocol(metar: &Metar) -> MetarData {
    let wind = build_wind(metar);
    let (temp_c, dew_point_c) = build_temps(metar);
    let qnh_hpa = build_qnh(metar);
    let (visibility_m, clouds, rvr_reported, vertical_visibility_ft, present_weather) =
        build_obscuration(metar);

    MetarData {
        raw: metar.raw.clone(),
        icao: metar.icao.clone(),
        wind,
        temp_c,
        dew_point_c,
        qnh_hpa,
        visibility_m,
        clouds,
        rvr_reported,
        vertical_visibility_ft,
        present_weather,
    }
}

fn build_wind(metar: &Metar) -> Option<WindData> {
    let wind = &metar.wind;
    let speed_kt: f64 = wind_speed_to_kt(wind.speed.velocity.to_option()?, wind.speed.unit);

    let gust_kt = wind
        .speed
        .gust
        .and_then(|g| g.to_option())
        .map(|g| wind_speed_to_kt(g, wind.speed.unit));

    let direction = match &wind.dir {
        InternalWindDir::Variable => WindDirection::Variable,
        InternalWindDir::Heading(track) => match track.0 {
            OptionalData::Data(deg) => WindDirection::Heading {
                degrees: deg as u16,
            },
            OptionalData::Undefined => WindDirection::Variable,
        },
    };

    // Track implements Clone but not Copy, so clone the Option.
    let variable_sector = wind.varying.clone().and_then(|(s, e)| {
        let from = s.0.to_option()? as u16;
        let to = e.0.to_option()? as u16;
        Some(VariableWind {
            from_degrees: from,
            to_degrees: to,
        })
    });

    Some(WindData {
        direction,
        speed_kt,
        gust_kt,
        variable_sector,
    })
}

fn wind_speed_to_kt(value: u32, unit: VelocityUnit) -> f64 {
    match unit {
        VelocityUnit::Knots => value as f64,
        VelocityUnit::MetersPerSecond => value as f64 * 1.944,
    }
}

fn build_temps(metar: &Metar) -> (Option<i32>, Option<i32>) {
    let temp = metar.temperature.temp.to_option();
    let dew = metar.temperature.dew_point.to_option();
    (temp, dew)
}

fn build_qnh(metar: &Metar) -> Option<u32> {
    use metar_decoder::pressure::PressureUnit;
    let p = metar.pressure.qnh.as_ref()?;
    match p.unit {
        PressureUnit::Hectopascals => p.value.to_option(),
        PressureUnit::InchesOfMercury => None,
    }
}

fn build_obscuration(
    metar: &Metar,
) -> (Option<u32>, Vec<CloudLayer>, bool, Option<u32>, Vec<String>) {
    let Obscuration::Described(ref desc) = metar.obscuration else {
        // CAVOK: unlimited visibility, no clouds, no RVR
        return (None, Vec::new(), false, None, Vec::new());
    };

    let visibility_m = match desc.visibility.value {
        VisibilityUnit::Meters(OptionalData::Data(v)) => Some(v),
        _ => None,
    };

    let clouds = desc
        .clouds
        .iter()
        .filter_map(|c| match c {
            Cloud::CloudData(cd) => {
                let coverage = match &cd.coverage {
                    OptionalData::Data(cov) => match cov {
                        InternalCoverage::Few => CloudCoverage::Few,
                        InternalCoverage::Scattered => CloudCoverage::Scattered,
                        InternalCoverage::Broken => CloudCoverage::Broken,
                        InternalCoverage::Overcast => CloudCoverage::Overcast,
                    },
                    OptionalData::Undefined => CloudCoverage::Unknown,
                };
                let height_ft = match &cd.height {
                    // CloudHeight stores the value in hundreds of feet.
                    OptionalData::Data(h) => Some(h.height as u32 * 100),
                    OptionalData::Undefined => None,
                };
                let is_cumulonimbus = matches!(
                    &cd.cloud_type,
                    Some(OptionalData::Data(s)) if s == "CB"
                );
                Some(CloudLayer {
                    coverage,
                    height_ft,
                    is_cumulonimbus,
                })
            }
            Cloud::NCD | Cloud::NSC | Cloud::CLR => None,
        })
        .collect();

    let rvr_reported = !desc.rvr.is_empty();

    // VerticalVisibility stores the value in hundreds of feet.
    let vertical_visibility_ft = desc
        .vertical_visibility
        .as_ref()
        .and_then(|vv| vv.visibility.to_option())
        .map(|v| v * 100);

    let present_weather = desc
        .present_weather
        .iter()
        .map(format_present_weather)
        .collect();

    (
        visibility_m,
        clouds,
        rvr_reported,
        vertical_visibility_ft,
        present_weather,
    )
}

fn format_present_weather(pw: &PresentWeather) -> String {
    let mut s = String::new();
    match &pw.intensity {
        Some(WeatherIntensity::Light) => s.push('-'),
        Some(WeatherIntensity::Heavy) => s.push('+'),
        Some(WeatherIntensity::Vicinity) => s.push_str("VC"),
        None => {}
    }
    if let Some(q) = &pw.descriptor {
        s.push_str(match q {
            Qualifier::Shallow => "MI",
            Qualifier::Patches => "BC",
            Qualifier::Partial => "PR",
            Qualifier::Low => "DR",
            Qualifier::Blowing => "BL",
            Qualifier::Showers => "SH",
            Qualifier::Thunderstorm => "TS",
            Qualifier::Freezing => "FZ",
        });
    }
    for phenom in &pw.phenomena {
        if let OptionalData::Data(p) = phenom {
            s.push_str(match p {
                WeatherPhenomenon::DZ => "DZ",
                WeatherPhenomenon::RA => "RA",
                WeatherPhenomenon::SN => "SN",
                WeatherPhenomenon::SG => "SG",
                WeatherPhenomenon::PL => "PL",
                WeatherPhenomenon::GR => "GR",
                WeatherPhenomenon::GS => "GS",
                WeatherPhenomenon::UP => "UP",
                WeatherPhenomenon::BR => "BR",
                WeatherPhenomenon::FG => "FG",
                WeatherPhenomenon::FU => "FU",
                WeatherPhenomenon::VA => "VA",
                WeatherPhenomenon::DU => "DU",
                WeatherPhenomenon::SA => "SA",
                WeatherPhenomenon::HZ => "HZ",
                WeatherPhenomenon::PO => "PO",
                WeatherPhenomenon::SQ => "SQ",
                WeatherPhenomenon::FC => "FC",
                WeatherPhenomenon::SS => "SS",
                WeatherPhenomenon::DS => "DS",
            });
        }
    }
    s
}

pub(crate) fn airport_to_protocol_info(icao: &str, runways: &[Runway]) -> AirportInfo {
    let runways = runways
        .iter()
        .map(|rw| {
            let primary = RunwayInfo {
                identifier: rw.primary.identifier.clone(),
                degrees: rw.primary.degrees,
            };
            let reciprocal = rw.reciprocal.as_ref().map(|r| RunwayInfo {
                identifier: r.identifier.clone(),
                degrees: r.degrees,
            });
            PhysicalRunway {
                primary,
                reciprocal,
            }
        })
        .collect();
    AirportInfo {
        icao: icao.to_string(),
        runways,
    }
}

pub(crate) fn protocol_to_internal_use(use_: RunwayUse) -> InternalRunwayUse {
    match use_ {
        RunwayUse::Departing => InternalRunwayUse::Departing,
        RunwayUse::Arriving => InternalRunwayUse::Arriving,
        RunwayUse::Both => InternalRunwayUse::Both,
        _ => {
            tracing::warn!(
                ?use_,
                "unrecognised RunwayUse variant from plugin; treating as Both"
            );
            InternalRunwayUse::Both
        }
    }
}
