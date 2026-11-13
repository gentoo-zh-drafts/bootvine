#[cfg(not(any(feature = "systemd", feature = "openrc")))]
compile_error!("bootvine needs at least one service manager: enable the systemd or openrc feature");

pub mod config_reading;
pub mod console_session;
pub mod system_install;
