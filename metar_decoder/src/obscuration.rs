use nom::{
    Parser,
    branch::alt,
    bytes::complete::{tag, take},
    character::complete::{self, alphanumeric1, u32},
    combinator::{all_consuming, map, map_parser, map_res, opt, value},
    multi::{many0, many1, separated_list0},
    sequence::{preceded, separated_pair, terminated},
};

use crate::{
    optional_data::OptionalData,
    units::altitudes::{CloudHeight, nom_cloud_height},
};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Obscuration {
    Described(DescribedObscuration),
    Cavok,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DescribedObscuration {
    pub visibility: Visibility,
    pub rvr: Vec<Rvr>,
    pub clouds: Vec<Cloud>,
    pub present_weather: Vec<PresentWeather>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Rvr {
    pub runway: String,
    pub value: OptionalData<u32, 4>,
    pub distance_modifier: Option<DistanceModifier>,
    pub comment: Option<Trend>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Trend {
    Increasing,
    Decreasing,
    NoDistinctChange,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Cloud {
    NCD, // No cloud detected
    NSC, // No significant clouds
    CloudData(CloudData),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CloudData {
    pub coverage: OptionalData<CloudCoverage, 3>,
    pub height: OptionalData<CloudHeight, 3>,
    pub cloud_type: Option<OptionalData<String, 3>>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CloudCoverage {
    Few,
    Scattered,
    Broken,
    Overcast,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Visibility {
    pub value: VisibilityUnit,
    pub ndv: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum VisibilityUnit {
    Meters(OptionalData<u32, 4>),
    StatuteMiles(StatuteMilesVisibility),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct StatuteMilesVisibility {
    pub whole: Option<u32>,
    pub fraction: Option<(u32, u32)>,
    pub modifier: Option<DistanceModifier>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DistanceModifier {
    LessThan,
    GreaterThan,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PresentWeather {
    pub intensity: Option<WeatherIntensity>,
    pub descriptor: Option<Qualifier>,
    pub phenomena: Vec<WeatherPhenomenon>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WeatherIntensity {
    Light,    // -
    Heavy,    // +
    Vicinity, // VC
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Qualifier {
    Shallow,      // MI
    Patches,      // BC
    Partial,      // PR
    Low,          // DR_drifting
    Blowing,      // BL
    Showers,      // SH
    Thunderstorm, // TS
    Freezing,     // FZ
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WeatherPhenomenon {
    DZ, // Drizzle
    RA, // Rain
    SN, // Snow
    SG, // Snow grains
    PL, // Ice pellets
    GR, // Hail
    GS, // Small hail and/or snow pellets
    UP, // Unknown precipitation
    BR, // Mist
    FG, // Fog
    FU, // Smoke
    VA, // Volcanic ash
    DU, // Widespread dust
    SA, // Sand
    HZ, // Haze
    PO, // Dust/sandwhirls (dust devils)
    SQ, // Squalls
    FC, // Funnelcloud
    SS, // Sandstorm
    DS, // Duststorm
}

pub(crate) fn nom_obscuration(input: &str) -> nom::IResult<&str, Obscuration> {
    alt((
        value(Obscuration::Cavok, tag("CAVOK")),
        map(nom_described_obscuration, Obscuration::Described),
    ))
    .parse(input)
}

fn nom_described_obscuration(input: &str) -> nom::IResult<&str, DescribedObscuration> {
    map(
        (
            nom_visibility,
            preceded(
                opt(complete::char(' ')),
                separated_list0(complete::char(' '), nom_present_weather),
            ),
            preceded(
                opt(complete::char(' ')),
                separated_list0(complete::char(' '), nom_rvr),
            ),
            preceded(
                opt(complete::char(' ')),
                separated_list0(complete::char(' '), nom_cloud),
            ),
        ),
        |(visibility, present_weather, rvr, clouds)| DescribedObscuration {
            visibility,
            rvr,
            present_weather,
            clouds,
        },
    )
    .parse(input)
}

fn nom_visibility(input: &str) -> nom::IResult<&str, Visibility> {
    (
        alt((
            map(nom_statute_miles_visibility, VisibilityUnit::StatuteMiles),
            map(
                OptionalData::optional_field(map_parser(take(4usize), all_consuming(u32))),
                VisibilityUnit::Meters,
            ),
        )),
        opt(tag("NDV")).map(|ndv| ndv.is_some()),
    )
        .map(|(value, ndv)| Visibility { value, ndv })
        .parse(input)
}

fn nom_fraction(input: &str) -> nom::IResult<&str, (u32, u32)> {
    separated_pair(u32, tag("/"), u32).parse(input)
}

fn nom_statute_miles_visibility(input: &str) -> nom::IResult<&str, StatuteMilesVisibility> {
    let fraction_only = map(
        (
            opt(nom_distance_modifier),
            terminated(nom_fraction, tag("SM")),
        ),
        |(modifier, fraction)| StatuteMilesVisibility {
            whole: None,
            fraction: Some(fraction),
            modifier,
        },
    );

    let whole = map(
        terminated(
            (
                opt(nom_distance_modifier),
                u32,
                opt(preceded(tag(" "), nom_fraction)),
            ),
            tag("SM"),
        ),
        |(modifier, whole, fraction)| StatuteMilesVisibility {
            whole: Some(whole),
            fraction,
            modifier,
        },
    );

    alt((fraction_only, whole)).parse(input)
}

fn nom_distance_modifier(input: &str) -> nom::IResult<&str, DistanceModifier> {
    alt((
        value(DistanceModifier::LessThan, tag("M")),
        value(DistanceModifier::GreaterThan, tag("P")),
    ))
    .parse(input)
}

fn nom_rvr(input: &str) -> nom::IResult<&str, Rvr> {
    map(
        preceded(
            tag("R"),
            separated_pair(
                alphanumeric1,
                tag("/"),
                (
                    opt(nom_distance_modifier),
                    OptionalData::optional_field(map_parser(take(4usize), all_consuming(u32))),
                    opt(alt((
                        value(Trend::Decreasing, tag("D")),
                        value(Trend::Increasing, tag("U")),
                        value(Trend::NoDistinctChange, tag("N")),
                    ))),
                ),
            ),
        ),
        |(runway, (distance_modifier, value, comment))| Rvr {
            runway: runway.to_string(),
            value,
            distance_modifier,
            comment,
        },
    )
    .parse(input)
}

fn nom_cloud_coverage(input: &str) -> nom::IResult<&str, OptionalData<CloudCoverage, 3>> {
    OptionalData::optional_field(alt((
        value(CloudCoverage::Few, tag("FEW")),
        value(CloudCoverage::Scattered, tag("SCT")),
        value(CloudCoverage::Broken, tag("BKN")),
        value(CloudCoverage::Overcast, tag("OVC")),
    )))
    .parse(input)
}

fn nom_cloud_type(input: &str) -> nom::IResult<&str, OptionalData<String, 3>> {
    OptionalData::optional_field(map(alphanumeric1, |s: &str| s.to_string())).parse(input)
}

fn nom_cloud(input: &str) -> nom::IResult<&str, Cloud> {
    alt((
        value(Cloud::NCD, tag("NCD")),
        value(Cloud::NSC, tag("NSC")),
        map(nom_cloud_data, Cloud::CloudData),
    ))
    .parse(input)
}

fn nom_cloud_data(input: &str) -> nom::IResult<&str, CloudData> {
    let (input, coverage) = nom_cloud_coverage.parse(input)?;
    let (input, height) = nom_cloud_height.parse(input)?;
    let (input, cloud_type) = opt(nom_cloud_type).parse(input)?;
    Ok((
        input,
        CloudData {
            coverage,
            height,
            cloud_type,
        },
    ))
}

fn nom_present_weather(input: &str) -> nom::IResult<&str, PresentWeather> {
    map_res(
        (
            opt(alt((
                value(WeatherIntensity::Light, tag("-")),
                value(WeatherIntensity::Heavy, tag("+")),
                value(WeatherIntensity::Vicinity, tag("VC")),
            ))),
            opt(alt((
                value(Qualifier::Shallow, tag("MI")),
                value(Qualifier::Patches, tag("BC")),
                value(Qualifier::Partial, tag("PR")),
                value(Qualifier::Low, tag("DR")),
                value(Qualifier::Blowing, tag("BL")),
                value(Qualifier::Showers, tag("SH")),
                value(Qualifier::Thunderstorm, tag("TS")),
                value(Qualifier::Freezing, tag("FZ")),
            ))),
            many0(alt((
                value(WeatherPhenomenon::DZ, tag("DZ")),
                value(WeatherPhenomenon::RA, tag("RA")),
                value(WeatherPhenomenon::SN, tag("SN")),
                value(WeatherPhenomenon::SG, tag("SG")),
                value(WeatherPhenomenon::PL, tag("PL")),
                value(WeatherPhenomenon::GR, tag("GR")),
                value(WeatherPhenomenon::GS, tag("GS")),
                value(WeatherPhenomenon::UP, tag("UP")),
                value(WeatherPhenomenon::BR, tag("BR")),
                value(WeatherPhenomenon::FG, tag("FG")),
                value(WeatherPhenomenon::FU, tag("FU")),
                value(WeatherPhenomenon::VA, tag("VA")),
                value(WeatherPhenomenon::DU, tag("DU")),
                value(WeatherPhenomenon::SA, tag("SA")),
                value(WeatherPhenomenon::HZ, tag("HZ")),
                value(WeatherPhenomenon::PO, tag("PO")),
                value(WeatherPhenomenon::SQ, tag("SQ")),
                value(WeatherPhenomenon::FC, tag("FC")),
                value(WeatherPhenomenon::SS, tag("SS")),
                value(WeatherPhenomenon::DS, tag("DS")),
            ))),
        ),
        |(intensity, descriptor, phenomena)| {
            if descriptor.is_none() && phenomena.is_empty() {
                Err("At least one of descriptor, or phenomena must be present")
            } else {
                Ok(PresentWeather {
                    intensity,
                    descriptor,
                    phenomena,
                })
            }
        },
    )
    .parse(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unknown_type() {
        let input = "OVC018///";
        let expected = CloudData {
            coverage: OptionalData::Data(CloudCoverage::Overcast),
            height: OptionalData::Data(CloudHeight { height: 18 }),
            cloud_type: Some(OptionalData::Undefined),
        };
        assert_eq!(nom_cloud_data(input), Ok(("", expected)));
    }

    #[test]
    fn test_known_error() {
        let input = "9999 OVC018///";
        let expected = DescribedObscuration {
            visibility: Visibility {
                value: VisibilityUnit::Meters(OptionalData::Data(9999)),
                ndv: false,
            },
            rvr: vec![],
            clouds: vec![Cloud::CloudData(CloudData {
                coverage: OptionalData::Data(CloudCoverage::Overcast),
                height: OptionalData::Data(CloudHeight { height: 18 }),
                cloud_type: Some(OptionalData::Undefined),
            })],
            present_weather: vec![],
        };
        assert_eq!(nom_described_obscuration(input), Ok(("", expected)));
    }
}
