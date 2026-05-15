use jiff::Zoned;
use runway_plugin_api::{helpers::best_headwind, *};

/// Build a minimal `AirportSelectionRequest` for ENGM from a raw METAR string.
///
/// Headwind values are pre-computed so the plugin can pick the runway direction:
/// `hw_01l` is the headwind for runway 01L, `hw_19r` for 19R.
#[cfg(test)]
pub(crate) fn test_request(
    metar_raw: &str,
    parsed: Option<ParsedMetar>,
    hw_01l: Option<i32>,
    hw_19r: Option<i32>,
) -> AirportSelectionRequest {
    AirportSelectionRequest {
        icao: "ENGM".to_string(),
        runways: vec![
            RunwayInfo {
                identifier: "01L".to_string(),
                heading: 6,
                headwind_kt: hw_01l,
                tailwind_kt: None,
                crosswind_kt: None,
                crosswind_direction: None,
            },
            RunwayInfo {
                identifier: "01R".to_string(),
                heading: 6,
                headwind_kt: hw_01l,
                tailwind_kt: None,
                crosswind_kt: None,
                crosswind_direction: None,
            },
            RunwayInfo {
                identifier: "19L".to_string(),
                heading: 186,
                headwind_kt: hw_19r,
                tailwind_kt: None,
                crosswind_kt: None,
                crosswind_direction: None,
            },
            RunwayInfo {
                identifier: "19R".to_string(),
                heading: 186,
                headwind_kt: hw_19r,
                tailwind_kt: None,
                crosswind_kt: None,
                crosswind_direction: None,
            },
        ],
        metar: parsed.map(|p| MetarData {
            raw: metar_raw.to_string(),
            parsed: Some(p),
        }),
        timestamp_utc: "2026-01-01T14:00:00Z".to_string(),
    }
}

pub fn select(airport: &AirportSelectionRequest) -> AirportSelectionResult {
    let runway_direction = determine_runway_direction(airport);

    let (
        ceiling_for_lvp,
        rvr_reported,
        visibility_below_5000,
        reported_vv,
        possible_deice_conditions,
        forced_deice_condition,
    ) = analyze_weather(airport);

    let now = Zoned::now()
        .in_tz("Europe/Oslo")
        .expect("Failed to get timezone Europe/Oslo");

    let mode = if now
        .date()
        .at(22, 30, 0, 0)
        .in_tz("Europe/Oslo")
        .expect("Failed to convert 22:30 to Europe/Oslo")
        <= now
    {
        EngmMode::Segregated
    } else if now
        .date()
        .at(6, 30, 0, 0)
        .in_tz("Europe/Oslo")
        .expect("Failed to convert 06:30 to Europe/Oslo")
        > now
    {
        EngmMode::Single
    } else if ceiling_for_lvp
        || rvr_reported
        || visibility_below_5000
        || reported_vv
        || possible_deice_conditions
        || forced_deice_condition
    {
        EngmMode::Segregated
    } else {
        EngmMode::Mixed
    };

    let runway_uses = match mode {
        EngmMode::Mixed => vec![
            RunwayUseEntry {
                runway: format!("{runway_direction}L"),
                use_: RunwayUse::Both,
            },
            RunwayUseEntry {
                runway: format!("{runway_direction}R"),
                use_: RunwayUse::Both,
            },
        ],
        EngmMode::Segregated => vec![
            RunwayUseEntry {
                runway: format!("{runway_direction}L"),
                use_: RunwayUse::Departing,
            },
            RunwayUseEntry {
                runway: format!("{runway_direction}R"),
                use_: RunwayUse::Arriving,
            },
        ],
        EngmMode::Single => {
            let runway = match runway_direction.as_str() {
                "01" => "01L",
                "19" => "19R",
                _ => "01L",
            };
            vec![RunwayUseEntry {
                runway: runway.to_string(),
                use_: RunwayUse::Both,
            }]
        }
    };

    AirportSelectionResult {
        icao: airport.icao.clone(),
        handled: true,
        runway_uses,
    }
}

enum EngmMode {
    Mixed,
    Segregated,
    Single,
}

fn determine_runway_direction(airport: &AirportSelectionRequest) -> String {
    // Compare one representative runway from each end of the parallel pair.
    // best_headwind with a 2 kt threshold mirrors the existing generic logic.
    let pair: Vec<RunwayInfo> = airport
        .runways
        .iter()
        .filter(|r| r.identifier == "01L" || r.identifier == "19R")
        .cloned()
        .collect();

    best_headwind(&pair, 2)
        .map(|r| r.identifier[..2].to_string())
        .unwrap_or_else(|| "01".to_string())
}

