use std::fs;

use bootvine_systemd_link::link_error::Error;

use crate::console_session::{
    console_takeover::{KERNEL_PRINTK, console_level},
    resume_console::RESUME_CONSOLE_DIRECTORY,
    service_manager::{ServiceManager, SystemdManager},
};

const SAVED_KERNEL_CONSOLE_LEVEL: &str = "/run/bootvine/kernel-console-level";

pub fn save_console_state() -> Result<(), Error> {
    fs::create_dir_all(RESUME_CONSOLE_DIRECTORY)?;
    fs::write(SAVED_KERNEL_CONSOLE_LEVEL, console_level()?)?;
    Ok(())
}

pub fn restore_console_state() -> Result<(), Error> {
    fs::write(
        KERNEL_PRINTK,
        fs::read_to_string(SAVED_KERNEL_CONSOLE_LEVEL)?,
    )?;
    SystemdManager.restore_configured_status()?;
    Ok(())
}
