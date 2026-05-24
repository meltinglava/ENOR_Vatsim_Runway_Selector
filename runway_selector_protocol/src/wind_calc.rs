use crate::{RunwayInfo, VariableWind, WindData, WindDirection};

/// Signed headwind component in knots.
/// Positive = headwind, negative = tailwind.
/// Returns `0.0` for purely variable wind direction.
pub fn headwind_kt(runway: &RunwayInfo, wind: &WindData) -> f64 {
    let speed = effective_speed(wind);
    let factor = match &wind.direction {
        WindDirection::Variable => 0.0,
        WindDirection::Heading { degrees } => match &wind.variable_sector {
            None => cos_diff(runway.degrees, *degrees),
            Some(vs) => max_headwind_factor(runway.degrees, vs),
        },
    };
    speed * factor
}

/// Crosswind component magnitude in knots (always ≥ 0).
/// Returns `0.0` for purely variable wind direction.
pub fn crosswind_kt(runway: &RunwayInfo, wind: &WindData) -> f64 {
    let speed = effective_speed(wind);
    let factor = match &wind.direction {
        WindDirection::Variable => 0.0,
        WindDirection::Heading { degrees } => match &wind.variable_sector {
            None => sin_diff(runway.degrees, *degrees).abs(),
            Some(vs) => max_crosswind_factor(runway.degrees, vs),
        },
    };
    speed * factor
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn effective_speed(wind: &WindData) -> f64 {
    wind.gust_kt.unwrap_or(wind.speed_kt)
}

/// Cosine of the signed angle from runway heading to wind direction.
fn cos_diff(runway_hdg: u16, wind_dir: u16) -> f64 {
    (signed_angle_diff(runway_hdg, wind_dir) as f64)
        .to_radians()
        .cos()
}

/// Sine of the signed angle from runway heading to wind direction.
fn sin_diff(runway_hdg: u16, wind_dir: u16) -> f64 {
    (signed_angle_diff(runway_hdg, wind_dir) as f64)
        .to_radians()
        .sin()
}

/// Signed difference in degrees from runway heading to wind direction (-180..=180).
fn signed_angle_diff(runway_hdg: u16, wind_dir: u16) -> i32 {
    let diff = (wind_dir as i32 - runway_hdg as i32).rem_euclid(360);
    if diff > 180 { diff - 360 } else { diff }
}

/// Maximum headwind factor across a variable wind sector.
fn max_headwind_factor(runway_hdg: u16, vs: &VariableWind) -> f64 {
    let f_from = cos_diff(runway_hdg, vs.from_degrees);
    let f_to = cos_diff(runway_hdg, vs.to_degrees);
    // If the runway heading itself is within the arc, full headwind (1.0) is possible.
    if sector_contains(vs.from_degrees, vs.to_degrees, runway_hdg) {
        f_from.max(f_to).max(1.0)
    } else {
        f_from.max(f_to)
    }
}

/// Maximum crosswind factor across a variable wind sector.
fn max_crosswind_factor(runway_hdg: u16, vs: &VariableWind) -> f64 {
    let f_from = sin_diff(runway_hdg, vs.from_degrees).abs();
    let f_to = sin_diff(runway_hdg, vs.to_degrees).abs();
    // If a 90° perpendicular falls inside the arc, full crosswind (1.0) is possible.
    let perp_r = (runway_hdg as u32 + 90) as u16 % 360;
    let perp_l = (runway_hdg as u32 + 270) as u16 % 360;
    if sector_contains(vs.from_degrees, vs.to_degrees, perp_r)
        || sector_contains(vs.from_degrees, vs.to_degrees, perp_l)
    {
        f_from.max(f_to).max(1.0)
    } else {
        f_from.max(f_to)
    }
}

/// Returns `true` if `angle` lies within the arc from `from` to `to` (inclusive, clockwise).
fn sector_contains(from: u16, to: u16, angle: u16) -> bool {
    if from <= to {
        angle >= from && angle <= to
    } else {
        angle >= from || angle <= to
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunwayInfo, VariableWind, WindData, WindDirection};

    fn rwy(hdg: u16) -> RunwayInfo {
        RunwayInfo {
            identifier: "RWY".to_string(),
            degrees: hdg,
        }
    }

    fn steady_wind(dir: u16, speed_kt: f64) -> WindData {
        WindData {
            direction: WindDirection::Heading { degrees: dir },
            speed_kt,
            gust_kt: None,
            variable_sector: None,
        }
    }

    #[test]
    fn direct_headwind() {
        let hw = headwind_kt(&rwy(360), &steady_wind(360, 10.0));
        assert!((hw - 10.0).abs() < 0.01, "expected ≈10.0, got {hw}");
    }

    #[test]
    fn direct_crosswind() {
        let cw = crosswind_kt(&rwy(360), &steady_wind(90, 10.0));
        assert!((cw - 10.0).abs() < 0.01, "expected ≈10.0, got {cw}");
    }

    #[test]
    fn tailwind_is_negative_headwind() {
        let hw = headwind_kt(&rwy(360), &steady_wind(180, 10.0));
        assert!((hw - (-10.0)).abs() < 0.01, "expected ≈−10.0, got {hw}");
    }

    #[test]
    fn gust_used_for_speed() {
        let wind = WindData {
            direction: WindDirection::Heading { degrees: 360 },
            speed_kt: 10.0,
            gust_kt: Some(20.0),
            variable_sector: None,
        };
        let hw = headwind_kt(&rwy(360), &wind);
        assert!((hw - 20.0).abs() < 0.01);
    }

    #[test]
    fn variable_wind_returns_zero() {
        let wind = WindData {
            direction: WindDirection::Variable,
            speed_kt: 5.0,
            gust_kt: None,
            variable_sector: None,
        };
        assert_eq!(headwind_kt(&rwy(360), &wind), 0.0);
        assert_eq!(crosswind_kt(&rwy(360), &wind), 0.0);
    }

    #[test]
    fn variable_sector_headwind_takes_max() {
        // Runway 360; variable sector 330V030 straddles the runway heading.
        let wind = WindData {
            direction: WindDirection::Heading { degrees: 360 },
            speed_kt: 10.0,
            gust_kt: None,
            variable_sector: Some(VariableWind {
                from_degrees: 330,
                to_degrees: 30,
            }),
        };
        let hw = headwind_kt(&rwy(360), &wind);
        assert!(
            (hw - 10.0).abs() < 0.01,
            "expected full headwind ≈10.0, got {hw}"
        );
    }
}
