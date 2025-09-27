use nom::{Parser, character::complete::space0, combinator::opt, sequence::preceded};

use crate::{
    obscuration::{Obscuration, nom_obscuration},
    wind::{Wind, nom_wind},
};

#[derive(Debug, Clone)]
pub struct Trend {
    pub wind: Option<Wind>,
    pub obscuration: Option<Obscuration>,
    // TODO: Add more types that can come here.
}

pub(crate) fn nom_becoming(input: &str) -> nom::IResult<&str, Trend> {
    (
        opt(preceded(space0, nom_wind)),
        opt(preceded(space0, nom_obscuration)),
    )
        .map(|(wind, obscuration)| Trend { wind, obscuration })
        .parse(input)
}
