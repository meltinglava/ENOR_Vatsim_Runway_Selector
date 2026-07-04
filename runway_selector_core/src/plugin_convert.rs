//! Conversions from [`metar_decoder`] domain types into the HTTP/JSON
//! plugin-contract types in [`runway_plugin_api`].
//!
//! The host parses METARs locally with `metar_decoder` and pre-computes the
//! per-runway wind components **once**, then ships both to area plugins as
//! JSON. This module is the bridge; plugins never re-do wind trigonometry
//! or METAR parsing.
//!
//! Fields the plugin contract intentionally omits (BECMG, TEMPO, NATO mil
//! code, sea-surface indicator, directional visibility) are dropped on the
//! floor here — no plugin currently needs them.

use metar_decoder::{
    metar::Metar as DecodedMetar,
    obscuration::{
        Cloud as DecodedCloud, CloudCoverage as DecodedCloudCoverage,
        DescribedObscuration as DecodedDescribedObscuration, Obscuration as DecodedObscuration,
        PresentWeather as DecodedPresentWeather, Qualifier as DecodedQualifier,
        VisibilityUnit as DecodedVisibilityUnit, WeatherIntensity as DecodedWeatherIntensity,
    },
    pressure::{PressureSingle as DecodedPressureSingle, PressureUnit as DecodedPressureUnit},
    wind::{Wind as DecodedWind, WindDirection as DecodedWindDirection},
};

use crate::airport::{Airport, CrosswindDirection, RunwayInUseSource};
use crate::runway::{RunwayDirection, RunwayUse};

use runway_plugin_api as api;

const KNOTS_PER_METER_PER_SECOND: f64 = 1.943_844;

pub fn metar_to_wire(m: &DecodedMetar) -> api::MetarData {
    api::MetarData {
        raw: m.raw.clone(),
        parsed: Some(parsed_metar_to_wire(m)),
    }
}

fn parsed_metar_to_wire(m: &DecodedMetar) -> api::ParsedMetar {
    let described = match &m.obscuration {
        DecodedObscuration::Cavok => None,
        DecodedObscuration::Described(d) => Some(d),
    };

    api::ParsedMetar {
        is_cavok: described.is_none(),
        wind: wind_to_wire(&m.wind),
        visibility_meters: described.and_then(visibility_meters),
        rvr: described.map(rvr_to_wire).unwrap_or_default(),
        clouds: described.map(clouds_to_wire).unwrap_or_default(),
        vertical_visibility_hundreds_ft: described.and_then(|d| {
            d.vertical_visibility
                .as_ref()
                // VV present: unreadable "VV///" is encoded as 0 per contract.
                .map(|vv| vv.visibility.to_option().unwrap_or(0) as i32)
        }),
        weather_phenomena: described
            .map(|d| {
                d.present_weather
                    .iter()
                    .map(present_weather_to_wire)
                    .collect()
            })
            .unwrap_or_default(),
        temperature_c: m.temperature.temp.to_option(),
        dew_point_c: m.temperature.dew_point.to_option(),
        qnh_hpa: qnh_hpa(m),
    }
}

fn wind_to_wire(w: &DecodedWind) -> Option<api::WindData> {
    // Speed reported as "//" means we know nothing useful about the wind.
    let speed_raw = w.speed.velocity.to_option()?;
    let to_knots = |v: u32| match w.speed.unit {
        metar_decoder::units::velocity::VelocityUnit::Knots => v,
        metar_decoder::units::velocity::VelocityUnit::MetersPerSecond => {
            (f64::from(v) * KNOTS_PER_METER_PER_SECOND).round() as u32
        }
    };

    let (direction_degrees, is_variable) = match &w.dir {
        DecodedWindDirection::Variable => (None, true),
        DecodedWindDirection::Heading(track) => (track.0.to_option(), false),
    };

    let (variable_from_degrees, variable_to_degrees) = w
        .varying
        .as_ref()
        .map(|(from, to)| (from.0.to_option(), to.0.to_option()))
        .unwrap_or((None, None));

    Some(api::WindData {
        direction_degrees,
        is_variable,
        speed_kt: to_knots(speed_raw),
        gust_kt: w.speed.gust.and_then(|g| g.to_option()).map(to_knots),
        variable_from_degrees,
        variable_to_degrees,
    })
}

fn visibility_meters(d: &DecodedDescribedObscuration) -> Option<u32> {
    match &d.visibility.value {
        DecodedVisibilityUnit::Meters(opt) => opt.to_option(),
        // US statute-mile form: not converted; plugins that care can parse
        // `raw`. No supported area reports statute miles today.
        DecodedVisibilityUnit::StatuteMiles(_) => None,
    }
}