fn analyze_weather(airport: &AirportSelectionRequest) -> (bool, bool, bool, bool, bool, bool) {
    let Some(metar) = &airport.metar else {
        return (false, false, false, false, false, false);
    };
    let Some(parsed) = &metar.parsed else {
        return (false, false, false, false, false, false);
    };

    if parsed.is_cavok {
        return (false, false, false, false, false, false);
    }

    let ceiling_clouds = [CloudCoverage::Broken, CloudCoverage::Overcast];
    let ceiling_for_lvp = parsed
        .clouds
        .iter()
        .filter(|c| match c.coverage {
            Some(ref cov) => ceiling_clouds.contains(cov),
            None => true, // undefined coverage → assume BKN/OVC
        })
        .any(|c| match c.height_hundreds_ft {
            Some(h) => h < 15,
            None => true, // undefined height → assume below 1500 ft
        });

    let rvr_reported = !parsed.rvr.is_empty();

    let visibility_below_5000 = parsed.visibility_meters.is_some_and(|v| v < 5000);

    let reported_vv = parsed.vertical_visibility_hundreds_ft.is_some();

    let forced_deice_condition = parsed
        .weather_phenomena
        .iter()
        .any(|pw| pw.descriptors.contains(&WeatherDescriptor::Freezing));

    let contender_for_deice = parsed
        .weather_phenomena
        .iter()
        .flat_map(|pw| pw.phenomena.iter())
        .any(|p| {
            matches!(
                p.as_str(),
                "DZ" | "RA" | "SN" | "SG" | "PL" | "GR" | "GS" | "UP" | "BR" | "FG"
            )
        });

    let possible_deice_conditions = match parsed.temperature_c {
        None => contender_for_deice,
        Some(temp) => temp < 5 && contender_for_deice,
    };

    (
        ceiling_for_lvp,
        rvr_reported,
        visibility_below_5000,
        reported_vv,
        possible_deice_conditions,
        forced_deice_condition,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad_weather_parsed() -> ParsedMetar {
        ParsedMetar {
            is_cavok: false,
            wind: None,
            visibility_meters: None,
            rvr: vec![],
            clouds: vec![CloudData {
                coverage: Some(CloudCoverage::Overcast),
                height_hundreds_ft: Some(9),
                cloud_type: None,
            }],
            vertical_visibility_hundreds_ft: None,
            weather_phenomena: vec![WeatherPhenomenonData {
                intensity: None,
                descriptors: vec![],
                phenomena: vec!["SN".to_string()],
            }],
            temperature_c: Some(-9),
            dew_point_c: Some(-12),
            qnh_hpa: Some(1024),
        }
    }

    fn good_weather_parsed() -> ParsedMetar {
        ParsedMetar {
            is_cavok: false,
            wind: None,
            visibility_meters: Some(9999),
            rvr: vec![],
            clouds: vec![CloudData {
                coverage: Some(CloudCoverage::Few),
                height_hundreds_ft: Some(40),
                cloud_type: None,
            }],
            vertical_visibility_hundreds_ft: None,
            weather_phenomena: vec![],
            temperature_c: Some(6),
            dew_point_c: Some(1),
            qnh_hpa: Some(1015),
        }
    }

    /// Segregated: bad weather during daytime should trigger segregated mode.
    /// The 'bad weather' METAR has -SHSN OVC009 M09/M12 which means:
    /// OVC at 900ft (< 1500ft) → ceiling_for_lvp = true → Segregated.
    /// We can't assert the exact mode from a unit test since it depends on
    /// the current time. Instead we verify weather analysis flags directly.
    #[test]
    fn bad_weather_triggers_ceiling_lvp() {
        let req = test_request(
            "ENGM 080920Z VRB03KT 9999 -SHSN OVC009 M09/M12 Q1024 NOSIG",
            Some(bad_weather_parsed()),
            None,
            None,
        );
        let (ceiling, ..) = analyze_weather(&req);
        assert!(ceiling, "OVC009 should trigger ceiling_for_lvp");
    }

    #[test]
    fn good_weather_no_flags() {
        let req = test_request(
            "ENGM 141020Z 35010KT 9999 FEW040 06/01 Q1015 NOSIG",
            Some(good_weather_parsed()),
            None,
            None,
        );
        let (ceiling, rvr, vis, vv, deice, fz) = analyze_weather(&req);
        assert!(!ceiling);
        assert!(!rvr);
        assert!(!vis);
        assert!(!vv);
        assert!(!deice);
        assert!(!fz);
    }

    #[test]
    fn wind_selects_northbound_direction() {
        // 01L headwind 10kt, 19R headwind -10kt → should pick "01"
        let req = test_request("ENGM", None, Some(10), Some(-10));
        assert_eq!(determine_runway_direction(&req), "01");
    }

    #[test]
    fn wind_selects_southbound_direction() {
        // 19R headwind 10kt, 01L headwind -10kt → should pick "19"
        let req = test_request("ENGM", None, Some(-10), Some(10));
        assert_eq!(determine_runway_direction(&req), "19");
    }

    #[test]
    fn calm_wind_defaults_to_northbound() {
        let req = test_request("ENGM", None, Some(0), Some(0));
        assert_eq!(determine_runway_direction(&req), "01");
    }

    #[test]
    fn result_is_always_handled() {
        let req = test_request("ENGM", None, None, None);
        let result = select(&req);
        assert!(result.handled);
        assert_eq!(result.icao, "ENGM");
        assert!(!result.runway_uses.is_empty());
    }
}
