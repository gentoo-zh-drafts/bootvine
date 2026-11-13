use std::path::Path;

#[cfg(feature = "openrc")]
use bootvine_openrc_link::OpenrcLink;
#[cfg(feature = "systemd")]
use bootvine_systemd_link::SystemdLink;

use crate::console_session::run_error::RunError;

#[cfg(feature = "systemd")]
const SYSTEMD_RUNTIME_DIRECTORY: &str = "/run/systemd/system";
#[cfg(feature = "openrc")]
const OPENRC_RUNLEVEL_FILE: &str = "/run/openrc/softlevel";

pub enum DetectedManager {
    #[cfg(feature = "systemd")]
    Systemd(SystemdLink),
    #[cfg(feature = "openrc")]
    Openrc(OpenrcLink),
}

impl DetectedManager {
    pub fn running() -> Result<Self, RunError> {
        #[cfg(feature = "systemd")]
        if Path::new(SYSTEMD_RUNTIME_DIRECTORY).is_dir() {
            return Ok(Self::Systemd(SystemdLink));
        }
        #[cfg(feature = "openrc")]
        if Path::new(OPENRC_RUNLEVEL_FILE).is_file() {
            return Ok(Self::Openrc(OpenrcLink::default()));
        }
        Err(RunError::NoServiceManager)
    }
}