fn rvr_to_wire(d: &DecodedDescribedObscuration) -> Vec<api::RvrData> {
    d.rvr
        .iter()
        .map(|r| api::RvrData {
            runway: r.runway.clone(),
            meters: r.value.to_option(),
        })
        .collect()
}

fn clouds_to_wire(d: &DecodedDescribedObscuration) -> Vec<api::CloudData> {
    d.clouds
        .iter()
        .filter_map(|c| match c {
            DecodedCloud::NCD | DecodedCloud::NSC | DecodedCloud::CLR => None,
            DecodedCloud::CloudData(data) => Some(api::CloudData {
                coverage: data.coverage.clone().to_option().map(|c| match c {
                    DecodedCloudCoverage::Few => api::CloudCoverage::Few,
                    DecodedCloudCoverage::Scattered => api::CloudCoverage::Scattered,
                    DecodedCloudCoverage::Broken => api::CloudCoverage::Broken,
                    DecodedCloudCoverage::Overcast => api::CloudCoverage::Overcast,
                }),
                height_hundreds_ft: data.height.clone().to_option().map(|h| h.height),
                cloud_type: data.cloud_type.clone().and_then(|t| t.to_option()),
            }),
        })
        .collect()
}

fn present_weather_to_wire(p: &DecodedPresentWeather) -> api::WeatherPhenomenonData {
    api::WeatherPhenomenonData {
        intensity: p.intensity.as_ref().map(|i| match i {
            DecodedWeatherIntensity::Light => api::WeatherIntensity::Light,
            DecodedWeatherIntensity::Heavy => api::WeatherIntensity::Heavy,
            DecodedWeatherIntensity::Vicinity => api::WeatherIntensity::Vicinity,
        }),
        descriptors: p
            .descriptor
            .as_ref()
            .map(|d| {
                vec![match d {
                    DecodedQualifier::Shallow => api::WeatherDescriptor::Shallow,
                    DecodedQualifier::Patches => api::WeatherDescriptor::Patches,
                    DecodedQualifier::Partial => api::WeatherDescriptor::Partial,
                    DecodedQualifier::Low => api::WeatherDescriptor::LowDrifting,
                    DecodedQualifier::Blowing => api::WeatherDescriptor::Blowing,
                    DecodedQualifier::Showers => api::WeatherDescriptor::Shower,
                    DecodedQualifier::Thunderstorm => api::WeatherDescriptor::Thunderstorm,
                    DecodedQualifier::Freezing => api::WeatherDescriptor::Freezing,
                }]
            })
            .unwrap_or_default(),
        phenomena: p
            .phenomena
            .iter()
            .filter_map(|opt| opt.clone().to_option())
            // The decoder's variant names are exactly the two-letter METAR
            // codes (DZ, RA, SN, …), which is the wire format.
            .map(|code| format!("{code:?}"))
            .collect(),
    }
}

fn qnh_hpa(m: &DecodedMetar) -> Option<u32> {
    let reading_to_hpa = |r: &DecodedPressureSingle| {
        let value = r.value.to_option()?;
        Some(match r.unit {
            DecodedPressureUnit::Hectopascals => value,
            // Reported in hundredths of inHg (e.g. 2992 → 29.92 inHg).
            DecodedPressureUnit::InchesOfMercury => {
                (f64::from(value) / 100.0 * 33.863_886).round() as u32
            }
        })
    };
    m.pressure
        .qnh
        .as_ref()
        .and_then(reading_to_hpa)
        .or_else(|| m.pressure.altimeter.as_ref().and_then(reading_to_hpa))
}

// ─── Airport / RunwayInfo / Selection ────────────────────────────────────────

/// Build an [`api::AirportSelectionRequest`] from a parsed [`Airport`].
///
/// Wind components are computed here, once, on the host — plugins receive
/// headwind / tailwind / crosswind per runway direction and never do the
/// trigonometry themselves.
pub fn airport_to_request(airport: &Airport) -> api::AirportSelectionRequest {
    let runways = airport
        .runways
        .iter()
        .flat_map(|rwy| rwy.runways.iter())
        .map(|dir| runway_direction_to_wire(airport, dir))
        .collect();

    api::AirportSelectionRequest {
        icao: airport.icao.clone(),
        runways,
        metar: airport.metar.as_ref().map(metar_to_wire),
    }
}

fn runway_direction_to_wire(airport: &Airport, dir: &RunwayDirection) -> api::RunwayInfo {
    let headwind = airport.runway_max_headwind(dir);
    let tailwind = airport.runway_max_tailwind(dir);
    let crosswind = airport.runway_max_crosswind(dir);
    api::RunwayInfo {
        identifier: dir.identifier.clone(),
        heading: dir.degrees,
        headwind_kt: headwind,
        tailwind_kt: tailwind,
        crosswind_kt: crosswind.map(|(magnitude, _)| magnitude.max(0)),
        crosswind_direction: crosswind.map(|(_, direction)| match direction {
            CrosswindDirection::Left => api::CrosswindDirection::Left,
            CrosswindDirection::Right => api::CrosswindDirection::Right,
            CrosswindDirection::Variable => api::CrosswindDirection::Variable,
        }),
    }
}

