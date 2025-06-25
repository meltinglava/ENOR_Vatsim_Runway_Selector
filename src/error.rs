use std::{io, num::ParseIntError};

use config::ConfigError;
use rust_flightweather::metar;
use thiserror::Error;

pub(crate) type ApplicationResult<T> = Result<T, ApplicationError>;

#[derive(Debug, Error)]
pub(crate) enum ApplicationError {
    #[error("Error regarding config: {0}")]
    ConfigError(#[from] ConfigError),
    #[error("System input/output error: {0}")]
    IoError(#[from] io::Error),
    #[error("Failed to parse integer error: {0}")]
    ParseIntError(#[from] ParseIntError),
    #[error("Error with reqwest: {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("Failed to parse METAR: {0}")]
    MetarParseError(#[from] metar::MetarError),
    #[error("Failed to parse file in a known encoding: {0}")]
    EncodingError(String),
    #[error("Time error: {0}")]
    TimeError(#[from] jiff::Error),
    #[error("No runway to set based on wind.")]
    NoRunwayToSet,
}
