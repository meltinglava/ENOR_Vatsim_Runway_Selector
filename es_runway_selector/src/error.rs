use std::io;

use config::ConfigError;
use runway_selector_areas::AreaRegistryError;
use runway_selector_core::CoreError;
use self_update::errors::Error as SelfUpdateError;
use thiserror::Error;
use tokio::task::JoinError;

pub(crate) type ApplicationResult<T> = Result<T, ApplicationError>;

#[derive(Debug, Error)]
pub(crate) enum ApplicationError {
    #[error("Core error: {0}")]
    Core(#[from] CoreError),
    #[error("Area registry error: {0}")]
    Areas(#[from] AreaRegistryError),
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    #[error("System input/output error: {0}")]
    Io(#[from] io::Error),
    #[error("Self update error: {0}")]
    SelfUpdate(#[from] SelfUpdateError),
    #[error("Async join error: {0}")]
    AsyncJoin(#[from] JoinError),
    #[error("Plugin host error: {0}")]
    Plugin(#[from] runway_selector_plugin_host::PluginError),
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),
}
