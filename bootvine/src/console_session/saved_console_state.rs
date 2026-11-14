use std::{
    env::{self, VarError},
    fs, io,
};

use bootvine_screen::service_manager::ServiceManager;
use clap::ValueEnum;

use crate::console_session::{
    console_takeover::{KERNEL_PRINTK, bring_own_terminal_forward, console_level},
    resume_console::RESUME_CONSOLE_DIRECTORY,
    run_error::RunError,
    service_manager::DetectedManager,
};

const SAVED_KERNEL_CONSOLE_LEVEL: &str = "/run/bootvine/kernel-console-level";
const SERVICE_RESULT_VARIABLE: &str = "SERVICE_RESULT";
const CLEAN_SERVICE_RESULT: &str = "success";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum StatusOutputRestore {
    Always,
    UnlessCleanExit,
}

pub fn save_console_state() -> Result<(), RunError> {
    fs::create_dir_all(RESUME_CONSOLE_DIRECTORY)?;
    fs::write(SAVED_KERNEL_CONSOLE_LEVEL, console_level()?)?;
    Ok(())
}

pub fn take_shutdown_console() -> Result<(), RunError> {
    save_console_state()?;
    bring_own_terminal_forward().map_err(io::Error::from)?;
    match DetectedManager::running()? {
        #[cfg(feature = "systemd")]
        DetectedManager::Systemd(service_manager) => service_manager.silence_status_output()?,
        #[cfg(feature = "openrc")]
        DetectedManager::Openrc(service_manager) => service_manager.silence_status_output()?,
    }
    Ok(())
}

pub fn restore_console_state(status_output_restore: StatusOutputRestore) -> Result<(), RunError> {
    fs::write(
        KERNEL_PRINTK,
        fs::read_to_string(SAVED_KERNEL_CONSOLE_LEVEL)?,
    )?;
    match (status_output_restore, env::var(SERVICE_RESULT_VARIABLE)) {
        (StatusOutputRestore::UnlessCleanExit, Ok(service_result))
            if service_result == CLEAN_SERVICE_RESULT =>
        {
            return Ok(());
        }
        (_, Err(unreadable @ VarError::NotUnicode(_))) => return Err(unreadable.into()),
        (StatusOutputRestore::Always, _)
        | (StatusOutputRestore::UnlessCleanExit, Ok(_) | Err(VarError::NotPresent)) => {}
    }
    match DetectedManager::running()? {
        #[cfg(feature = "systemd")]
        DetectedManager::Systemd(service_manager) => service_manager.restore_status_output()?,
        #[cfg(feature = "openrc")]
        DetectedManager::Openrc(service_manager) => service_manager.restore_status_output()?,
    }
    Ok(())
}
