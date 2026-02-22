use indexmap::IndexMap;
use itertools::{
    Itertools,
    MinMaxResult::{MinMax, NoElements, OneElement},
};
use jiff::Zoned;
use metar_decoder::{
    metar::Metar,
    obscuration::{Cloud, CloudCoverage, Obscuration, VisibilityUnit},
    optional_data::OptionalData,
    units::{track::Track, velocity::WindVelocity},
    wind::Wind,
};

use crate::{
    config::ESConfig,
    error::{ApplicationError, ApplicationResult},
    runway::{Runway, RunwayDirection, RunwayUse},
    util::diff_angle,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunwayInUseSource {
    Atis,
    Metar,
    Default,
}

impl RunwayInUseSource {
    pub fn default_sort_order() -> [RunwayInUseSource; 3] {
        [
            RunwayInUseSource::Atis,
            RunwayInUseSource::Metar,
            RunwayInUseSource::Default,
        ]
    }
}

#[derive(Debug)]
pub struct Airport {
    pub icao: String,
    pub metar: Option<Metar>,
    pub runways: Vec<Runway>,
    pub runways_in_use: IndexMap<RunwayInUseSource, IndexMap<String, RunwayUse>>,
}

#[allow(dead_code)] // planned for runway report output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunwayWindComponents {
    pub headwind: i32,
    pub crosswind: i32,
    pub crosswind_direction: CrosswindDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrosswindDirection {
    Left,
    Right,
    Variable,
}

#[derive(Debug)]
enum EngmModes {
    Mixed,
    Segregated,
    Single,
}

impl Airport {
    #[allow(dead_code)] // planned for runway report output
    pub fn runway_wind_components(
        &self,
        runway_direction: &RunwayDirection,
    ) -> Option<RunwayWindComponents> {
        let headwind = self.runway_max_headwind(runway_direction)?;
        let (crosswind, crosswind_direction) = self.runway_max_crosswind(runway_direction)?;

        Some(RunwayWindComponents {
            headwind,
            crosswind,
            crosswind_direction,
        })
    }

    pub fn runway_max_headwind(&self, runway_direction: &RunwayDirection) -> Option<i32> {
        let metar = self.metar.as_ref()?;
        Self::calculate_max_headwind_from_wind(runway_direction, metar.wind.clone())
    }

    #[allow(dead_code)] // planned for runway report output
    pub fn runway_max_tailwind(&self, runway_direction: &RunwayDirection) -> Option<i32> {
        let metar = self.metar.as_ref()?;
        Self::calculate_max_tailwind_from_wind(runway_direction, metar.wind.clone())
    }

    pub fn runway_max_crosswind(
        &self,
        runway_direction: &RunwayDirection,
    ) -> Option<(i32, CrosswindDirection)> {
        let metar = self.metar.as_ref()?;
        Self::calculate_max_crosswind_from_wind(runway_direction, &metar.wind)
    }

    pub fn set_runway_based_on_metar_wind(&self) -> ApplicationResult<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" {
            unreachable!("ENGM should be dealt with before reaching this step")
        } else if self.icao == "ENZV" {
            self.set_runway_for_enzv()
        } else if self.runways.len() == 1 {
            self.internal_set_runway_based_on_metar_wind(0)
                .ok_or(ApplicationError::NoRunwayToSet)
        } else {
            unreachable!(
                "Airport {} has multiple runways, but no specific logic implemented for it",
                self.icao
            );
        }
    }

    fn internal_set_runway_based_on_metar_wind(
        &self,
        runway_index: usize,
    ) -> Option<IndexMap<String, RunwayUse>> {
        let headwinds = self.runways[runway_index]
            .runways
            .iter()
            .map(|dir| {
                let headwind = self.runway_max_headwind(dir);
                (dir.identifier.clone(), headwind)
            })
            .collect::<IndexMap<_, _>>();

        let valid_headwind_values = headwinds
            .values()
            .filter_map(|v| v.as_ref())
            .collect::<Vec<_>>();
        if valid_headwind_values.is_empty() {
            return None;
        }

        let (min, max) = match valid_headwind_values.iter().cloned().minmax() {
            MinMax(min, max) => (min, max),
            NoElements => return None,
            OneElement(value) => (value, value),
        };

        if (max - min) > 2 {
            let selected = headwinds
                .iter()
                .find(|(_, v)| v.as_ref().map(|w| w == max).unwrap_or(false));
            if let Some((ident, _)) = selected {
                let mut map = IndexMap::new();
                map.insert(ident.clone(), RunwayUse::Both);
                return Some(map);
            }
        }

        None
    }

    pub(crate) fn set_runway_for_engm(
        &self,
        config: &ESConfig,
    ) -> ApplicationResult<(RunwayInUseSource, IndexMap<String, RunwayUse>)> {
        let source;
        let runway_direction: String = match self.internal_set_runway_based_on_metar_wind(0) {
            Some(map) => {
                source = RunwayInUseSource::Metar;
                map.keys().next().unwrap()[..2].to_string()
            }
            None => {
                source = RunwayInUseSource::Default;
                config
                    .get_default_runways()
                    .get(&self.icao)
                    .map(|&rwy| format!("{:02}", rwy))
                    .unwrap_or_else(|| "01".to_string()) // Default to 01 if no default is set
            }
        };

        let mut ceiling_for_lvp = false;
        let mut rvr_reported = false;
        let mut visibility_below_5000 = false;
        let mut reported_vv = false;
        let mut possible_deice_conditions = false;
        let mut forced_deice_condition = false;

        if let Some(metar) = &self.metar
            && let Obscuration::Described(described_obscuration) = &metar.obscuration
        {
            let ceiling_clouds = [CloudCoverage::Broken, CloudCoverage::Overcast];
            ceiling_for_lvp = described_obscuration
                .clouds
                .iter()
                .filter_map(|cloud| match cloud {
                    Cloud::CloudData(cloud_data) => Some(cloud_data),
                    Cloud::NCD | Cloud::NSC | Cloud::CLR => None,
                })
                .filter(|cloud| {
                    if let OptionalData::Data(coverage) = &cloud.coverage {
                        ceiling_clouds.contains(coverage)
                    } else {
                        true // If coverage is undefined, we assume its broken or overcast
                    }
                })
                .any(|cloud| {
                    if let OptionalData::Data(height) = &cloud.height {
                        height.height < 15 // Ceiling below 1500 feet
                    } else {
                        true // If height is undefined, we assume its below 1500 feet
                    }
                });

            rvr_reported = !described_obscuration.rvr.is_empty();

            if let VisibilityUnit::Meters(data) = described_obscuration.visibility.value {
                visibility_below_5000 = if let OptionalData::Data(value) = data {
                    value < 5000
                } else {
                    true
                }
            }

            reported_vv = described_obscuration.vertical_visibility.is_some();

            forced_deice_condition = described_obscuration
                .present_weather
                .iter()
                .cloned()
                .flat_map(|pw| pw.descriptor)
                .any(|descriptor| descriptor == metar_decoder::obscuration::Qualifier::Freezing);

            let contender_for_deice = described_obscuration
                .present_weather
                .iter()
                .cloned()
                .flat_map(|pw| pw.phenomena)
                .any(|phenomenon| {
                    use metar_decoder::obscuration::WeatherPhenomenon::*;
                    phenomenon.to_option().is_some_and(|p| match p {
                        DZ | RA | SN | SG | PL | GR | GS | UP | BR | FG => true,
                        FU | VA | DU | SA | HZ | PO | SQ | FC | SS | DS => false,
                    })
                });

            possible_deice_conditions = match metar.temprature.temp {
                OptionalData::Undefined => contender_for_deice,
                OptionalData::Data(temp) => temp < 5 && contender_for_deice,
            }
        }

        let now = Zoned::now()
            .in_tz("Europe/Oslo")
            .expect("Failed to get timezone Europe/Oslo");
        let mode = if now.date().at(22, 30, 0, 0).in_tz("Europe/Oslo")? <= now {
            EngmModes::Segregated
        } else if now.date().at(6, 30, 0, 0).in_tz("Europe/Oslo")? > now {
            EngmModes::Single // could be segregated / mixed if weather is bad, but currently out of scope
        } else if ceiling_for_lvp
            || rvr_reported
            || visibility_below_5000
            || reported_vv
            || possible_deice_conditions
            || forced_deice_condition
        {
            EngmModes::Segregated
        } else {
            EngmModes::Mixed
        };

        let mut map = IndexMap::new();
        match mode {
            EngmModes::Mixed => {
                map.insert(format!("{}L", runway_direction), RunwayUse::Both);
                map.insert(format!("{}R", runway_direction), RunwayUse::Both);
            }
            EngmModes::Segregated => {
                map.insert(format!("{}L", runway_direction), RunwayUse::Departing);
                map.insert(format!("{}R", runway_direction), RunwayUse::Arriving);
            }
            EngmModes::Single => {
                let runway = match runway_direction.as_str() {
                    "01" => "01L",
                    "19" => "19R",
                    _ => unreachable!(
                        "Runway direction {} is not valid for ENGM",
                        runway_direction
                    ),
                }
                .to_string();
                map.insert(runway, RunwayUse::Both);
            }
        }
        Ok((source, map))
    }

    fn set_runway_for_enzv(&self) -> ApplicationResult<IndexMap<String, RunwayUse>> {
        let main_runway_index = self
            .runways
            .iter()
            .enumerate()
            .filter(|(_, runway)| runway.runways.iter().any(|dir| dir.identifier == "18"))
            .map(|(i, _)| i)
            .next()
            .unwrap();
        let main_runway = match self.internal_set_runway_based_on_metar_wind(main_runway_index) {
            Some(rwy) => rwy.keys().next().unwrap().to_string(),
            None => "18".to_string(),
        };

        let default_fallback = Ok(IndexMap::from([(main_runway.clone(), RunwayUse::Both)]));

        let main_runway_direction = self.runways[main_runway_index]
            .runways
            .iter()
            .find(|dir| dir.identifier == main_runway)
            .unwrap();

        let Some((crosswind, _crosswind_direction)) =
            self.runway_max_crosswind(main_runway_direction)
        else {
            return default_fallback;
        };
        if crosswind < 15 {
            // If crosswind is below 15 knots, we can use the main runway
            return default_fallback;
        }
        let secondary_runway_index = main_runway_index & 1;
        let secondary_runway_crosswind = self
            .runway_max_crosswind(&self.runways[secondary_runway_index].runways[0])
            .map(|(crosswind, _crosswind_direction)| crosswind)
            .unwrap();
        let secondary_runway =
            match self.internal_set_runway_based_on_metar_wind(secondary_runway_index) {
                Some(rwy) => rwy.keys().next().unwrap().to_string(),
                None => return default_fallback,
            };
        if secondary_runway_crosswind < crosswind {
            // If the secondary runway has a lower crosswind, we use it
            let mut map = IndexMap::new();
            map.insert(secondary_runway, RunwayUse::Both);
            Ok(map)
        } else {
            default_fallback
        }
    }

    fn calculate_max_crosswind_from_wind(
        runway: &RunwayDirection,
        wind: &Wind,
    ) -> Option<(i32, CrosswindDirection)> {
        const EPSILON: f64 = 1e-9;
        let track: u32 = runway.degrees as u32;

        let (factor, direction) =
            if let Some((Track(OptionalData::Data(start)), Track(OptionalData::Data(end)))) =
                wind.varying
            {
                let cross_from_right = (track + 90) % 360;
                let cross_from_left = (track + 270) % 360;
                let (start, end): (u32, u32) = (start % 360, end % 360);

                let includes = |angle| {
                    if start <= end {
                        angle >= start && angle <= end
                    } else {
                        angle >= start || angle <= end
                    }
                };

                let includes_right = includes(cross_from_right);
                let includes_left = includes(cross_from_left);

                if includes_right && includes_left {
                    (1.0, CrosswindDirection::Variable)
                } else if includes_right {
                    (1.0, CrosswindDirection::Right)
                } else if includes_left {
                    (1.0, CrosswindDirection::Left)
                } else {
                    let start_component = Self::signed_crosswind_component_factor(track, start);
                    let end_component = Self::signed_crosswind_component_factor(track, end);
                    let start_abs = start_component.abs();
                    let end_abs = end_component.abs();

                    if (start_abs - end_abs).abs() < EPSILON {
                        let direction = if start_component.signum() != end_component.signum() {
                            CrosswindDirection::Variable
                        } else {
                            Self::crosswind_direction_from_signed_factor(start_component)
                        };
                        (start_abs, direction)
                    } else if start_abs > end_abs {
                        (
                            start_abs,
                            Self::crosswind_direction_from_signed_factor(start_component),
                        )
                    } else {
                        (
                            end_abs,
                            Self::crosswind_direction_from_signed_factor(end_component),
                        )
                    }
                }
            } else {
                match &wind.dir {
                    metar_decoder::wind::WindDirection::Heading(wind_dir) => {
                        if let Some(wind_dir) = wind_dir.0.to_option() {
                            let signed_factor =
                                Self::signed_crosswind_component_factor(track, wind_dir);
                            (
                                signed_factor.abs(),
                                Self::crosswind_direction_from_signed_factor(signed_factor),
                            )
                        } else {
                            (1.0, CrosswindDirection::Variable)
                        }
                    }
                    metar_decoder::wind::WindDirection::Variable => {
                        (1.0, CrosswindDirection::Variable)
                    }
                }
            };

        Self::scale_wind_speed(wind.speed, factor).map(|crosswind| (crosswind, direction))
    }

    fn calculate_max_headwind_from_wind(runway: &RunwayDirection, wind: Wind) -> Option<i32> {
        let track = runway.degrees as u32;
        let factor =
            if let Some((Track(OptionalData::Data(start)), Track(OptionalData::Data(end)))) =
                wind.varying
            {
                let heads = [track % 360];
                let (start, end) = (start % 360, end % 360);
                let includes = |a| {
                    if start <= end {
                        a >= start && a <= end
                    } else {
                        a >= start || a <= end
                    }
                };

                if heads.iter().any(|&a| includes(a)) {
                    1.0
                } else {
                    let s = f64::from(diff_angle(track, start));
                    let e = f64::from(diff_angle(track, end));
                    s.to_radians().cos().max(e.to_radians().cos())
                }
            } else {
                match &wind.dir {
                    metar_decoder::wind::WindDirection::Heading(wind_dir) => wind_dir
                        .0
                        .to_option()
                        .map(|wind_dir| (diff_angle(track, wind_dir) as f64).to_radians().cos())
                        .unwrap_or(1.0),
                    metar_decoder::wind::WindDirection::Variable => 1.0,
                }
            };

        Self::scale_wind_speed(wind.speed, factor)
    }

    fn calculate_max_tailwind_from_wind(runway: &RunwayDirection, wind: Wind) -> Option<i32> {
        let opposite_runway = RunwayDirection {
            degrees: ((runway.degrees as u32 + 180) % 360) as u16,
            identifier: runway.identifier.clone(),
        };
        Self::calculate_max_headwind_from_wind(&opposite_runway, wind)
            .map(|headwind| headwind.max(0))
    }

    fn signed_crosswind_component_factor(track: u32, wind_dir: u32) -> f64 {
        let angle = ((wind_dir as i32 - track as i32 + 540) % 360) - 180;
        (angle as f64).to_radians().sin()
    }

    fn crosswind_direction_from_signed_factor(signed_factor: f64) -> CrosswindDirection {
        const EPSILON: f64 = 1e-9;
        if signed_factor > EPSILON {
            CrosswindDirection::Right
        } else if signed_factor < -EPSILON {
            CrosswindDirection::Left
        } else {
            CrosswindDirection::Variable
        }
    }

    fn scale_wind_speed(speed: WindVelocity, factor: f64) -> Option<i32> {
        speed
            .get_max_wind_speed()
            .map(|speed| (f64::from(speed) * factor).ceil() as i32)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Airport, CrosswindDirection, RunwayDirection};
    use crate::airports::tests::make_test_airport;

    fn runway_direction<'a>(airport: &'a Airport, identifier: &str) -> &'a RunwayDirection {
        airport
            .runways
            .iter()
            .flat_map(|runway| runway.runways.iter())
            .find(|direction| direction.identifier == identifier)
            .unwrap()
    }

    fn test_for_airport(metar: &str, expected_runway: &str) {
        let ap = make_test_airport(metar);
        let a = ap.set_runway_based_on_metar_wind().unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a.keys().next().unwrap(), expected_runway);
    }

    #[test]
    fn test_for_enhv() {
        test_for_airport("ENHV 081620Z AUTO 08008KT 9999 OVC006/// 08/07 Q1001", "08");
    }

    #[test]
    fn test_runway_wind_component_api() {
        let ap = make_test_airport("ENHV 081620Z AUTO 08008KT 9999 OVC006/// 08/07 Q1001");
        let runway_08 = runway_direction(&ap, "08");
        let runway_26 = runway_direction(&ap, "26");

        let components = ap.runway_wind_components(runway_08).unwrap();
        assert_eq!(components.headwind, 8);
        assert!(components.crosswind <= 1);

        assert_eq!(ap.runway_max_headwind(runway_08), Some(8));
        assert!(
            ap.runway_max_crosswind(runway_08)
                .is_some_and(|(crosswind, _direction)| crosswind <= 1)
        );
        assert!(
            ap.runway_max_tailwind(runway_26)
                .is_some_and(|tailwind| tailwind >= 7)
        );
    }

    #[test]
    fn test_crosswind_direction_is_side_sensitive() {
        let runway_08_right = {
            let airport = make_test_airport("ENHV 081620Z AUTO 17010KT 9999 OVC006/// 08/07 Q1001");
            let runway_08 = runway_direction(&airport, "08");
            airport
                .runway_max_crosswind(runway_08)
                .map(|(_crosswind, direction)| direction)
        };
        let runway_08_left = {
            let airport = make_test_airport("ENHV 081620Z AUTO 35010KT 9999 OVC006/// 08/07 Q1001");
            let runway_08 = runway_direction(&airport, "08");
            airport
                .runway_max_crosswind(runway_08)
                .map(|(_crosswind, direction)| direction)
        };

        assert_eq!(runway_08_right, Some(CrosswindDirection::Right));
        assert_eq!(runway_08_left, Some(CrosswindDirection::Left));
    }
}
