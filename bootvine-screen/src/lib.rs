#![forbid(unsafe_code)]
#![deny(unreachable_pub)]
#![deny(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::dbg_macro,
    clippy::exit
)]

pub mod boot_model;
#[cfg(test)]
mod boot_test_fixtures;
pub mod boot_view;
pub mod screen_components;
pub mod screen_settings;
pub mod service_manager;
