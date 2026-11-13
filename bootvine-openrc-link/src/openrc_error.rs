use std::io;

use bootvine_screen::screen_components::handover::WatchFailure;

#[derive(Debug, thiserror::Error)]
pub enum OpenrcError {
    #[error("watching the OpenRC state directory: {0}")]
    Watch(#[from] io::Error),
}

impl From<OpenrcError> for WatchFailure {
    fn from(error: OpenrcError) -> Self {
        Self {
            reason: error.to_string(),
        }
    }
}
