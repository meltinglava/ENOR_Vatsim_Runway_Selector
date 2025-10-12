use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::tag,
    character::complete::one_of,
    combinator::{opt, value},
    multi::many1,
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct NatoMilCode {
    pub codes: Vec<NatoMilCodeType>,
    pub modifier: Option<char>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum NatoMilCodeType {
    Blue,
    White,
    Green,
    Yellow,
    Amber,
    Red,
    Black,
}

pub(crate) fn nom_nato_mil_code(input: &str) -> IResult<&str, NatoMilCode> {
    (
        many1(alt((
            value(NatoMilCodeType::Blue, tag("BLU")),
            value(NatoMilCodeType::White, tag("WHT")),
            value(NatoMilCodeType::Green, tag("GRN")),
            value(NatoMilCodeType::Yellow, tag("YLO")),
            value(NatoMilCodeType::Amber, tag("AMB")),
            value(NatoMilCodeType::Red, tag("RED")),
            value(NatoMilCodeType::Black, tag("BLACK")),
        ))),
        opt(one_of("+-")),
    )
        .map(|(codes, modifier)| NatoMilCode { codes, modifier })
        .parse(input)
}
