use indexmap::IndexMap;
use itertools::{Itertools, MinMaxResult::{MinMax, NoElements, OneElement}};
use jiff::Zoned;
use rust_flightweather::{metar::Metar, types::{CloudLayer, Clouds, Data, Visibility}};

use crate::{config::ESConfig, metar::{calculate_max_crosswind, calculate_max_headwind}};
use crate::runway::{Runway, RunwayUse};

#[derive(Debug)]
pub struct Airport {
    pub icao: String,
    pub metar: Option<Metar>,
    pub runways: Vec<Runway>,
    pub runways_in_use: IndexMap<String, RunwayUse>,
}

#[derive(Debug)]
enum EngmModes {
    Mixed,
    Segregated,
    Single,
}

impl Airport {
    pub fn set_runway_based_on_metar_wind(&self, config: &ESConfig) -> Option<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" {
            Some(self.set_runway_for_engm(config))
        } else if self.icao == "ENZV" {
            Some(self.set_runway_for_enzv())
        } else if self.runways.len() == 1 {
            self.internal_set_runway_based_on_metar_wind(0)
        } else {
            unreachable!("Airport {} has multiple runways, but no specific logic implemented for it", self.icao);
        }
    }

    fn internal_set_runway_based_on_metar_wind(&self, runway_index: usize) -> Option<IndexMap<String, RunwayUse>> {
        let metar = self.metar.as_ref()?;
        let headwinds = self.runways[runway_index].runways.iter().map(|dir| {
            let headwind = calculate_max_headwind(dir, metar.wind.clone());
            (dir.identifier.clone(), headwind)
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


    fn set_runway_for_engm(&self, config: &ESConfig) -> IndexMap<String, RunwayUse> {
        let runway_direction: String = match self.internal_set_runway_based_on_metar_wind(0) {
            Some(map) => {
                map.keys().next().unwrap()[..2].to_string()
            },
            None => {
                config.get_default_runways().get(&self.icao)
                    .map(|&rwy| format!("{:02}", rwy))
                    .unwrap_or_else(|| "01".to_string()) // Default to 01 if no default is set
            }
        };

        let ceiling_for_lvp = if let Some(metar) = self.metar.as_ref() {
            match metar.clouds {
                Data::Known(Clouds::CloudLayers) => {
                    metar.cloud_layers.iter().filter(|layer| {
                        match layer {
                            CloudLayer::Broken(_, ceil) => ceil.map(|c| c < 500).unwrap_or(false),
                            CloudLayer::Overcast(_, ceil) => ceil.map(|c| c < 500).unwrap_or(false),
                            _ => false
                        }
                    }).next().is_some()
                }
                _ => false,
            }
        } else {
            false
        };

        let rvr_reported = if let Some(metar) = self.metar.as_ref() {
            !metar.rvr.is_empty() // RVR is a vec. We dont carre about the values, just if it exists
        } else {
            false
        };

        let visibility_below_5000 = if let Some(metar) = self.metar.as_ref() {
            match &metar.visibility {
                Data::Known(v) => match v {
                    Visibility::Metres(m) => *m < 5000,
                    Visibility::StatuteMiles(_) => unreachable!("No norwegian airports should report vis in statue miles"), // We don't handle statute miles here
                    Visibility::Cavok => false,
                },
                Data::Unknown => false,
            }
        } else {
            false
        };

        let reported_vv = self.metar.as_ref().and_then(|m| m.vert_visibility.clone()).is_some();

        let now = Zoned::now().in_tz("Europe/Oslo").expect("Failed to get timezone Europe/Oslo");
        let mode = if now.date().at(22, 30, 0, 0).in_tz("Europe/Oslo").unwrap() <= now {
            EngmModes::Segregated
        } else if now.date().at(6, 30, 0, 0).in_tz("Europe/Oslo").unwrap() > now {
            EngmModes::Single // could be mixed if weather is bad, but currently out of scope
        } else {
            if ceiling_for_lvp || rvr_reported || visibility_below_5000 || reported_vv {
                EngmModes::Segregated
            } else {
                EngmModes::Mixed
            }
        };

        let mut map = IndexMap::new();
        match mode {
            EngmModes::Mixed => {
                map.insert(format!("{}L", runway_direction), RunwayUse::Both);
                map.insert(format!("{}R", runway_direction), RunwayUse::Both);
            },
            EngmModes::Segregated => {
                map.insert(format!("{}L", runway_direction), RunwayUse::Departing);
                map.insert(format!("{}R", runway_direction), RunwayUse::Arriving);
            },
            EngmModes::Single => {
                let runway = match runway_direction.as_str() {
                    "01" => "01L",
                    "19" => "19R",
                    _ => unreachable!("Runway direction {} is not valid for ENGM", runway_direction),
                }.to_string();
                map.insert(runway, RunwayUse::Both);
            },
        }
        map
    }

    fn set_runway_for_enzv(&self) -> IndexMap<String, RunwayUse> {
        let main_runway_index = self.runways.iter().enumerate().filter(|(_, runway)| {
            runway.runways.iter().any(|dir| {
                dir.identifier == "18"
            })
        }).map(|(i, _)| i).next().unwrap();
        let main_runway = match self.internal_set_runway_based_on_metar_wind(main_runway_index) {
            Some(rwy) => rwy.keys().next().unwrap().to_string(),
            None => "18".to_string(),
        };

        let default_fallback = IndexMap::from([
            (main_runway.clone(), RunwayUse::Both),
        ]);

        let main_runway_direction = self.runways[main_runway_index].runways.iter().find(|dir| dir.identifier == main_runway).unwrap();

        if let Some(metar) = self.metar.as_ref() {
            let crosswind_main_runway = calculate_max_crosswind(&main_runway_direction, &metar.wind);
            if crosswind_main_runway.is_none() {
                return default_fallback;
            }
            if let Some(crosswind) = crosswind_main_runway {
                if crosswind.as_knots() < 15.0 {
                    // If crosswind is below 15 knots, we can use the main runway
                    return default_fallback;
                }
                let secondary_runway_index = !main_runway_index;
                let secondary_runway_crosswind = calculate_max_crosswind(&self.runways[secondary_runway_index].runways[0], &metar.wind).unwrap();
                let secondary_runway = match self.internal_set_runway_based_on_metar_wind(secondary_runway_index) {
                    Some(rwy) => rwy.keys().next().unwrap().to_string(),
                    None => return default_fallback,
                };
                if secondary_runway_crosswind < crosswind {
                    // If the secondary runway has a lower crosswind, we use it
                    let mut map = IndexMap::new();
                    map.insert(secondary_runway, RunwayUse::Both);
                    return map;
                } else {
                    return default_fallback;
                }
            }
        }
        default_fallback
    }
}
