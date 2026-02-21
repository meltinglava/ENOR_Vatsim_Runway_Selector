use indexmap::{IndexMap, map::Entry};
use regex::Regex;
use std::sync::LazyLock;

use crate::runway::RunwayUse;

pub fn find_runway_in_use_from_atis(atis: &str) -> IndexMap<String, RunwayUse> {
    static SINGLE_POST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bRUNWAY ([0-9]{2}[LRC]*) IN USE\b").unwrap());

    static SINGLE_PRE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bRUNWAY IN USE ([0-9]{2}[LRC]*)\b").unwrap());

    static ARR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bAPPROACH (?:RWY|RUNWAY) ([0-9]{2}[LRC]*)\b").unwrap());

    static DEP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\bDEPARTURE RUNWAY ([0-9]{2}[LRC]*)\b").unwrap());

    static MULTI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bRUNWAYS ([0-9]{2}[LRC]*) AND ([0-9]{2}[LRC]*) IN USE\b").unwrap()
    });

    let is_arrival = atis.contains(" ARRIVAL INFORMATION ");
    let is_departure = atis.contains(" DEPARTURE INFORMATION ");

    let mut runways = IndexMap::new();

    // 1) Parse the most specific / unambiguous forms first
    for c in MULTI.captures_iter(atis) {
        upsert(&mut runways, &c[1], RunwayUse::Both);
        upsert(&mut runways, &c[2], RunwayUse::Both);
    }

    for c in SINGLE_PRE.captures_iter(atis) {
        upsert(&mut runways, &c[1], RunwayUse::Both);
    }

    for c in ARR.captures_iter(atis) {
        upsert(&mut runways, &c[1], RunwayUse::Arriving);
    }

    for c in DEP.captures_iter(atis) {
        upsert(&mut runways, &c[1], RunwayUse::Departing);
    }

    // 2) Generic "RUNWAY XX IN USE" as fallback, BUT:
    //    - interpret by bulletin type if split ATIS
    //    - ignore occurrences that are actually part of "DEPARTURE RUNWAY ..." or "APPROACH ... RUNWAY ..."
    //    - if combined/unknown, do not override already-known specific info
    for caps in SINGLE_POST.captures_iter(atis) {
        let whole = caps.get(0).unwrap();
        let rwy = &caps[1];

        // Look at a short window before the match to see if this "RUNWAY" is preceded by
        // "DEPARTURE " or "APPROACH " (meaning it’s not a standalone "RUNWAY XX IN USE" statement).
        let start = whole.start();
        let window_start = start.saturating_sub(20);
        let prefix = &atis[window_start..start];

        if prefix.contains("DEPARTURE ") || prefix.contains("APPROACH ") {
            continue;
        }

        if is_arrival {
            upsert(&mut runways, rwy, RunwayUse::Arriving);
        } else if is_departure {
            upsert(&mut runways, rwy, RunwayUse::Departing);
        } else {
            // Combined/unknown: only use as fallback; never override specific info
            if !runways.contains_key(rwy) {
                runways.insert(rwy.to_string(), RunwayUse::Both);
            }
        }
    }

    runways
}

fn upsert(map: &mut IndexMap<String, RunwayUse>, rwy: &str, new_use: RunwayUse) {
    match map.entry(rwy.to_string()) {
        Entry::Vacant(e) => {
            e.insert(new_use);
        }
        Entry::Occupied(mut e) => {
            e.insert((*e.get()).merged_with(new_use));
        }
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use super::*;

    #[test]
    fn test_find_runway_in_use_from_atis() {
        let text = "RUNWAY IN USE 19L";
        let map = find_runway_in_use_from_atis(text);
        assert_eq!(map.len(), 1);
        assert_eq!(map["19L"], RunwayUse::Both);
    }

    #[test]
    fn test_engm_single_atis() {
        let atis = "OSLO GARDERMOEN INFORMATION LIMA .. TIME 1550 .. EXPECT ILS OR RNP APPROACH RUNWAY 01R .. DEPARTURE RUNWAY 01L IN USE .. RCR RWY 01L AT TIME 1427 .. RWYCC 3/3/3 .. 100 PERCENT 06 MM DRY SNOW .. RCR RWY 01R AT TIME 1429 .. RWYCC 3/3/3 .. 100 PERCENT 04 MM DRY SNOW .. TRANSITION LEVEL 85 .. FOR CLEARANCE AND START UP, CONTACT POLARIS CONTROL 121.550 .. MET REPORT .. WIND 020 DEGREES 3 KNOTS .. VISIBILITY 6 KM .. CLOUDS SCT 700 FT BKN 1500 FT .. LIGHT SNOW .. TMP -4 DP -5 .. QNH 1010 .. ACKNOWLEDGE INFORMATION LIMA ON FIRST CONTACT.";
        let a = find_runway_in_use_from_atis(atis);
        let expected = [
            ("01L".to_owned(), RunwayUse::Departing),
            ("01R".to_owned(), RunwayUse::Arriving),
        ]
        .into_iter()
        .collect_vec();
        let actual = a.into_iter().sorted_by_key(|n| n.0.clone()).collect_vec();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_engm_split() {
        let arr = "OSLO GARDERMOEN ARRIVAL INFORMATION CHARLIE .. TIME 1750 .. EXPECT ILS OR RNP APPROACH RUNWAY 01R .. RCR RWY 01R AT TIME 1700 .. RWYCC 4/4/4 .. 100 PERCENT 03 MM DRY SNOW .. CAUTION SLIPPERY TAXIWAYS AND RWY EXITS .. TRANSITION LEVEL 85 .. AIRMET INDIA 04 VALID .. MET REPORT .. WIND 050 DEGREES 4 KNOTS .. VISIBILITY 3300 METERS .. CLOUDS FEW 700 FT SCT 2000 FT OVC 2700 FT .. LIGHT SNOW .. TMP -3 DP -4 .. QNH 1010 .. ACKNOWLEDGE INFORMATION CHARLIE ON FIRST CONTACT.";
        let dep = "OSLO GARDERMOEN DEPARTURE INFORMATION HOTEL .. TIME 1750 .. RUNWAY 01L IN USE .. RCR RWY 01L AT TIME 1621 .. RWYCC 4/4/4 .. 100 PERCENT 03 MM DRY SNOW .. CAUTION SLIPPERY TAXIWAYS AND RWY EXITS .. ADVISE IF DE-ICE IS REQUIRED ON FIRST CONTACT WITH ATC .. FOR EN-ROUTE CLEARANCE REQUEST VIA DATALINK OR CONTACT TOWER 118.305 .. AIRMET INDIA 04 VALID .. MET REPORT .. WIND 050 DEGREES 4 KNOTS .. VISIBILITY 3300 METERS .. CLOUDS FEW 700 FT SCT 2000 FT OVC 2700 FT .. LIGHT SNOW .. TMP -3 DP -4 .. QNH 1010 .. ACKNOWLEDGE INFORMATION HOTEL ON FIRST CONTACT.";
        let a = find_runway_in_use_from_atis(arr);
        let d = find_runway_in_use_from_atis(dep);
        let ea = Some(("01R".to_owned(), RunwayUse::Arriving));
        let ed = Some(("01L".to_owned(), RunwayUse::Departing));
        assert_eq!(ea, a.into_iter().next());
        assert_eq!(ed, d.into_iter().next());
    }
}
