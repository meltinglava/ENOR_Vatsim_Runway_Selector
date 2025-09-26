use std::fmt::Display;

use jiff::{Zoned, civil::date, tz::TimeZone};
use nom::{
    IResult, Parser,
    bytes::complete::take,
    character::complete::{char, i8},
    combinator::map_parser,
    sequence::terminated,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Timestamp {
    timestamp: Zoned,
}

impl Timestamp {
    pub fn new(timestamp: Zoned) -> Self {
        Timestamp { timestamp }
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.timestamp.strftime("%d%H%MZ").fmt(f)
    }
}

fn parse_double_digit(input: &str) -> IResult<&str, i8> {
    map_parser(take(2usize), i8).parse(input)
}

fn get_date_form_fields(cmp: &Zoned, day: i8, hour: i8, minute: i8) -> Zoned {
    let ordering = day
        .cmp(&cmp.day())
        .then_with(|| hour.cmp(&cmp.hour()))
        .then_with(|| minute.cmp(&cmp.minute()));
    let month = if ordering == std::cmp::Ordering::Less {
        cmp.month()
    } else {
        cmp.month() - 1
    };
    date(cmp.year(), month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(cmp.time_zone().clone())
        .unwrap()
}

pub(crate) fn nom_metar_timestamp(input: &str) -> IResult<&str, Timestamp> {
    let mut now = Zoned::now().with_time_zone(TimeZone::UTC);
    now += jiff::SignedDuration::from_hours(1);
    nom_metar_timestamp_with_zone(input, &mut now)
}

fn nom_metar_timestamp_with_zone<'a>(
    input: &'a str,
    refernce_time: &mut Zoned,
) -> IResult<&'a str, Timestamp> {
    let (rest, fields) = terminated(
        (parse_double_digit, parse_double_digit, parse_double_digit),
        char('Z'),
    )
    .parse(input)?;
    let (day, hour, minute) = fields;
    *refernce_time += jiff::SignedDuration::from_hours(1);
    let timestamp = get_date_form_fields(&refernce_time, day, hour, minute);
    Ok((rest, Timestamp { timestamp }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_timestamp() -> Zoned {
        date(2025, 06, 28)
            .at(16, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
    }

    #[test]
    fn test_metar_timestamp() {
        let input = "281250Z";
        let mut day = make_test_timestamp();
        let expected = Timestamp::new(
            day.date()
                .at(12, 50, 0, 0)
                .to_zoned(day.time_zone().clone())
                .unwrap(),
        );
        assert_eq!(
            nom_metar_timestamp_with_zone(input, &mut day),
            Ok(("", expected))
        );
    }

    #[test]
    fn test_metar_day_before() {
        let input = "271250Z";
        let mut day = make_test_timestamp();
        let expected = Timestamp::new(
            day.date()
                .yesterday()
                .unwrap()
                .at(12, 50, 0, 0)
                .to_zoned(day.time_zone().clone())
                .unwrap(),
        );
        assert_eq!(
            nom_metar_timestamp_with_zone(input, &mut day),
            Ok(("", expected))
        );
    }

    #[test]
    fn test_metar_last_month() {
        let input = "291250Z";
        let mut day = make_test_timestamp();
        let expected = Timestamp::new(
            date(2025, 05, 29)
                .at(12, 50, 0, 0)
                .to_zoned(day.time_zone().clone())
                .unwrap(),
        );
        assert_eq!(
            nom_metar_timestamp_with_zone(input, &mut day),
            Ok(("", expected))
        );
    }

    #[test]
    fn test_display() {
        let mut r = make_test_timestamp();
        let timestamp = nom_metar_timestamp_with_zone("281220Z", &mut r).unwrap().1;
        let formatted = format!("{}", timestamp);
        assert_eq!(formatted, "281220Z");
    }
}
