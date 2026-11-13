use std::{fs, io, path::Path};

use bootvine_screen::screen_settings::config_file::Config;
use toml::de::Error as ParseError;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read: {0}")]
    Unreadable(#[from] io::Error),
    #[error("invalid: {0}")]
    Invalid(#[from] ParseError),
}

pub fn read_config_file(path: &Path) -> Result<Config, ConfigError> {
    Ok(fs::read_to_string(path)?.parse()?)
}
