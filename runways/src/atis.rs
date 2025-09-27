use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::runway::RunwayUse;

pub fn find_runway_in_use_from_atis(atis: &str) -> IndexMap<String, RunwayUse> {
    static SINGLE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"RUNWAY IN USE ([0-9]{2}[LRC]*)").unwrap());
    static ARR: Lazy<Regex> = Lazy::new(|| Regex::new(r"APPROACH RWY ([0-9]{2}[LRC]*)").unwrap());
    static DEP: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"DEPARTURE RUNWAY ([0-9]{2}[LRC]*)").unwrap());
    static MULTI: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"RUNWAYS ([0-9]{2}[LRC]*) AND ([0-9]{2}[LRC]*) IN USE").unwrap());

    let mut runways = IndexMap::new();

    if let Some(c) = SINGLE.captures(atis) {
        runways.insert(c[1].to_string(), RunwayUse::Both);
    } else if let Some(c) = ARR.captures(atis) {
        runways.insert(c[1].to_string(), RunwayUse::Arriving);
    } else if let Some(c) = DEP.captures(atis) {
        runways.insert(c[1].to_string(), RunwayUse::Departing);
    } else if let Some(c) = MULTI.captures(atis) {
        runways.insert(c[1].to_string(), RunwayUse::Both);
        runways.insert(c[2].to_string(), RunwayUse::Both);
    }

    runways
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_runway_in_use_from_atis() {
        let text = "RUNWAY IN USE 19L";
        let map = find_runway_in_use_from_atis(text);
        assert_eq!(map.len(), 1);
        assert_eq!(map["19L"], RunwayUse::Both);
    }
}
