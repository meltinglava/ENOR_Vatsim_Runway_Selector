use std::fmt::{self, Display};

use nom::{
    Parser, branch::alt, bytes::complete::tag, character::complete::u32, combinator::opt,
    sequence::preceded,
};

use crate::optional_data::OptionalNumber;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct WindVelocity {
    pub velocity: OptionalNumber<2>,
    pub gust: Option<OptionalNumber<2>>,
    pub unit: VelocityUnit,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum VelocityUnit {
    MetersPerSecond,
    Knots,
}

impl WindVelocity {
    pub fn get_max_wind_speed(&self) -> Option<u32> {
        self.gust.unwrap_or(self.velocity).to_option()
    }
}

pub(crate) fn nom_velocity(input: &str) -> nom::IResult<&str, WindVelocity> {
    let (rest, v) = OptionalNumber::optional_field(u32).parse(input)?;
    let (rest, gust) = opt(preceded(tag("G"), OptionalNumber::optional_field(u32))).parse(rest)?;
    let (rest, unit) = alt((tag("KT"), tag("MPS"))).parse(rest)?;
    let unit = match unit {
        "KT" => VelocityUnit::Knots,
        "MPS" => VelocityUnit::MetersPerSecond,
        _ => {
            return Err(nom::Err::Error(nom::error::Error::new(
                input,
                nom::error::ErrorKind::Tag,
            )));
        }
    };
    Ok((
        rest,
        WindVelocity {
            velocity: v,
            unit,
            gust,
        },
    ))
}

impl Display for WindVelocity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.velocity)?;
        if let Some(gust) = self.gust {
            write!(f, "G{}", gust)?;
        }
        write!(
            f,
            "{}",
            match self.unit {
                VelocityUnit::Knots => "KT",
                VelocityUnit::MetersPerSecond => "MPS",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::optional_data::OptionalData::Data;

    fn setup_test(s: &str, v: WindVelocity) {
        let result = nom_velocity(s);
        let (remaining, velocity) = result.unwrap();
        assert_eq!(remaining, "");
        assert_eq!(velocity.velocity, v.velocity);
        assert_eq!(velocity.unit, v.unit);
    }

    #[test]
    fn test_knot() {
        setup_test(
            "10KT",
            WindVelocity {
                velocity: Data(10),
                unit: VelocityUnit::Knots,
                gust: None,
            },
        );
    }

    #[test]
    fn test_mps() {
        setup_test(
            "10MPS",
            WindVelocity {
                velocity: Data(10),
                unit: VelocityUnit::MetersPerSecond,
                gust: None,
            },
        );
    }

    #[test]
    fn test_invalid_input() {
        let result = nom_velocity("10XYZ");
        assert!(result.is_err());
    }

    #[test]
    fn test_display_kt() {
        let v = WindVelocity {
            velocity: Data(10),
            unit: VelocityUnit::Knots,
            gust: None,
        };
        assert_eq!(v.to_string(), "10KT");
    }

    #[test]
    fn test_display_mps() {
        let v = WindVelocity {
            velocity: Data(10),
            unit: VelocityUnit::MetersPerSecond,
            gust: None,
        };
        assert_eq!(v.to_string(), "10MPS");
    }

    #[test]
    fn test_gust() {
        let wind = "13G19KT";
        let result = nom_velocity(wind);
        assert_eq!(
            result,
            Ok((
                "",
                WindVelocity {
                    velocity: Data(13),
                    gust: Some(Data(19)),
                    unit: VelocityUnit::Knots
                }
            ))
        );
    }
}
