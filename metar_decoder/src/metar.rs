use std::{
    fmt::Debug,
    io::{BufRead, BufReader, Read},
    str::FromStr,
};

use itertools::Itertools;
use nom::{
    AsBytes, Finish, IResult, Parser,
    branch::permutation,
    bytes::complete::tag,
    character::complete::{char, space0},
    combinator::{opt, rest},
    error::ParseError,
    sequence::preceded,
};

use crate::{
    obscuration::{Obscuration, PresentWeather, nom_obscuration, nom_recent_present_weather},
    pressure::{Pressure, nom_pressure},
    sea_surface_indicator::{SeaSurfaceIndicator, nom_sea_surface_indicator},
    temprature::{TempratureInfo, nom_temprature_info},
    trend::{Trend, nom_becoming},
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
    pub recent_weather: Option<Vec<PresentWeather>>,
    pub nosig: bool,
    pub sea_surface_indicator: Option<SeaSurfaceIndicator>,
    pub tempo: Option<Trend>,
    pub becoming: Option<Trend>,
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

#[allow(unused)]
pub(crate) fn debug<P, I, O, E>(name: &'static str, mut f: P) -> impl FnMut(I) -> IResult<I, O, E>
where
    P: FnMut(I) -> IResult<I, O, E>,
    I: Clone + AsBytes + Debug,
    E: ParseError<I> + Debug,
    O: Debug,
{
    move |input: I| {
        eprintln!("→ {name} | input: {}", preview(&input));
        let res = f(input.clone());
        match &res {
            Ok((rest, out)) => {
                let in_len = input.as_bytes().len();
                let rest_len = rest.as_bytes().len();
                let consumed = in_len.saturating_sub(rest_len);
                eprintln!(
                    "← {name} | OK    | out={:?} | rest: {} | consumed: {}",
                    short_dbg(out),
                    preview(rest),
                    consumed
                );
            }
            Err(nom::Err::Error(e)) => {
                eprintln!("← {name} | ERROR | {e:?}");
            }
            Err(nom::Err::Failure(e)) => {
                eprintln!("← {name} | FAIL  | {e:?}");
            }
            Err(nom::Err::Incomplete(needed)) => {
                eprintln!("← {name} | INCOMPLETE | needed: {needed:?}");
            }
        }
        res
    }
}

fn preview<I: AsBytes>(i: &I) -> String {
    let b = i.as_bytes();
    let take = core::cmp::min(48, b.len());
    let head = String::from_utf8_lossy(&b[..take]);
    let ell = if b.len() > take { "…" } else { "" };
    format!("\"{head}{ell}\" (len {})", b.len())
}

fn short_dbg<T: Debug>(t: &T) -> String {
    let s = format!("{t:?}");
    const MAX: usize = 120;
    if s.len() > MAX {
        format!("{}…", &s[..MAX])
    } else {
        s
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
            (obscuration, pressure, temprature),
            recent_weather,
            sea_surface_indicator,
            nosig,
            becoming,
            tempo,
            remark,
        ),
    ) = (
        nom::bytes::complete::take(4usize),
        preceded(char(' '), nom_metar_timestamp),
        opt(tag(" AUTO")),
        preceded(char(' '), nom_wind),
        permutation((
            preceded(char(' '), nom_obscuration),
            preceded(char(' '), nom_pressure),
            preceded(opt(char(' ')), nom_temprature_info),
        )),
        preceded(space0, opt(nom_recent_present_weather)),
        preceded(space0, opt(nom_sea_surface_indicator)),
        opt(preceded(space0, tag("NOSIG"))),
        opt(preceded((space0, tag("BECMG")), nom_becoming)),
        opt(preceded((space0, tag("TEMPO")), nom_becoming)),
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
            recent_weather,
            nosig: nosig.is_some(),
            sea_surface_indicator,
            becoming,
            tempo,
            remarks: remark.map(str::to_string),
        },
    ))
}

#[allow(unused)]
fn parse_metars<R: Read>(input: R) -> Result<Vec<(String, Metar)>, nom::error::Error<String>> {
    let reader = BufReader::new(input);
    reader
        .lines()
        .map_while(Result::ok)
        .map(|s| s.trim().to_owned())
        .map(|m| -> Result<(String, Metar), nom::error::Error<String>> {
            let (rest, metar) = nom_parse_metar(&m).finish()?;
            Ok((rest.to_string(), metar))
        })
        .try_collect()
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

    #[test]
    fn test_parseble() {
        let input = "ENBR 111150Z 25006KT 9999 VCSH FEW005 SCT011 BKN014 12/10 Q1026 TEMPO SCT014 BKN020 RMK WIND 1200FT 27013KT";
        Metar::from_str(input).unwrap();
    }
}
