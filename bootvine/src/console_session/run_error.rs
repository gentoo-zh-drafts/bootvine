use std::{env::VarError, io};

#[cfg(feature = "openrc")]
use bootvine_openrc_link::OpenrcError;
use bootvine_screen::screen_components::handover::WatchFailure;
#[cfg(feature = "systemd")]
use bootvine_systemd_link::LinkError;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[cfg(feature = "systemd")]
    #[error(transparent)]
    Systemd(#[from] LinkError),
    #[cfg(feature = "openrc")]
    #[error(transparent)]
    Openrc(#[from] OpenrcError),
    #[error("console: {0}")]
    Console(#[from] io::Error),
    #[error("reading SERVICE_RESULT: {0}")]
    ServiceResult(#[from] VarError),
    #[error("{0}")]
    Watch(WatchFailure),
    #[error("no supported service manager is running")]
    NoServiceManager,
}

impl From<RunError> for WatchFailure {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Watch(failure) => failure,
            other => Self {
                reason: other.to_string(),
            },
        }
    }
}
