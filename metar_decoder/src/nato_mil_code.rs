use nom::{IResult, Parser, branch::alt, bytes::complete::tag, combinator::value};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum NatoMilCode {
    Blue,
    White,
    Green,
    Yellow,
    Amber,
    Red,
    Black,
}

pub(crate) fn nom_nato_mil_code(input: &str) -> IResult<&str, NatoMilCode> {
    alt((
        value(NatoMilCode::Blue, tag("BLU")),
        value(NatoMilCode::White, tag("WHT")),
        value(NatoMilCode::Green, tag("GRN")),
        value(NatoMilCode::Yellow, tag("YLO")),
        value(NatoMilCode::Amber, tag("AMB")),
        value(NatoMilCode::Red, tag("RED")),
        value(NatoMilCode::Black, tag("BLACK")),
    ))
    .parse(input)
}
