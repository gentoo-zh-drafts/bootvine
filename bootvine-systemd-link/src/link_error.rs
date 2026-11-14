use std::{convert::Infallible, io};

use bootvine_screen::screen_components::handover::WatchFailure;
use rustix::io::Errno;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LinkFailure {
    #[error("systemd bus: {0}")]
    Bus(#[from] zbus::Error),
    #[error("systemd refused the call: {0}")]
    Refused(BusRefusal),
    #[error("starting a runtime to reach systemd: {0}")]
    CallRuntime(io::Error),
    #[error("the service manager closed the connection")]
    Disconnected,
    #[error("the service manager did not answer in time")]
    CallTimedOut,
    #[error("console: {0}")]
    Console(#[from] io::Error),
    #[error("entering the new root: {0}")]
    RootSwitch(#[from] Errno),
    #[error("the service manager did not come back: {0}")]
    ManagerUnreachable(Box<LinkFailure>),
}

#[derive(Debug, thiserror::Error)]
#[error("{name}: {description}")]
pub(crate) struct BusRefusal {
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct LinkError(#[from] LinkFailure);

impl From<io::Error> for LinkError {
    fn from(error: io::Error) -> Self {
        Self(LinkFailure::Console(error))
    }
}

impl From<LinkFailure> for WatchFailure {
    fn from(failure: LinkFailure) -> Self {
        Self {
            reason: failure.to_string(),
        }
    }
}

impl From<LinkError> for WatchFailure {
    fn from(error: LinkError) -> Self {
        error.0.into()
    }
}

impl From<Infallible> for LinkFailure {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}
