use std::{
    io::{BufRead, BufReader, Read},
    str::FromStr,
};

use itertools::Itertools;
use nom::{
    Finish, IResult, Parser,
    bytes::complete::tag,
    character::complete::{char, space0},
    combinator::{opt, rest},
    sequence::preceded,
};

use crate::{
    obscuration::{Obscuration, nom_obscuration},
    pressure::{Pressure, nom_pressure},
    sea_surface_indicator::{SeaSurfaceIndicator, nom_sea_surface_indicator},
    temprature::{TempratureInfo, nom_temprature_info},
    units::timestamp::{Timestamp, nom_metar_timestamp},
    wind::{Wind, nom_wind},
};

#[derive(Debug, Clone)]
pub struct Metar {
    pub raw: String,
    pub icao: String,
    pub timestamp: Timestamp,
    pub auto: bool,
    pub wind: Wind,
    pub obscuration: Obscuration,
    pub temprature: TempratureInfo,
    pub pressure: Pressure,
    pub nosig: bool,
    pub sea_surface_indicator: Option<SeaSurfaceIndicator>,
    pub remarks: Option<String>,
}

impl FromStr for Metar {
    type Err = nom::error::Error<String>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (rest, metar) = nom_parse_metar(s).finish()?;
        if !rest.is_empty() {
            Err(nom::error::Error::new(
                rest.to_string(),
                nom::error::ErrorKind::NonEmpty,
            ))
        } else {
            Ok(metar)
        }
    }
}

pub fn nom_parse_metar(input: &str) -> IResult<&str, Metar> {
    let (
        rest,
        (
            icao,
            timestamp,
            auto,
            wind,
            obscuration,
            temprature,
            pressure,
            sea_surface_indicator,
            nosig,
            remark,
        ),
    ) = (
        nom::bytes::complete::take(4usize),
        preceded(char(' '), nom_metar_timestamp),
        opt(tag(" AUTO")),
        preceded(char(' '), nom_wind),
        preceded(char(' '), nom_obscuration),
        preceded(opt(char(' ')), nom_temprature_info),
        preceded(char(' '), nom_pressure),
        preceded(space0, opt(nom_sea_surface_indicator)),
        opt(preceded(space0, tag("NOSIG"))),
        opt(preceded(
            (space0, tag("RMK ")),
            rest.map_res(|s| match s {
                "" => Err("Empty remark"),
                _ => Ok(s),
            }),
        )),
    )
        .parse(input)?;
    Ok((
        rest,
        Metar {
            raw: input.to_string(),
            icao: icao.to_string(),
            timestamp,
            auto: auto.is_some(),
            wind,
            obscuration,
            temprature,
            pressure,
            nosig: nosig.is_some(),
            sea_surface_indicator,
            remarks: remark.map(str::to_string),
        },
    ))
}

fn parse_metars<R: Read>(input: R) -> Result<Vec<(String, Metar)>, nom::error::Error<String>> {
    let reader = BufReader::new(input);
    let results = reader
        .lines()
        .filter_map(Result::ok)
        .map(|m| -> Result<(String, Metar), nom::error::Error<String>> {
            let (rest, metar) = nom_parse_metar(&m).finish()?;
            Ok((rest.to_string(), metar))
        })
        .try_collect();
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        let input = include_str!("../test.metars");
        input
            .lines()
            .for_each(|line| match nom_parse_metar(line.trim()) {
                Ok((rest, metar)) => {
                    assert!(
                        rest.is_empty(),
                        "Unparsed input remains: '{}'\n input: '{}'\n data: {:?}",
                        rest,
                        line,
                        metar
                    );
                }
                Err(e) => {
                    panic!("Failed to parse line '{}': {:?}", line, e);
                }
            });
    }
}
