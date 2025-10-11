use nom::{Parser, character::complete::space0, combinator::opt, multi::many1, sequence::preceded};

use crate::{
    obscuration::{
        Cloud, PresentWeather, VerticalVisibility, Visibility, nom_cloud, nom_present_weather,
        nom_vertical_visibility, nom_visibility,
    },
    wind::{Wind, nom_wind},
};

#[derive(Debug, Clone)]
pub struct Trend {
    pub wind: Option<Wind>,
    pub visibility: Option<Visibility>,
    pub expected_present_weather: Option<Vec<PresentWeather>>,
    pub cloud_layers: Option<Vec<Cloud>>,
    pub vertical_visibility: Option<VerticalVisibility>,
    // TODO: Add more types that can come here.
}

pub(crate) fn nom_becoming(input: &str) -> nom::IResult<&str, Trend> {
    (
        opt(preceded(space0, nom_wind)),
        opt(preceded(space0, nom_visibility)),
        opt(many1(preceded(space0, nom_present_weather))),
        opt(many1(preceded(space0, nom_cloud))),
        opt(preceded(space0, nom_vertical_visibility)),
    )
        .map(
            |(wind, visibility, expected_present_weather, cloud_layers, vv)| Trend {
                wind,
                visibility,
                cloud_layers,
                expected_present_weather,
                vertical_visibility: vv,
            },
        )
        .parse(input)
}
