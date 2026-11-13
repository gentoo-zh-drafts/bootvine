use std::{
    fs,
    future::{Future, pending},
    io,
    os::{fd::OwnedFd, unix::net::UnixDatagram},
    path::{Path, PathBuf},
    pin::pin,
    time::Duration,
};

use crossterm::event::Event;
use futures::future::select;
use rustix::{
    io::Errno,
    process::{Pid, PidfdFlags, pidfd_open, test_kill_process},
};
use tokio::{io::unix::AsyncFd, time::sleep};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::{
    boot_view::PasswordPrompt,
    console_session::{
        passphrase_input,
        stream_sources::{DirectoryWatch, readable_source},
    },
    systemd_link::monotonic_clock::monotonic_now,
};

const REQUESTS: &str = "/run/systemd/ask-password";
const REQUEST_PREFIX: &str = "ask.";
const ASK_SECTION: &str = "[Ask]";

pub fn watch_requests() -> io::Result<DirectoryWatch> {
    DirectoryWatch::open(REQUESTS)
}

#[derive(Debug, PartialEq, Eq)]
pub struct PasswordRequest {
    pub file: PathBuf,
    pub message: String,
    socket: PathBuf,
    echo: bool,
    silent: bool,
    not_after: PromptDeadline,
    asker: PromptAsker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptDeadline {
    Never,
    At(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptAsker {
    Process(Pid),
    Unnamed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptStatus {
    Awaited,
    Abandoned,
}

#[derive(Debug, PartialEq, Eq)]
enum RequestFile {
    Pending(PasswordRequest),
    Withdrawn,
    Forbidden,
    Malformed,
}

enum AskerWatch {
    Watching(AsyncFd<OwnedFd>),
    AlreadyGone,
    NoAsker,
}

#[must_use]
pub fn requested() -> bool {
    !request_files().is_empty()
}

fn request_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(REQUESTS)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(REQUEST_PREFIX))
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    files
}

impl PasswordRequest {
    fn read(file: &Path) -> io::Result<RequestFile> {
        match fs::read_to_string(file) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RequestFile::Withdrawn),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                Ok(RequestFile::Forbidden)
            }
            contents => Ok(Self::parse(file, &contents?)),
        }
    }

    fn parse(file: &Path, text: &str) -> RequestFile {
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
        let asker = match field("PID").map(str::parse) {
            None => PromptAsker::Unnamed,
            Some(Ok(raw)) if let Some(pid) = Pid::from_raw(raw) => PromptAsker::Process(pid),
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
        })
    }

    fn status(&self) -> PromptStatus {
        if let PromptDeadline::At(deadline) = self.not_after
            && monotonic_now() > deadline
        {
            return PromptStatus::Abandoned;
        }
        let PromptAsker::Process(asker) = self.asker else {
            return PromptStatus::Awaited;
        };
        match test_kill_process(asker) {
            Err(Errno::SRCH) => PromptStatus::Abandoned,
            Ok(()) | Err(Errno::PERM) => PromptStatus::Awaited,
            Err(error) => panic!("probing the process that asked for a password failed: {error}"),
        }
    }

    fn until_abandoned(&self) -> impl Future<Output = ()> + Send + 'static {
        #[expect(
            clippy::disallowed_methods,
            reason = "a password request past its deadline has no time left"
        )]
        let remaining = match self.not_after {
            PromptDeadline::At(deadline) => {
                PromptDeadline::At(deadline.saturating_sub(monotonic_now()))
            }
            PromptDeadline::Never => PromptDeadline::Never,
        };
        let asker_watch = match self.asker {
            PromptAsker::Unnamed => AskerWatch::NoAsker,
            PromptAsker::Process(asker) => match pidfd_open(asker, PidfdFlags::NONBLOCK) {
                Err(Errno::SRCH) => AskerWatch::AlreadyGone,
                opened => AskerWatch::Watching(
                    readable_source(opened.expect("a pidfd opens for any live process"))
                        .expect("the runtime accepts a pidfd"),
                ),
            },
        };
        async move {
            let deadline_passed = async move {
                match remaining {
                    PromptDeadline::At(remaining) => sleep(remaining).await,
                    PromptDeadline::Never => pending::<()>().await,
                }
            };
            let asker_exited = async move {
                match asker_watch {
                    AskerWatch::Watching(pidfd) => {
                        drop(
                            pidfd
                                .readable()
                                .await
                                .expect("the runtime keeps polling a registered pidfd"),
                        );
                    }
                    AskerWatch::AlreadyGone => {}
                    AskerWatch::NoAsker => pending::<()>().await,
                }
            };
            select(pin!(deadline_passed), pin!(asker_exited)).await;
        }
    }

    fn answer(&self, password: &str) -> io::Result<()> {
        let mut packet = Zeroizing::new(Vec::with_capacity(password.len() + 1));
        packet.push(b'+');
        packet.extend_from_slice(password.as_bytes());
        UnixDatagram::unbound()?.send_to(&packet, &self.socket)?;
        Ok(())
    }
}

