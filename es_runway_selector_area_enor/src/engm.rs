//! Oslo Gardermoen (ENGM) runway selection logic.
//!
//! ### Modes
//! | Time (Europe/Oslo) | Weather | Mode |
//! |---|---|---|
//! | 22:30–06:30 | any | Segregated |
//! | 06:30–22:30 | LVP conditions | Segregated |
//! | 06:30–22:30 | normal | Mixed |
//! | 06:30 transition | calm or no wind info | Single |
//!
//! In **Segregated** mode: `XXL` Departing, `XXR` Arriving.
//! In **Mixed** mode: both `XXL` and `XXR` are Both.
//! In **Single** mode: `01L` or `19R` is Both.

use jiff::Zoned;
use runway_selector_protocol::{
    AirportInfo, MetarData, RunwayAssignment, RunwayUse, SelectionTag, Tag, tags,
    wind_calc::headwind_kt,
};
use tracing::warn;

// ─── ENGM-specific tag registry ───────────────────────────────────────────────

const MIXED_MODE: Tag = Tag {
    id: "mixed_mode",
    symbol: "⇄",
    label: "Mixed mode (both runways dep+arr)",
};
const SEGREGATED_MODE: Tag = Tag {
    id: "segregated_mode",
    symbol: "⇅",
    label: "Segregated mode (L dep / R arr)",
};
const SINGLE_MODE: Tag = Tag {
    id: "single_mode",
    symbol: "↑",
    label: "Single runway mode",
};

pub fn select_runways(
    airport: &AirportInfo,
    metar: Option<&MetarData>,
) -> (Vec<RunwayAssignment>, Vec<SelectionTag>) {
    let runway_direction = choose_active_runway_direction(airport, metar);
    let (mode, lvp) = determine_mode(metar);
    let selection_tags = build_tags(airport, metar, &runway_direction, &mode, lvp);
    let assignments = build_assignments(&runway_direction, mode);
    (assignments, selection_tags)
}

fn build_tags(
    airport: &AirportInfo,
    metar: Option<&MetarData>,
    runway_direction: &str,
    mode: &EngmMode,
    lvp: bool,
) -> Vec<SelectionTag> {
    let mut out = Vec::new();

    // Mode tag (always present — explains what config was chosen).
    out.push(match mode {
        EngmMode::Mixed => MIXED_MODE.reason(),
        EngmMode::Segregated => SEGREGATED_MODE.reason(),
        EngmMode::Single => SINGLE_MODE.reason(),
    });

    if lvp {
        out.push(tags::LVP.reason());
    }

    // Tailwind check: if the chosen direction has a meaningful tailwind, flag it.
    if let Some(wind) = metar.and_then(|m| m.wind.as_ref()) {
        let has_tailwind = airport
            .runways
            .iter()
            .flat_map(|rw| rw.iter())
            .filter(|r| r.identifier.starts_with(runway_direction))
            .any(|r| headwind_kt(r, wind) < -1.0);
        if has_tailwind {
            out.push(tags::TAILWIND.conflict());
        }
    }

    out
}

// ─── Runway direction (01 vs 19) ─────────────────────────────────────────────

fn choose_active_runway_direction(airport: &AirportInfo, metar: Option<&MetarData>) -> String {
    // Find runway pair with heading ~010 (01L/01R side).
    // Pick the direction with the most headwind; fall back to "01".
    let Some(wind) = metar.and_then(|m| m.wind.as_ref()) else {
        return "01".to_string();
    };

    // Find the first runway that has a 01/19 direction.
    let Some(runway) = airport.runways.iter().find(|rw| {
        rw.iter()
            .any(|r| r.identifier.starts_with("01") || r.identifier.starts_with("19"))
    }) else {
        return "01".to_string();
    };

    // With only one direction, use it directly.
    let Some(rwy_b) = runway.reciprocal.as_ref() else {
        return runway.primary.identifier[..2].to_string();
    };

    // Pick whichever direction has more headwind.
    let rwy_a = &runway.primary;
    let hw_a = headwind_kt(rwy_a, wind);
    let hw_b = headwind_kt(rwy_b, wind);

    if hw_b > hw_a + 2.0 {
        rwy_b.identifier[..2].to_string()
    } else {
        rwy_a.identifier[..2].to_string()
    }
}

// ─── Mode determination ───────────────────────────────────────────────────────

enum EngmMode {
    Mixed,
    Segregated,
    Single,
}

/// Returns `(mode, lvp_active)`. `lvp_active` is `true` only when LVP weather
/// is the deciding factor (i.e. during day hours).
fn determine_mode(metar: Option<&MetarData>) -> (EngmMode, bool) {
    let now = match Zoned::now().in_tz("Europe/Oslo") {
        Ok(z) => z,
        Err(e) => {
            warn!(error = %e, "Failed to get Oslo timezone; defaulting to Segregated");
            return (EngmMode::Segregated, false);
        }
    };

    let after_2230 = now
        .date()
        .at(22, 30, 0, 0)
        .in_tz("Europe/Oslo")
        .map(|t| now >= t)
        .unwrap_or(true);
    let before_0630 = now
        .date()
        .at(6, 30, 0, 0)
        .in_tz("Europe/Oslo")
        .map(|t| now < t)
        .unwrap_or(false);

    if after_2230 {
        return (EngmMode::Segregated, false);
    }
    if before_0630 {
        return (EngmMode::Single, false);
    }

    // Day: check weather for LVP.
    let lvp = is_lvp_weather(metar);
    if lvp {
        (EngmMode::Segregated, true)
    } else {
        (EngmMode::Mixed, false)
    }
}

