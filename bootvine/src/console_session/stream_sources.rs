use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    iter,
    path::Path,
    rc::Rc,
    time::Duration,
};

use bootvine_screen::screen_components::{
    console_claim::MarkerPresence, passphrase_input::KeyPress, screen_application::Message,
};
use bootvine_systemd_link::{
    descriptor_streams::{DirectoryChange, DirectoryWatch, items_when_readable, periodic_inputs},
    link_error::Error,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{Stream, StreamExt, future::ready, stream};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::pread,
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    signal::unix::{SignalKind, signal},
};
use tracing::info;

use crate::console_session::resume_console::{
    RESUME_CONSOLE_DIRECTORY, RESUME_CONSOLE_MARKER, resume_console_is_open,
};

pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const FOREGROUND_TERMINAL_STATE: &str = "/sys/class/tty/tty0/active";

fn rearm_attribute(attribute: &mut File) -> io::Result<String> {
    attribute.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    attribute.read_to_string(&mut contents)?;
    Ok(contents)
}

fn active_console_number(attribute: &str) -> u16 {
    attribute
        .trim()
        .strip_prefix("tty")
        .expect("the foreground console attribute names a ttyN device")
        .parse()
        .expect("virtual console numbers fit in sixteen bits")
}

fn watched_foreground_terminal() -> io::Result<AsyncFd<File>> {
    let mut attribute = File::open(FOREGROUND_TERMINAL_STATE)?;
    rearm_attribute(&mut attribute)?;
    Ok(unsafe { AsyncFd::register_with_interest(attribute, Interest::PRIORITY) }?)
}

pub struct ConsoleContentWatch {
    contents: AsyncFd<File>,
}

impl ConsoleContentWatch {
    pub fn open(console_number: u16) -> io::Result<Rc<Self>> {
        let contents = File::open(format!("/dev/vcs{console_number}"))?;
        let watch = Self {
            contents: unsafe { AsyncFd::register_with_interest(contents, Interest::PRIORITY) }?,
        };
        watch.forget_own_writes();
        Ok(Rc::new(watch))
    }

    pub fn forget_own_writes(&self) {
        pread(self.contents.get_ref(), &mut [0_u8; 1], 0)
            .expect("the contents of bootvine's console stay readable");
    }

    fn overwritten_since_forgotten(&self) -> bool {
        let mut watched = [PollFd::new(self.contents.get_ref(), PollFlags::PRI)];
        poll(&mut watched, Some(&Timespec::default()))
            .expect("polling the contents of bootvine's console succeeds");
        watched[0].revents().contains(PollFlags::PRI)
    }
}

pub fn console_overwrite_inputs(watch: Rc<ConsoleContentWatch>) -> impl Stream<Item = Message> {
    stream::unfold(watch, |watch| async move {
        loop {
            let mut readiness = watch
                .contents
                .ready(Interest::PRIORITY)
                .await
                .expect("the runtime keeps polling the contents of bootvine's console");
            readiness.clear_ready();
            drop(readiness);
            if watch.overwritten_since_forgotten() {
                return Some((Message::ConsoleOverwritten, watch));
            }
        }
    })
}

pub fn terminal_switch_inputs() -> impl Stream<Item = Message> {
    let watched = match watched_foreground_terminal() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!("the kernel has no virtual consoles, not following console switches");
            return stream::pending().right_stream();
        }
        opened => opened.expect("the foreground console attribute is readable"),
    };
    stream::unfold(watched, |mut watched| async move {
        let attribute = {
            let mut readiness = watched
                .ready_mut(Interest::PRIORITY)
                .await
                .expect("the runtime keeps polling the foreground console attribute");
            readiness.clear_ready();
            rearm_attribute(readiness.get_inner_mut())
                .expect("the foreground console attribute stays readable")
        };
        Some((
            Message::TerminalActivated(active_console_number(&attribute)),
            watched,
        ))
    })
    .left_stream()
}

pub fn resume_marker_inputs() -> impl Stream<Item = Message> {
    let watch = match DirectoryWatch::open(RESUME_CONSOLE_DIRECTORY) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            info!(
                path = RESUME_CONSOLE_DIRECTORY,
                "not allowed to watch for the resume console"
            );
            return stream::pending().right_stream();
        }
        opened => opened.expect("the resume console directory can be watched"),
    };
    items_when_readable(watch, DirectoryWatch::changed)
        .expect("the runtime accepts an inotify descriptor")
        .filter(|change| ready(*change == DirectoryChange::Changed))
        .map(|_| {
            Message::ResumeMarker(
                if resume_console_is_open(Path::new(RESUME_CONSOLE_MARKER)) {
                    MarkerPresence::Present
                } else {
                    MarkerPresence::Absent
                },
            )
        })
        .left_stream()
}

pub fn frame_inputs() -> impl Stream<Item = Message> {
    periodic_inputs(FRAME_INTERVAL, || Message::Tick)
}

fn drain_terminal_events() -> io::Result<Vec<Event>> {
    iter::from_fn(|| match event::poll(Duration::ZERO) {
        Ok(true) => Some(event::read()),
        Ok(false) => None,
        Err(error) => Some(Err(error)),
    })
    .collect()
}

fn key_press_of(event: &Event) -> Option<KeyPress> {
    let Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        ..
    }) = event
    else {
        return None;
    };
    let control = modifiers.contains(KeyModifiers::CONTROL);
    Some(match *code {
        KeyCode::Enter => KeyPress::Enter,
        KeyCode::Backspace => KeyPress::Backspace,
        KeyCode::Esc => KeyPress::Escape,
        KeyCode::Char('u') if control => KeyPress::ClearLine,
        KeyCode::Char('c') if control => KeyPress::Interrupt,
        KeyCode::Char(_) if control => KeyPress::Other,
        KeyCode::Char(character) => KeyPress::Character(character),
        _ => KeyPress::Other,
    })
}

fn terminal_inputs(drained: io::Result<Vec<Event>>) -> Vec<Message> {
    match drained {
        Ok(events) => events
            .iter()
            .filter_map(key_press_of)
            .map(Message::KeyPressed)
            .collect(),
        Err(error) => vec![Message::Failed(Error::from(error).into())],
    }
}

pub fn key_inputs() -> io::Result<impl Stream<Item = Message>> {
    Ok(
        items_when_readable(io::stdin(), |_| drain_terminal_events())?
            .map(terminal_inputs)
            .flat_map(stream::iter),
    )
}

pub fn termination_inputs() -> io::Result<impl Stream<Item = Message>> {
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(stream::once(async move {
        terminate.recv().await;
        Message::Terminated
    }))
}
