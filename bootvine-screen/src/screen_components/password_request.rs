use std::{path::PathBuf, time::Duration};

use crate::screen_components::prompt::Passphrase;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PasswordRequest {
    pub file: PathBuf,
    pub message: String,
    pub socket: PathBuf,
    pub echo: bool,
    pub silent: bool,
    pub not_after: PromptDeadline,
    pub asker: PromptAsker,
    pub hint: RequestHint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordAnswer {
    pub socket: PathBuf,
    pub passphrase: Passphrase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptDeadline {
    Never,
    At(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptAsker {
    Process(i32),
    Unnamed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RequestHint {
    Shown(String),
    Hidden,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn cryptsetup_request(file: &str) -> PasswordRequest {
        PasswordRequest {
            file: PathBuf::from(file),
            message: "Please enter passphrase for disk secret:".to_owned(),
            socket: PathBuf::from("/run/systemd/ask-password/sck.1"),
            echo: false,
            silent: false,
            not_after: PromptDeadline::Never,
            asker: PromptAsker::Process(1),
            hint: RequestHint::Hidden,
        }
    }
}
