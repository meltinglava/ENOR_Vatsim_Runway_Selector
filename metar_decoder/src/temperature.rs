use nom::{
    IResult, Parser,
    character::complete::{char, i32},
    combinator::opt,
    sequence::separated_pair,
};

use crate::optional_data::OptionalData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemperatureInfo {
    pub temp: OptionalData<i32, 2>,
    pub dew_point: OptionalData<i32, 2>,
}

pub(crate) fn nom_temperature_info(input: &str) -> IResult<&str, TemperatureInfo> {
    separated_pair(
        OptionalData::optional_field(nom_maybe_negative_temp),
        char('/'),
        OptionalData::optional_field(nom_maybe_negative_temp),
    )
    .map(|(temp, dew_point)| TemperatureInfo { temp, dew_point })
    .parse(input)
}

pub(crate) fn nom_maybe_negative_temp(input: &str) -> IResult<&str, i32> {
    (opt(char('M')), i32)
        .map(|(sign, temp)| if sign.is_some() { -temp } else { temp })
        .parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let input = "34/12";
        let expected = TemperatureInfo {
            temp: OptionalData::Data(34),
            dew_point: OptionalData::Data(12),
        };
        let result = nom_temperature_info(input);
        assert_eq!(Ok(("", expected)), result);

        let input_with_negative = "M12/M34";
        let expected_with_negative = TemperatureInfo {
            temp: OptionalData::Data(-12),
            dew_point: OptionalData::Data(-34),
        };
        let result_with_negative = nom_temperature_info(input_with_negative);
        assert_eq!(Ok(("", expected_with_negative)), result_with_negative);
    }
}
