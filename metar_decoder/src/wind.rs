use crate::units::{
    track::{Track, nom_track},
    velocity::{WindVelocity, nom_velocity},
};

use nom::{
    Parser,
    branch::alt,
    bytes::complete::tag,
    character::complete::char,
    combinator::{map, opt, value},
    sequence::{preceded, separated_pair},
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Wind {
    pub dir: WindDirection,
    pub speed: WindVelocity,
    pub varying: Option<(Track, Track)>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WindDirection {
    Heading(Track),
    Variable,
}

pub(crate) fn nom_variable_wind(input: &str) -> nom::IResult<&str, WindDirection> {
    value(WindDirection::Variable, tag("VRB")).parse(input)
}

pub(crate) fn nom_heading(input: &str) -> nom::IResult<&str, WindDirection> {
    map(nom_track, WindDirection::Heading).parse(input)
}

pub(crate) fn nom_wind_direction(input: &str) -> nom::IResult<&str, WindDirection> {
    alt((nom_variable_wind, nom_heading)).parse(input)
}

pub(crate) fn nom_wind(input: &str) -> nom::IResult<&str, Wind> {
    (
        nom_wind_direction,
        nom_velocity,
        opt(preceded(
            char(' '),
            separated_pair(nom_track, char('V'), nom_track),
        )),
    )
        .map(|(dir, speed, varying)| Wind {
            dir,
            speed,
            varying,
        })
        .parse(input)
}

#[cfg(test)]
mod tests {
    use crate::units::velocity::VelocityUnit;

    use super::*;
    use crate::optional_data::OptionalData::Data;

    fn setup_test(s: &str, w: Wind) {
        let result = nom_wind(s);
        assert_eq!(Ok(("", w)), result);
    }

    #[test]
    fn test_parse() {
        let t = "21007G17KT 160V270";
        let expected = Wind {
            dir: WindDirection::Heading(Track(Data(210))),
            speed: WindVelocity {
                velocity: Data(7),
                gust: Some(Data(17)),
                unit: VelocityUnit::Knots,
            },
            varying: Some((Track(Data(160)), Track(Data(270)))),
        };
        setup_test(t, expected);
    }

    #[test]
    fn test_parse_variable_wind() {
        let t = "VRB03MPS";
        let expected = Wind {
            dir: WindDirection::Variable,
            speed: WindVelocity {
                velocity: Data(3),
                gust: None,
                unit: VelocityUnit::MetersPerSecond,
            },
            varying: None,
        };
        setup_test(t, expected);
    }

    #[test]
    fn test_parse_variable_wind_with_varying() {
        let t = "VRB03MPS 160V270";
        let expected = Wind {
            dir: WindDirection::Variable,
            speed: WindVelocity {
                velocity: Data(3),
                gust: None,
                unit: VelocityUnit::MetersPerSecond,
            },
            varying: Some((Track(Data(160)), Track(Data(270)))),
        };
        setup_test(t, expected);
    }

    #[test]
    fn test_parse_heading_wind() {
        let t = "27010KT";
        let expected = Wind {
            dir: WindDirection::Heading(Track(Data(270))),
            speed: WindVelocity {
                velocity: Data(10),
                gust: None,
                unit: VelocityUnit::Knots,
            },
            varying: None,
        };
        setup_test(t, expected);
    }

    #[test]
    fn test_wind_calm() {
        let t = "00000KT";
        let expected = Wind {
            dir: WindDirection::Heading(Track(Data(0))),
            speed: WindVelocity {
                velocity: Data(0),
                gust: None,
                unit: VelocityUnit::Knots,
            },
            varying: None,
        };
        setup_test(t, expected);
    }
}
