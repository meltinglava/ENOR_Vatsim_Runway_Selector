use std::{fmt::Display, num::ParseIntError};

use nom::{
    IResult, Parser,
    bytes::complete::take,
    character::complete::u32,
    combinator::{all_consuming, map_parser, verify},
};
use thiserror::Error;

use crate::optional_data::OptionalNumber;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Track(pub OptionalNumber<3>);

#[derive(Debug, Error, Clone)]
pub enum TrackParseError {
    #[error("Parse error: Invalid number: {0}")]
    InvalidNumber(#[from] ParseIntError),
    #[error("Parse error: Track value out of range (0-360): {0}")]
    OutOfRange(u16),
}

pub(crate) fn nom_track(input: &str) -> IResult<&str, Track> {
    OptionalNumber::optional_field(map_parser(
        take(3usize),
        all_consuming(verify(u32, |n: &u32| (0..=360).contains(n))),
    ))
    .map(|track| Track(track))
    .parse(input)
}

impl Display for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:03}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optional_data::OptionalData::Data;

    #[test]
    fn test_north() {
        let t = "36040KT";
        let result = nom_track(t);
        assert_eq!(result, Ok(("40KT", Track(Data(360)))));
    }

    #[test]
    fn test_calm() {
        let t = "00000KT";
        let result = nom_track(t);
        assert_eq!(result, Ok(("00KT", Track(Data(0)))));
    }

    #[test]
    fn test_write() {
        let track = Track(Data(80));
        let output = format!("{}", track);
        assert_eq!(output, "080");
    }
}
