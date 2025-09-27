use nom::{
    Parser,
    branch::alt,
    character::complete,
    sequence::{preceded, separated_pair},
};

use crate::{optional_data::OptionalData, temprature::nom_maybe_negative_temp};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SeaSurfaceIndicator {
    pub temperature: OptionalData<i32, 2>,
    pub state_of_sea: OptionalData<StateOfSea, 2>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum StateOfSea {
    SeaSate(OptionalData<CodeTable3700, 1>),
    SignificantWaveHeight(OptionalData<u16, 3>), // in decimeters
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[repr(u8)]
pub enum CodeTable3700 {
    CalmGlassLike = 0,
    CalmRippled = 1,
    Smooth = 2,
    Slight = 3,
    Moderate = 4,
    Rough = 5,
    VeryRough = 6,
    High = 7,
    VeryHigh = 8,
    Phenomenal = 9,
}

pub(crate) fn nom_sea_surface_indicator(input: &str) -> nom::IResult<&str, SeaSurfaceIndicator> {
    let (rest, (temperature, state_of_sea)) = separated_pair(
        preceded(
            complete::char('W'),
            OptionalData::optional_field(nom_maybe_negative_temp),
        ),
        complete::char('/'),
        OptionalData::optional_field(alt((nom_state_of_sea, nom_significant_wave_height))),
    )
    .parse(input)?;
    Ok((
        rest,
        SeaSurfaceIndicator {
            temperature,
            state_of_sea,
        },
    ))
}

pub(crate) fn nom_state_of_sea(input: &str) -> nom::IResult<&str, StateOfSea> {
    preceded(
        complete::char('S'),
        OptionalData::optional_field(complete::u8.map_res(|code| match code {
            0 => Ok(CodeTable3700::CalmGlassLike),
            1 => Ok(CodeTable3700::CalmRippled),
            2 => Ok(CodeTable3700::Smooth),
            3 => Ok(CodeTable3700::Slight),
            4 => Ok(CodeTable3700::Moderate),
            5 => Ok(CodeTable3700::Rough),
            6 => Ok(CodeTable3700::VeryRough),
            7 => Ok(CodeTable3700::High),
            8 => Ok(CodeTable3700::VeryHigh),
            9 => Ok(CodeTable3700::Phenomenal),
            _ => Err("Invalid code for State of Sea"),
        })),
    )
    .map(StateOfSea::SeaSate)
    .parse(input)
}

pub(crate) fn nom_significant_wave_height(input: &str) -> nom::IResult<&str, StateOfSea> {
    preceded(
        complete::char('H'),
        OptionalData::optional_field(complete::u16),
    )
    .map(StateOfSea::SignificantWaveHeight)
    .parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_INPUT_1: [(&str, SeaSurfaceIndicator); 6] = [
        (
            "W19/S4",
            SeaSurfaceIndicator {
                temperature: OptionalData::Data(19),
                state_of_sea: OptionalData::Data(StateOfSea::SeaSate(OptionalData::Data(CodeTable3700::Moderate))),
            },
        ),
        (
            "WM12/H75",
            SeaSurfaceIndicator {
                temperature: OptionalData::Data(-12),
                state_of_sea: OptionalData::Data(StateOfSea::SignificantWaveHeight(OptionalData::Data(75))),
            },
        ),
        (
            "W///S3",
            SeaSurfaceIndicator {
                temperature: OptionalData::Undefined,
                state_of_sea: OptionalData::Data(StateOfSea::SeaSate(OptionalData::Data(CodeTable3700::Slight))),
            },
        ),
        (
            "W17/S/",
            SeaSurfaceIndicator {
                temperature: OptionalData::Data(17),
                state_of_sea: OptionalData::Data(StateOfSea::SeaSate(OptionalData::Undefined)),
            },
        ),
        (
            "W17/H///",
            SeaSurfaceIndicator {
                temperature: OptionalData::Data(17),
                state_of_sea: OptionalData::Data(StateOfSea::SignificantWaveHeight(OptionalData::Undefined)),
            },
        ),
        (
            "W22///",
            SeaSurfaceIndicator {
                temperature: OptionalData::Data(22),
                state_of_sea: OptionalData::Undefined,
            },
        ),
    ];

    #[test]
    fn test_name() {
        for (input, expected) in TEST_INPUT_1 {
            let result = nom_sea_surface_indicator(input);
            assert_eq!(Ok(("", expected)), result, "Failed on input: {}", input);
        }
    }
}
