use std::{fs, io};

use bootvine_screen::service_manager::ServiceManager;

use crate::console_session::{
    console_takeover::{KERNEL_PRINTK, bring_own_terminal_forward, console_level},
    resume_console::RESUME_CONSOLE_DIRECTORY,
    run_error::RunError,
    service_manager::DetectedManager,
};

const SAVED_KERNEL_CONSOLE_LEVEL: &str = "/run/bootvine/kernel-console-level";

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

pub fn restore_console_state() -> Result<(), RunError> {
    fs::write(
        KERNEL_PRINTK,
        fs::read_to_string(SAVED_KERNEL_CONSOLE_LEVEL)?,
    )?;
    match DetectedManager::running()? {
        #[cfg(feature = "systemd")]
        DetectedManager::Systemd(service_manager) => service_manager.restore_status_output()?,
        #[cfg(feature = "openrc")]
        DetectedManager::Openrc(service_manager) => service_manager.restore_status_output()?,
    }
    Ok(())
}