pub fn runway_use_to_wire(u: RunwayUse) -> api::RunwayUse {
    match u {
        RunwayUse::Departing => api::RunwayUse::Departing,
        RunwayUse::Arriving => api::RunwayUse::Arriving,
        RunwayUse::Both => api::RunwayUse::Both,
    }
}

pub fn runway_use_from_wire(u: api::RunwayUse) -> RunwayUse {
    match u {
        api::RunwayUse::Departing => RunwayUse::Departing,
        api::RunwayUse::Arriving => RunwayUse::Arriving,
        api::RunwayUse::Both => RunwayUse::Both,
    }
}

pub fn selection_source_from_wire(s: api::SelectionSource) -> RunwayInUseSource {
    match s {
        api::SelectionSource::Metar => RunwayInUseSource::Metar,
        api::SelectionSource::Default => RunwayInUseSource::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metar_decoder::metar::Metar;
    use std::str::FromStr;

    fn parsed(raw: &str) -> api::ParsedMetar {
        let m = Metar::from_str(raw).unwrap();
        parsed_metar_to_wire(&m)
    }

    #[test]
    fn converts_simple_metar() {
        let p = parsed("ENBR 111150Z 25006KT 9999 VCSH FEW005 SCT011 BKN014 12/10 Q1026");

        assert!(!p.is_cavok);
        let wind = p.wind.unwrap();
        assert_eq!(wind.direction_degrees, Some(250));
        assert_eq!(wind.speed_kt, 6);
        assert!(!wind.is_variable);

        assert_eq!(p.visibility_meters, Some(9999));
        assert_eq!(p.clouds.len(), 3);
        assert_eq!(p.weather_phenomena.len(), 1);
        // "VCSH" = showers in the vicinity: descriptor SH, no phenomenon code.
        assert_eq!(
            p.weather_phenomena[0].intensity,
            Some(api::WeatherIntensity::Vicinity)
        );
        assert_eq!(
            p.weather_phenomena[0].descriptors,
            vec![api::WeatherDescriptor::Shower]
        );
        assert_eq!(p.temperature_c, Some(12));
        assert_eq!(p.dew_point_c, Some(10));
        assert_eq!(p.qnh_hpa, Some(1026));
    }

    #[test]
    fn cavok_sets_flag_and_empties_weather() {
        let p = parsed("ENZV 011200Z 05010KT CAVOK 20/20 Q1013");
        assert!(p.is_cavok);
        assert_eq!(p.visibility_meters, None);
        assert!(p.clouds.is_empty());
        assert!(p.rvr.is_empty());
    }

    #[test]
    fn variable_wind_maps_to_variable() {
        let p = parsed("ENGM 080920Z VRB03KT 9999 OVC009 M09/M12 Q1024");
        let wind = p.wind.unwrap();
        assert!(wind.is_variable);
        assert_eq!(wind.direction_degrees, None);
        assert_eq!(wind.speed_kt, 3);
    }

    #[test]
    fn mps_wind_converts_to_knots() {
        let p = parsed("UUEE 111150Z 25005MPS 9999 OVC009 05/03 Q1010");
        let wind = p.wind.unwrap();
        assert_eq!(wind.speed_kt, 10); // 5 m/s ≈ 9.7 kt, rounded
    }

    #[test]
    fn vertical_visibility_present() {
        let p = parsed("ENZV 111920Z 30010KT 4000 -DZ BR VV007 13/12 Q1027");
        assert_eq!(p.vertical_visibility_hundreds_ft, Some(7));
        assert_eq!(p.visibility_meters, Some(4000));
    }

    #[test]
    fn freezing_drizzle_carries_descriptor() {
        let p = parsed("ENGM 111920Z 30010KT 4000 FZDZ OVC003 M01/M02 Q1027");
        let wx = &p.weather_phenomena[0];
        assert_eq!(wx.descriptors, vec![api::WeatherDescriptor::Freezing]);
        assert_eq!(wx.phenomena, vec!["DZ".to_string()]);
    }

    #[test]
    fn unknown_values_map_to_none() {
        let p = parsed("EGWC 121350Z AUTO 05002KT //// ///////// ///// Q////");
        assert_eq!(p.qnh_hpa, None);
        assert_eq!(p.visibility_meters, None);
        assert_eq!(p.temperature_c, None);
    }

    #[test]
    fn inhg_altimeter_converts_to_hpa() {
        let p = parsed("KJFK 111150Z 25006KT 10SM FEW050 22/12 A2992");
        assert_eq!(p.qnh_hpa, Some(1013));
    }
}
