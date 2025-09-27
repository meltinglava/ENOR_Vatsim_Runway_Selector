use nom::{
    IResult, Parser,
    bytes::complete::take,
    character::complete::{self, u32},
    combinator::{all_consuming, map_parser, opt, value},
};

use crate::optional_data::OptionalData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pressure {
    pub qnh: Option<PressureSingle>,
    pub altimeter: Option<PressureSingle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressureSingle {
    pub value: OptionalData<u32, 4>,
    pub unit: PressureUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureUnit {
    Hectopascals,
    InchesOfMercury,
}

impl PressureUnit {
    const fn pressure_letter(&self) -> char {
        match self {
            PressureUnit::Hectopascals => 'Q',
            PressureUnit::InchesOfMercury => 'A',
        }
    }
}

pub(crate) fn nom_pressure(input: &str) -> IResult<&str, Pressure> {
    let hectopascals = move |i| nom_pressure_single(i, PressureUnit::Hectopascals);
    let inches_of_mercury = move |i| nom_pressure_single(i, PressureUnit::InchesOfMercury);
    (opt(hectopascals), opt(inches_of_mercury))
        //ensure that at least one of qnh or altimeter is present
        .map_res(|(qnh, altimeter)| {
            if qnh.is_some() || altimeter.is_some() {
                Ok(Pressure { qnh, altimeter })
            } else {
                Err("At least one of QNH or Altimeter must be present")
            }
        })
        .parse(input)
}

fn nom_pressure_single(input: &str, pressure_unit: PressureUnit) -> IResult<&str, PressureSingle> {
    (
        nom_pressure_unit(pressure_unit),
        OptionalData::optional_field(map_parser(take(4usize), all_consuming(u32))),
    )
        .map(|(unit, value)| PressureSingle { value, unit })
        .parse(input)
}

fn nom_pressure_unit<'a>(
    pressure_unit: PressureUnit,
) -> impl Parser<&'a str, Output = PressureUnit, Error = nom::error::Error<&'a str>> {
    value(
        pressure_unit,
        complete::char(pressure_unit.pressure_letter()),
    )
}
