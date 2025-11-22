use std::{io, num::ParseIntError};

use config::ConfigError;
use self_update::errors::Error as SelfUpdateError;
use thiserror::Error;
use tokio::task::JoinError;
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
    MetarParseError(#[from] nom::error::Error<String>),
    #[error("Failed to parse file in a known encoding: {0}")]
    EncodingError(String),
    #[error("Time error: {0}")]
    TimeError(#[from] jiff::Error),
    #[error("No runway to set based on wind.")]
    NoRunwayToSet,
    #[error("Self update error: {0}")]
    SelfUpdateError(#[from] SelfUpdateError),
    #[error("Join error: {0}")]
    AsyncJoinError(#[from] JoinError),
    #[error("VatsimUtil error: {0}")]
    VatsimUtilError(#[from] vatsim_utils::errors::VatsimUtilError),
}
