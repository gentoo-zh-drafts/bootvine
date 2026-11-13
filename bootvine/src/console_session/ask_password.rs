use std::{
    fs,
    future::{Future, pending},
    io,
    os::{fd::OwnedFd, unix::net::UnixDatagram},
    path::{Path, PathBuf},
    pin::pin,
};

use futures::future::select;
use rustix::{
    io::Errno,
    process::{Pid, PidfdFlags, pidfd_open, test_kill_process},
};
use tokio::{io::unix::AsyncFd, time::sleep};
use zeroize::Zeroizing;

use crate::{
    console_session::stream_sources::{DirectoryWatch, readable_source},
    screen_components::{
        password_request::{PasswordRequest, PromptAsker, PromptDeadline, RequestFile},
        prompt::Passphrase,
    },
    systemd_link::monotonic_clock::monotonic_now,
};

const REQUESTS: &str = "/run/systemd/ask-password";
const REQUEST_PREFIX: &str = "ask.";

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

pub fn watch_requests() -> io::Result<DirectoryWatch> {
    DirectoryWatch::open(REQUESTS)
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
        contents => Ok(PasswordRequest::parse(file, &contents?)),
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
