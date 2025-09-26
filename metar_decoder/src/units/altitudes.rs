use std::fmt::{self, Display};

use nom::{
    IResult, Parser,
    bytes::complete::take,
    character::complete::i32,
    combinator::{map, map_parser},
};

use crate::optional_data::OptionalData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudHeight {
    pub height: i32,
}

impl Display for CloudHeight {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:03}FT", self.height)
    }
}

pub(crate) fn nom_cloud_height(input: &str) -> IResult<&str, OptionalData<CloudHeight, 3>> {
    OptionalData::optional_field(map(map_parser(take(3usize), i32), |height| CloudHeight {
        height,
    }))
    .parse(input)
}
