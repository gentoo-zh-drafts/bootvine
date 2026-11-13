use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    iter,
    os::fd::AsRawFd,
    path::Path,
    rc::Rc,
    time::Duration,
};

use bootvine_screen::screen_components::{
    console_claim::MarkerPresence, passphrase_input::KeyPress, screen_application::Message,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{Stream, StreamExt, stream};
use inotify::{Inotify, WatchMask};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::pread,
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    signal::unix::{SignalKind, signal},
    time::{MissedTickBehavior, interval},
};
use tracing::info;

use crate::console_session::{
    resume_console::{RESUME_CONSOLE_DIRECTORY, RESUME_CONSOLE_MARKER, resume_console_is_open},
    run_error::RunError,
};

pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const FOREGROUND_TERMINAL_STATE: &str = "/sys/class/tty/tty0/active";
const INOTIFY_BUFFER: usize = 4096;

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

fn directory_changes(directory: &str) -> io::Result<impl Stream<Item = ()>> {
    fs::create_dir_all(directory)?;
    let inotify = Inotify::init()?;
    inotify.watches().add(
        directory,
        WatchMask::CLOSE_WRITE
            | WatchMask::MOVED_TO
            | WatchMask::MOVED_FROM
            | WatchMask::DELETE
            | WatchMask::ONLYDIR,
    )?;
    Ok(inotify
        .into_event_stream([0_u8; INOTIFY_BUFFER])?
        .map(|event| {
            event.expect("reading inotify events succeeds");
        }))
}

pub fn resume_marker_inputs() -> impl Stream<Item = Message> {
    let changes = match directory_changes(RESUME_CONSOLE_DIRECTORY) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            info!(
                path = RESUME_CONSOLE_DIRECTORY,
                "not allowed to watch for the resume console"
            );
            return stream::pending().right_stream();
        }
        opened => opened.expect("the resume console directory can be watched"),
    };
    changes
        .map(|()| {
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
    let mut ticks = interval(FRAME_INTERVAL);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    stream::poll_fn(move |context| ticks.poll_tick(context).map(|_| Some(Message::Tick)))
}

fn items_when_readable<Source: AsRawFd, Item>(
    source: Source,
    read: impl Fn(&Source) -> Item,
) -> io::Result<impl Stream<Item = Item>> {
    let readiness = unsafe { AsyncFd::register_with_interest(source, Interest::READABLE) }?;
    Ok(stream::poll_fn(move |context| {
        readiness.poll_read_ready(context).map(|ready| {
            let mut guard = ready.expect("the runtime keeps polling a registered descriptor");
            guard.clear_ready();
            Some(read(readiness.get_ref()))
        })
    }))
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
        Err(error) => vec![Message::Failed(RunError::from(error).into())],
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
