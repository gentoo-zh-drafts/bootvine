use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use tracing::warn;

const ASK_SECTION: &str = "[Ask]";

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

#[derive(Debug, PartialEq, Eq)]
pub enum RequestFile {
    Pending(PasswordRequest),
    Withdrawn,
    Forbidden,
    Malformed,
}

impl PasswordRequest {
    #[must_use]
    pub fn parse(file: &Path, text: &str) -> RequestFile {
        let fields: Vec<(&str, &str)> = text
            .lines()
            .map(str::trim)
            .skip_while(|line| *line != ASK_SECTION)
            .skip(1)
            .take_while(|line| !line.starts_with('['))
            .filter_map(|line| line.split_once('='))
            .collect();
        let field = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| *value)
        };
        let flag = |key: &str| field(key).is_some_and(is_true);
        let Some(socket) = field("Socket") else {
            warn!(path = %file.display(), "password request names no socket");
            return RequestFile::Malformed;
        };
        let not_after = match field("NotAfter").map(str::parse) {
            None | Some(Ok(0)) => PromptDeadline::Never,
            Some(Ok(micros)) => PromptDeadline::At(Duration::from_micros(micros)),
            Some(Err(error)) => {
                warn!(path = %file.display(), error = %error, "password request has an unreadable NotAfter");
                return RequestFile::Malformed;
            }
        };
        let asker = match field("PID").map(str::parse::<i32>) {
            None => PromptAsker::Unnamed,
            Some(Ok(raw)) if raw > 0 => PromptAsker::Process(raw),
            Some(Ok(raw)) => {
                warn!(path = %file.display(), pid = raw, "password request names an impossible PID");
                return RequestFile::Malformed;
            }
            Some(Err(error)) => {
                warn!(path = %file.display(), error = %error, "password request has an unreadable PID");
                return RequestFile::Malformed;
            }
        };
        RequestFile::Pending(Self {
            file: file.to_owned(),
            message: field("Message").unwrap_or_default().to_owned(),
            socket: PathBuf::from(socket),
            echo: flag("Echo"),
            silent: flag("Silent"),
            not_after,
            asker,
            hint: RequestHint::Hidden,
        })
    }
}

fn is_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "yes" | "y" | "true" | "t" | "on"
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const CRYPTSETUP_ASK: &str = concat!(
        "[Ask]\n",
        "PID=1\n",
        "Socket=/run/systemd/ask-password/sck.1\n",
        "AcceptCached=1\n",
        "Echo=0\n",
        "NotAfter=0\n",
        "Message=Please enter passphrase for disk secret:\n",
        "Icon=drive-harddisk\n",
        "Id=cryptsetup:/dev/vdb\n",
    );

    pub(crate) fn cryptsetup_request(file: &str) -> PasswordRequest {
        match PasswordRequest::parse(Path::new(file), CRYPTSETUP_ASK) {
            RequestFile::Pending(request) => request,
            unexpected => panic!("cryptsetup's request parses, got {unexpected:?}"),
        }
    }

    #[test]
    fn reads_what_cryptsetup_asks() {
        assert_eq!(
            PasswordRequest::parse(Path::new("ask.x"), CRYPTSETUP_ASK),
            RequestFile::Pending(PasswordRequest {
                file: PathBuf::from("ask.x"),
                message: "Please enter passphrase for disk secret:".to_owned(),
                socket: PathBuf::from("/run/systemd/ask-password/sck.1"),
                echo: false,
                silent: false,
                not_after: PromptDeadline::Never,
                asker: PromptAsker::Process(1),
                hint: RequestHint::Hidden,
            })
        );
    }

    #[test]
    fn ignores_requests_without_a_socket() {
        assert_eq!(
            PasswordRequest::parse(Path::new("ask.x"), "[Ask]\nMessage=hi\n"),
            RequestFile::Malformed
        );
    }

    #[test]
    fn a_nonzero_not_after_is_a_deadline() {
        let RequestFile::Pending(request) = PasswordRequest::parse(
            Path::new("ask.x"),
            "[Ask]\nSocket=/run/sck\nNotAfter=5000000\n",
        ) else {
            panic!("a request with a socket parses");
        };
        assert_eq!(
            request.not_after,
            PromptDeadline::At(Duration::from_secs(5))
        );
        assert_eq!(request.asker, PromptAsker::Unnamed);
    }

    #[test]
    fn rejects_requests_with_an_unreadable_pid() {
        assert_eq!(
            PasswordRequest::parse(Path::new("ask.x"), "[Ask]\nSocket=/run/sck\nPID=one\n"),
            RequestFile::Malformed
        );
        assert_eq!(
            PasswordRequest::parse(Path::new("ask.x"), "[Ask]\nSocket=/run/sck\nPID=0\n"),
            RequestFile::Malformed
        );
    }
}
