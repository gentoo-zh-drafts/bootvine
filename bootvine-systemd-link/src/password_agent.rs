use std::{
    fs,
    future::{Future, pending},
    io,
    os::{fd::OwnedFd, unix::net::UnixDatagram},
    path::{Path, PathBuf},
    pin::pin,
    time::Duration,
};

use bootvine_screen::screen_components::{
    password_request::{PasswordRequest, PromptAsker, PromptDeadline, RequestHint},
    prompt::Passphrase,
    screen_application::Message,
};
use futures::{
    StreamExt,
    future::{ready, select},
    stream,
    stream::LocalBoxStream,
};
use rustix::{
    io::Errno,
    process::{Pid, PidfdFlags, pidfd_open, test_kill_process},
};
use tokio::{io::unix::AsyncFd, time::sleep};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::{
    descriptor_streams::{
        BACKGROUND_CHECK_INTERVAL, DirectoryChange, DirectoryWatch, items_when_readable,
        periodic_inputs, readable_source,
    },
    monotonic_clock::monotonic_now,
};

const REQUESTS: &str = "/run/systemd/ask-password";
const REQUEST_PREFIX: &str = "ask.";
const ASK_SECTION: &str = "[Ask]";

#[derive(Debug, PartialEq, Eq)]
pub enum RequestFile {
    Pending(PasswordRequest),
    Withdrawn,
    Forbidden,
    Malformed,
}

#[must_use]
pub fn parse_request(file: &Path, text: &str) -> RequestFile {
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
    RequestFile::Pending(PasswordRequest {
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

fn is_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "yes" | "y" | "true" | "t" | "on"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptStatus {
    Awaited,
    Abandoned,
}

enum AskerWatch {
    Watching(AsyncFd<OwnedFd>),
    AlreadyGone,
    NoAsker,
}

fn list_awaited_requests() -> Message {
    Message::PasswordRequestsListed(awaited_requests())
}

pub fn password_request_inputs() -> LocalBoxStream<'static, Message> {
    let watch = match DirectoryWatch::open(REQUESTS) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            info!("not allowed to watch password prompts, polling for them instead");
            return periodic_inputs(BACKGROUND_CHECK_INTERVAL, list_awaited_requests).boxed_local();
        }
        watched => watched.expect("the password request directory can be watched"),
    };
    let changes = items_when_readable(watch, DirectoryWatch::changed)
        .expect("the runtime accepts an inotify descriptor")
        .filter(|change| ready(*change == DirectoryChange::Changed))
        .map(|_| list_awaited_requests());
    stream::once(ready(list_awaited_requests()))
        .chain(changes)
        .boxed_local()
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

fn read_request(file: &Path) -> io::Result<RequestFile> {
    match fs::read_to_string(file) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RequestFile::Withdrawn),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(RequestFile::Forbidden),
        contents => Ok(parse_request(file, &contents?)),
    }
}

fn asker_process(asker: PromptAsker) -> Option<Pid> {
    match asker {
        PromptAsker::Process(raw) => Some(Pid::from_raw(raw).expect("parsed PIDs are positive")),
        PromptAsker::Unnamed => None,
    }
}

fn status(request: &PasswordRequest) -> PromptStatus {
    if let PromptDeadline::At(deadline) = request.not_after
        && monotonic_now() > deadline
    {
        return PromptStatus::Abandoned;
    }
    let Some(asker) = asker_process(request.asker) else {
        return PromptStatus::Awaited;
    };
    match test_kill_process(asker) {
        Err(Errno::SRCH) => PromptStatus::Abandoned,
        Ok(()) | Err(Errno::PERM) => PromptStatus::Awaited,
        Err(error) => panic!("probing the process that asked for a password failed: {error}"),
    }
}

#[must_use]
pub fn awaited_requests() -> Vec<PasswordRequest> {
    request_files()
        .iter()
        .filter_map(
            |file| match read_request(file).expect("password request files are readable") {
                RequestFile::Pending(request) => Some(request),
                RequestFile::Withdrawn | RequestFile::Forbidden | RequestFile::Malformed => None,
            },
        )
        .filter(|request| status(request) == PromptStatus::Awaited)
        .collect()
}

pub fn until_abandoned(
    deadline: PromptDeadline,
    asker: PromptAsker,
) -> impl Future<Output = ()> + 'static {
    #[expect(
        clippy::disallowed_methods,
        reason = "a password request past its deadline has no time left"
    )]
    let remaining = match deadline {
        PromptDeadline::At(deadline) => {
            PromptDeadline::At(deadline.saturating_sub(monotonic_now()))
        }
        PromptDeadline::Never => PromptDeadline::Never,
    };
    let asker_watch = match asker_process(asker) {
        None => AskerWatch::NoAsker,
        Some(asker) => match pidfd_open(asker, PidfdFlags::NONBLOCK) {
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

pub fn send_answer(socket: &Path, passphrase: &Passphrase) -> io::Result<()> {
    let typed = passphrase.expose();
    let mut packet = Zeroizing::new(Vec::with_capacity(typed.len() + 1));
    packet.push(b'+');
    packet.extend_from_slice(typed.as_bytes());
    UnixDatagram::unbound()?.send_to(&packet, socket)?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
            parse_request(Path::new("ask.x"), CRYPTSETUP_ASK),
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
            parse_request(Path::new("ask.x"), "[Ask]\nMessage=hi\n"),
            RequestFile::Malformed
        );
    }

    #[test]
    fn a_nonzero_not_after_is_a_deadline() {
        let RequestFile::Pending(request) = parse_request(
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
            parse_request(Path::new("ask.x"), "[Ask]\nSocket=/run/sck\nPID=one\n"),
            RequestFile::Malformed
        );
        assert_eq!(
            parse_request(Path::new("ask.x"), "[Ask]\nSocket=/run/sck\nPID=0\n"),
            RequestFile::Malformed
        );
    }
}