fn is_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "yes" | "y" | "true" | "t" | "on"
    )
}

pub struct PasswordEntry {
    request: PasswordRequest,
    typed: Zeroizing<String>,
    sent: bool,
}

impl PasswordEntry {
    #[must_use]
    pub fn request_file(&self) -> &Path {
        &self.request.file
    }

    pub fn until_abandoned(&self) -> impl Future<Output = ()> + Send + 'static {
        self.request.until_abandoned()
    }

    pub fn follow(current: Option<Self>, requests_changed: bool) -> Option<Self> {
        match current {
            Some(entry) if entry.is_live(requests_changed) => return Some(entry),
            None if !requests_changed => return None,
            _ => {}
        }
        let entry = request_files()
            .iter()
            .filter_map(|file| {
                match PasswordRequest::read(file).expect("password request files are readable") {
                    RequestFile::Pending(request) => Some(request),
                    RequestFile::Withdrawn | RequestFile::Forbidden | RequestFile::Malformed => {
                        None
                    }
                }
            })
            .find_map(Self::open)?;
        info!(path = %entry.request.file.display(), "asking for a password");
        Some(entry)
    }

    fn open(request: PasswordRequest) -> Option<Self> {
        if request.status() == PromptStatus::Abandoned {
            return None;
        }
        Some(Self {
            request,
            typed: passphrase_input::empty(),
            sent: false,
        })
    }

    fn is_live(&self, requests_changed: bool) -> bool {
        self.request.status() == PromptStatus::Awaited
            && (!requests_changed || self.request.file.exists())
    }

    #[must_use]
    pub fn prompt(&self) -> PasswordPrompt<'_> {
        let input = if self.request.silent {
            String::new()
        } else if self.request.echo {
            self.typed.to_string()
        } else {
            passphrase_input::masked(&self.typed)
        };
        PasswordPrompt {
            message: &self.request.message,
            hint: None,
            input,
            sent: self.sent,
            rejected: false,
        }
    }

    pub fn type_key(&mut self, event: &Event) -> io::Result<()> {
        if self.sent {
            return Ok(());
        }
        let Some(submitted) = passphrase_input::type_key(&mut self.typed, event) else {
            return Ok(());
        };
        self.request.answer(&submitted)?;
        self.sent = true;
        info!(path = %self.request.file.display(), "answered a password prompt");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};

    use super::*;

    const CRYPTSETUP_ASK: &str = concat!(
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
                asker: PromptAsker::Process(Pid::from_raw(1).expect("pid 1 is valid")),
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
    }

    #[test]
    fn hides_what_is_typed() {
        let mut entry = PasswordEntry {
            request: match PasswordRequest::parse(Path::new("ask.x"), CRYPTSETUP_ASK) {
                RequestFile::Pending(request) => request,
                unexpected => panic!("cryptsetup's request parses, got {unexpected:?}"),
            },
            typed: passphrase_input::empty(),
            sent: false,
        };
        "secret".chars().for_each(|character| {
            entry
                .type_key(&Event::Key(KeyEvent::from(KeyCode::Char(character))))
                .expect("typed");
        });
        entry
            .type_key(&Event::Key(KeyEvent::from(KeyCode::Backspace)))
            .expect("erased");
        assert_eq!(entry.prompt().input, "*****");
    }
}