fn is_lvp_weather(metar: Option<&MetarData>) -> bool {
    let Some(m) = metar else { return false };

    // RVR reported → LVP.
    if m.rvr_reported {
        return true;
    }
    // Vertical visibility → LVP.
    if m.vertical_visibility_ft.is_some() {
        return true;
    }
    // Visibility < 5000 m → LVP.
    if m.visibility_m.is_some_and(|v| v < 5000) {
        return true;
    }
    // Ceiling (BKN/OVC) < 1500 ft → LVP.
    let ceiling_low = m.clouds.iter().any(|c| {
        use runway_selector_protocol::CloudCoverage::*;
        matches!(c.coverage, Broken | Overcast | Unknown)
            && c.height_ft.map(|h| h < 1500).unwrap_or(true)
    });
    if ceiling_low {
        return true;
    }
    // Freezing precipitation → forced LVP.
    if m.present_weather.iter().any(|pw| pw.contains("FZ")) {
        return true;
    }
    // Precipitation with temperature < 5°C → possible de-ice, treat as LVP.
    let precip = m.present_weather.iter().any(|pw| {
        ["DZ", "RA", "SN", "SG", "PL", "GR", "GS", "UP", "BR", "FG"]
            .iter()
            .any(|p| pw.contains(p))
    });
    if precip && m.temp_c.is_some_and(|t| t < 5) {
        return true;
    }

    false
}

// ─── Assignment building ──────────────────────────────────────────────────────

fn build_assignments(runway_dir: &str, mode: EngmMode) -> Vec<RunwayAssignment> {
    let mut assignments = Vec::new();
    match mode {
        EngmMode::Mixed => {
            assignments.push(RunwayAssignment {
                runway_id: format!("{runway_dir}L"),
                runway_use: RunwayUse::Both,
            });
            assignments.push(RunwayAssignment {
                runway_id: format!("{runway_dir}R"),
                runway_use: RunwayUse::Both,
            });
        }
        EngmMode::Segregated => {
            assignments.push(RunwayAssignment {
                runway_id: format!("{runway_dir}L"),
                runway_use: RunwayUse::Departing,
            });
            assignments.push(RunwayAssignment {
                runway_id: format!("{runway_dir}R"),
                runway_use: RunwayUse::Arriving,
            });
        }
        EngmMode::Single => {
            let single = match runway_dir {
                "01" => "01L",
                "19" => "19R",
                other => {
                    warn!(
                        runway_dir = other,
                        "Unexpected ENGM runway direction in Single mode"
                    );
                    "01L"
                }
            };
            assignments.push(RunwayAssignment {
                runway_id: single.to_string(),
                runway_use: RunwayUse::Both,
            });
        }
    }
    assignments
}

#[cfg(test)]
mod tests {
    use super::*;
    use runway_selector_protocol::{
        AirportInfo, MetarData, PhysicalRunway, RunwayInfo, WindData, WindDirection,
    };

    fn engm_airport() -> AirportInfo {
        AirportInfo {
            icao: "ENGM".to_string(),
            runways: vec![
                PhysicalRunway::pair(
                    RunwayInfo {
                        identifier: "01L".to_string(),
                        degrees: 10,
                    },
                    RunwayInfo {
                        identifier: "19R".to_string(),
                        degrees: 190,
                    },
                ),
                PhysicalRunway::pair(
                    RunwayInfo {
                        identifier: "01R".to_string(),
                        degrees: 10,
                    },
                    RunwayInfo {
                        identifier: "19L".to_string(),
                        degrees: 190,
                    },
                ),
            ],
        }
    }

    fn wind_metar(degrees: u16, speed_kt: f64) -> MetarData {
        MetarData {
            raw: "ENGM TEST".to_string(),
            icao: "ENGM".to_string(),
            wind: Some(WindData {
                direction: WindDirection::Heading { degrees },
                speed_kt,
                gust_kt: None,
                variable_sector: None,
            }),
            temp_c: None,
            dew_point_c: None,
            qnh_hpa: None,
            visibility_m: None,
            clouds: vec![],
            rvr_reported: false,
            vertical_visibility_ft: None,
            present_weather: vec![],
        }
    }

    #[test]
    fn wind_from_south_selects_19() {
        let airport = engm_airport();
        let metar = wind_metar(180, 10.0);
        assert_eq!(choose_active_runway_direction(&airport, Some(&metar)), "19");
    }

    #[test]
    fn wind_from_north_selects_01() {
        let airport = engm_airport();
        let metar = wind_metar(360, 10.0);
        assert_eq!(choose_active_runway_direction(&airport, Some(&metar)), "01");
    }

    #[test]
    fn no_wind_defaults_to_01() {
        let airport = engm_airport();
        assert_eq!(choose_active_runway_direction(&airport, None), "01");
    }

    #[test]
    fn lvp_snow_cold() {
        let metar = MetarData {
            raw: "ENGM TEST".to_string(),
            icao: "ENGM".to_string(),
            wind: None,
            temp_c: Some(-4),
            dew_point_c: Some(-5),
            qnh_hpa: Some(1010),
            visibility_m: Some(9999),
            clouds: vec![],
            rvr_reported: false,
            vertical_visibility_ft: None,
            present_weather: vec!["-SN".to_string()],
        };
        assert!(is_lvp_weather(Some(&metar)));
    }

    #[test]
    fn no_lvp_clear() {
        let metar = MetarData {
            raw: "ENGM TEST".to_string(),
            icao: "ENGM".to_string(),
            wind: None,
            temp_c: Some(10),
            dew_point_c: Some(5),
            qnh_hpa: Some(1013),
            visibility_m: Some(9999),
            clouds: vec![],
            rvr_reported: false,
            vertical_visibility_ft: None,
            present_weather: vec![],
        };
        assert!(!is_lvp_weather(Some(&metar)));
    }
}
