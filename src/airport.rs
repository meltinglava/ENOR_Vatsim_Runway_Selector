use indexmap::IndexMap;
use itertools::{Itertools, MinMaxResult::{MinMax, NoElements, OneElement}};
use jiff::{tz::TimeZone, Zoned};
use rust_flightweather::{metar::Metar, types::{CloudLayer, Clouds, Data, Visibility}};

use crate::metar::calculate_max_headwind;
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
    pub fn set_runway_based_on_metar_wind(&self) -> Option<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" {
            Some(self.set_runway_for_engm())
        } else if self.icao == "ENZV" {
            return None;
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


    fn set_runway_for_engm(&self) -> IndexMap<String, RunwayUse> {
        let runway_direction: String = match self.internal_set_runway_based_on_metar_wind(0) {
            Some(map) => {
                map.keys().next().unwrap()[..2].to_string()
            },
            None => {
                // TODO: Implement choise rather than default runway config
                "01".to_string()
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
}
