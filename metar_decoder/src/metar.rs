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
use tracing::warn;

use crate::{
    nato_mil_code::{NatoMilCode, nom_nato_mil_code},
    obscuration::{Obscuration, PresentWeather, nom_obscuration, nom_recent_present_weather},
    optional_data::OptionalData,
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
    pub corrected: bool,
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
    pub nato_mil_code: Option<OptionalData<NatoMilCode, 3>>,
    pub remarks: Option<String>,
}

impl FromStr for Metar {
    type Err = nom::error::Error<String>;

    #[tracing::instrument(name = "metar_parse")]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (rest, metar) = nom_parse_metar(s).finish().inspect_err(|e| {
            warn!(?e);
        })?;
        if !rest.is_empty() {
            warn!(?rest, "Unparsed input remains");
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
pub(crate) fn debug<P, I, O, E>(
    name: &'static str,
    mut parser: P,
) -> impl FnMut(I) -> IResult<I, O, E>
where
    P: Parser<I, Output = O, Error = E>,
    I: Clone + AsBytes + Debug,
    E: ParseError<I> + Debug,
    O: Debug,
{
    move |input: I| {
        eprintln!("→ {name} | input: {}", preview(&input));
        let res = parser.parse(input.clone());
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

fn nom_parse_metar(input: &str) -> IResult<&str, Metar> {
    let (
        rest,
        (
            icao,
            timestamp,
            corrected,
            auto,
            wind,
            (obscuration, temprature, pressure),
            recent_weather,
            sea_surface_indicator,
            nato_mil_code,
            nosig,
            becoming,
            tempo,
            remark,
        ),
    ) = (
        nom::bytes::complete::take(4usize),
        preceded(char(' '), nom_metar_timestamp),
        opt(tag(" COR")),
        opt(tag(" AUTO")),
        preceded(char(' '), nom_wind),
        permutation((
            preceded(char(' '), nom_obscuration),
            preceded(opt(char(' ')), nom_temprature_info),
            preceded(char(' '), nom_pressure),
        )),
        preceded(space0, opt(nom_recent_present_weather)),
        preceded(space0, opt(nom_sea_surface_indicator)),
        opt(preceded(
            space0,
            OptionalData::optional_field(nom_nato_mil_code),
        )),
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
            corrected: corrected.is_some(),
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
            nato_mil_code,
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
    use tracing_test::traced_test;

    #[test]
    #[traced_test]
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
    #[traced_test]
    fn test_parseble_1() {
        let input = "ENBR 111150Z 25006KT 9999 VCSH FEW005 SCT011 BKN014 12/10 Q1026 TEMPO SCT014 BKN020 RMK WIND 1200FT 27013KT";
        Metar::from_str(input).unwrap();
    }

    #[test]
    #[traced_test]
    fn test_parseble_2() {
        let input = "ENBL 111220Z 25006KT 200V290 1000 R07/0600 FG DZ SCT005 BKN010 09/09 Q1024";
        Metar::from_str(input).unwrap();
    }

    #[test]
    #[traced_test]
    fn test_parseble_3() {
        let input = "ENZV 111920Z 30010KT 4000 -DZ BR VV007 13/12 Q1027 TEMPO 1200 DZ VV003";
        Metar::from_str(input).unwrap();
    }

    #[test]
    #[traced_test]
    fn test_parseble_4() {
        let input = "ENBR 120520Z 32007KT 3500 DZ VV005 11/11 Q1026 TEMPO 9999 NSW SCT008 BKN015 RMK WIND 1200FT 33014KT";
        Metar::from_str(input).unwrap();
    }

    #[test]
    #[traced_test]
    #[tracing::instrument(name = "EGWC 121350Z AUTO 05002KT //// ///////// ///// Q////")]
    fn test_parseble_5() {
        let input = "EGWC 121350Z AUTO 05002KT //// ///////// ///// Q////";
        Metar::from_str(input).unwrap();
    }

    #[test]
    #[ignore = "only used for testing localy"]
    #[traced_test]
    fn test_found_bad_metars() {
        let path = std::path::Path::new("../failed_metars.json");
        let rdr = std::fs::File::open(path).unwrap();
        let metars: Vec<String> = serde_json::from_reader(rdr).unwrap();
        let mut fail_found = false;
        for m in metars {
            if let Err(_e) = Metar::from_str(&m) {
                fail_found = true;
            }
        }
        assert!(!fail_found, "Fails to parse metars");
    }
}
