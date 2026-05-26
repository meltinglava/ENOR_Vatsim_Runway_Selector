use std::{io, num::ParseIntError};

use thiserror::Error;
use tokio::task::JoinError;

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("System input/output error: {0}")]
    Io(#[from] io::Error),
    #[error("Failed to parse integer: {0}")]
    ParseInt(#[from] ParseIntError),
    #[error("HTTP error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Failed to parse METAR: {0}")]
    MetarParse(#[from] nom::error::Error<String>),
    #[error("Failed to decode sector file (tried UTF-8 and ISO-8859-1): {0}")]
    Encoding(String),
    #[error("Failed to parse area configuration: {0}")]
    AreaConfig(String),
    #[error("Time error: {0}")]
    Time(#[from] jiff::Error),
    #[error("No runway to set based on wind")]
    NoRunwayToSet,
    #[error("Async join error: {0}")]
    AsyncJoin(#[from] JoinError),
    #[error("VATSIM API error: {0}")]
    VatsimUtil(#[from] vatsim_utils::errors::VatsimUtilError),
}
