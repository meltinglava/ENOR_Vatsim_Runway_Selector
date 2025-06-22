use indexmap::IndexMap;
use itertools::{Itertools, MinMaxResult::{MinMax, NoElements, OneElement}};
use rust_flightweather::metar::Metar;

use crate::metar::calculate_max_headwind;
use crate::runway::{Runway, RunwayUse};

#[derive(Debug)]
pub struct Airport {
    pub icao: String,
    pub metar: Option<Metar>,
    pub runways: Vec<Runway>,
    pub runways_in_use: IndexMap<String, RunwayUse>,
}

impl Airport {
    pub fn set_runway_based_on_metar_wind(&self) -> Option<IndexMap<String, RunwayUse>> {
        if self.icao == "ENGM" || self.icao == "ENZV" {
            return None;
        }

        let metar = self.metar.as_ref()?;
        let headwinds = self.runways.iter().flat_map(|runway| {
            runway.runways.iter().map(|dir| {
                let headwind = calculate_max_headwind(dir, metar.wind.clone());
                (dir.identifier.clone(), headwind)
            })
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
}
