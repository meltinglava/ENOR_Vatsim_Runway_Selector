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
};

use crate::{
    config::ESConfig,
    error::{ApplicationError, ApplicationResult},
    metar::{calculate_max_crosswind, calculate_max_headwind},
    runway::{Runway, RunwayUse},
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

#[derive(Debug)]
enum EngmModes {
    Mixed,
    Segregated,
    Single,
}

impl Airport {
    pub fn set_runway_based_on_metar_wind(
        &self,
        config: &ESConfig,
    ) -> ApplicationResult<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" {
            self.set_runway_for_engm(config)
        } else if self.icao == "ENZV" {
            self.set_runway_for_enzv()
        } else if self.runways.len() == 1 {
            self.internal_set_runway_based_on_metar_wind(0)
                .ok_or(ApplicationError::NoRunwayToSet)
        } else if self.icao == "ENVR" {
            Ok(IndexMap::new())
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
        let metar = self.metar.as_ref()?;
        let headwinds = self.runways[runway_index]
            .runways
            .iter()
            .map(|dir| {
                let headwind = calculate_max_headwind(dir, metar.wind.clone());
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

    fn set_runway_for_engm(
        &self,
        config: &ESConfig,
    ) -> ApplicationResult<IndexMap<String, RunwayUse>> {
        let runway_direction: String = match self.internal_set_runway_based_on_metar_wind(0) {
            Some(map) => map.keys().next().unwrap()[..2].to_string(),
            None => {
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
                        height.height < 500 // Ceiling below 500 feet
                    } else {
                        true // If height is undefined, we assume its below 500 feet
                    }
                });

            rvr_reported = described_obscuration.rvr.iter().any(|rvr| {
                if let OptionalData::Data(value) = rvr.value {
                    value < 1000
                } else {
                    true
                }
            });

            if let VisibilityUnit::Meters(data) = described_obscuration.visibility.value {
                visibility_below_5000 = if let OptionalData::Data(value) = data {
                    value < 5000
                } else {
                    true
                }
            }
            // TODO: Handle statute miles visibility

            reported_vv = false; // TODO: Handle vertical visibility
        }

        let now = Zoned::now()
            .in_tz("Europe/Oslo")
            .expect("Failed to get timezone Europe/Oslo");
        let mode = if now.date().at(22, 30, 0, 0).in_tz("Europe/Oslo")? <= now {
            EngmModes::Segregated
        } else if now.date().at(6, 30, 0, 0).in_tz("Europe/Oslo")? > now {
            EngmModes::Single // could be segregated / mixed if weather is bad, but currently out of scope
        } else if ceiling_for_lvp || rvr_reported || visibility_below_5000 || reported_vv {
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
        Ok(map)
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

        if let Some(metar) = self.metar.as_ref() {
            let crosswind_main_runway = calculate_max_crosswind(main_runway_direction, &metar.wind);
            if crosswind_main_runway.is_none() {
                return default_fallback;
            }
            if let Some(crosswind) = crosswind_main_runway {
                if crosswind < 15 {
                    // If crosswind is below 15 knots, we can use the main runway
                    return default_fallback;
                }
                let secondary_runway_index = main_runway_index & 1;
                let secondary_runway_crosswind = calculate_max_crosswind(
                    &self.runways[secondary_runway_index].runways[0],
                    &metar.wind,
                )
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
                    return Ok(map);
                } else {
                    return default_fallback;
                }
            }
        }
        default_fallback
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::airports::tests::make_test_airport;

    use super::*;

    fn test_for_airport(metar: &str, expected_runway: &str) {
        let ap = make_test_airport(metar);
        let a = ap
            .set_runway_based_on_metar_wind(&ESConfig::new_for_test())
            .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a.keys().next().unwrap(), expected_runway);
    }

    #[test]
    fn test_for_enhv() {
        test_for_airport("ENHV 081620Z AUTO 08008KT 9999 OVC006/// 08/07 Q1001", "08");
    }
}
