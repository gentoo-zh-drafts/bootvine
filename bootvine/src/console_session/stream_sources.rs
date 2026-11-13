use std::{
    fs::{self, File},
    future::Future,
    io::{self, Read, Seek, SeekFrom},
    iter,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    rc::Rc,
    time::Duration,
};

use crossterm::event::{self, Event};
use futures::{FutureExt, Stream, StreamExt, future::ready, stream};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::inotify::{self, CreateFlags, WatchFlags},
    io::{Errno, pread, read},
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    signal::unix::{SignalKind, signal},
    time::{Interval, MissedTickBehavior, interval},
};
use tracing::info;

use crate::console_session::{resume_console::RESUME_CONSOLE_DIRECTORY, watch_screen::ScreenInput};

pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);
pub const BACKGROUND_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const INOTIFY_BUFFER: usize = 4096;
const FOREGROUND_TERMINAL_STATE: &str = "/sys/class/tty/tty0/active";

pub struct DirectoryWatch {
    inotify: OwnedFd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryChange {
    Changed,
    Unchanged,
}

impl DirectoryWatch {
    pub fn open(directory: &str) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let inotify = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        inotify::add_watch(
            &inotify,
            directory,
            WatchFlags::CLOSE_WRITE
                | WatchFlags::MOVED_TO
                | WatchFlags::MOVED_FROM
                | WatchFlags::DELETE
                | WatchFlags::ONLYDIR,
        )?;
        Ok(Self { inotify })
    }

    #[must_use]
    pub fn changed(&self) -> DirectoryChange {
        let mut events = [0_u8; INOTIFY_BUFFER];
        let drained_reads = iter::from_fn(|| match read(&self.inotify, &mut events) {
            Ok(0) | Err(Errno::AGAIN) => None,
            Ok(read_bytes) => Some(read_bytes),
            Err(error) => panic!("reading inotify events failed: {error}"),
        })
        .count();
        if drained_reads > 0 {
            DirectoryChange::Changed
        } else {
            DirectoryChange::Unchanged
        }
    }
}

impl AsRawFd for DirectoryWatch {
    fn as_raw_fd(&self) -> RawFd {
        self.inotify.as_raw_fd()
    }
}

fn rearm_attribute(attribute: &mut File) -> io::Result<()> {
    attribute.seek(SeekFrom::Start(0))?;
    attribute.read_to_end(&mut Vec::new())?;
    Ok(())
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

pub fn console_overwrite_inputs(watch: Rc<ConsoleContentWatch>) -> impl Stream<Item = ScreenInput> {
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
                return Some((ScreenInput::ConsoleOverwritten, watch));
            }
        }
    })
}

pub fn terminal_switch_inputs() -> impl Stream<Item = ScreenInput> {
    let watched = match watched_foreground_terminal() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!("the kernel has no virtual consoles, not following console switches");
            return stream::pending().right_stream();
        }
        opened => opened.expect("the foreground console attribute is readable"),
    };
    stream::unfold(watched, |mut watched| async move {
        {
            let mut readiness = watched
                .ready_mut(Interest::PRIORITY)
                .await
                .expect("the runtime keeps polling the foreground console attribute");
            readiness.clear_ready();
            rearm_attribute(readiness.get_inner_mut())
                .expect("the foreground console attribute stays readable");
        }
        Some((ScreenInput::ConsoleUpkeep, watched))
    })
    .left_stream()
}

pub fn resume_marker_inputs() -> impl Stream<Item = ScreenInput> {
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
        .map(|_| ScreenInput::ConsoleUpkeep)
        .left_stream()
}

pub struct CarriedItem<Item, State> {
    pub item: Item,
    pub state: State,
}

pub fn unfold_carrying<State, Item, Next, Pending>(
    initial: State,
    mut next: Next,
) -> impl Stream<Item = Item>
where
    Next: FnMut(State) -> Pending,
    Pending: Future<Output = Option<CarriedItem<Item, State>>>,
{
    stream::unfold(initial, move |state| {
        next(state).map(|carried| carried.map(|carried| (carried.item, carried.state)))
    })
}

pub fn readable_source<Source: AsRawFd>(source: Source) -> io::Result<AsyncFd<Source>> {
    Ok(unsafe { AsyncFd::register_with_interest(source, Interest::READABLE) }?)
}

pub fn items_when_readable<Source: AsRawFd, Item>(
    source: Source,
    read: impl Fn(&Source) -> Item,
) -> io::Result<impl Stream<Item = Item>> {
    let readiness = readable_source(source)?;
    Ok(stream::poll_fn(move |context| {
        readiness.poll_read_ready(context).map(|ready| {
            let mut guard = ready.expect("the runtime keeps polling a registered descriptor");
            guard.clear_ready();
            Some(read(readiness.get_ref()))
        })
    }))
}

fn steady_interval(period: Duration) -> Interval {
    let mut ticks = interval(period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticks
}

pub fn periodic_inputs(
    period: Duration,
    input: fn() -> ScreenInput,
) -> impl Stream<Item = ScreenInput> {
    let mut ticks = steady_interval(period);
    stream::poll_fn(move |context| ticks.poll_tick(context).map(|_| Some(input())))
}

pub fn frame_inputs() -> impl Stream<Item = ScreenInput> {
    periodic_inputs(FRAME_INTERVAL, || ScreenInput::Frame)
}

fn drain_terminal_events() -> io::Result<Vec<Event>> {
    iter::from_fn(|| match event::poll(Duration::ZERO) {
        Ok(true) => Some(event::read()),
        Ok(false) => None,
        Err(error) => Some(Err(error)),
    })
    .collect()
}

fn terminal_inputs(drained: io::Result<Vec<Event>>) -> Vec<ScreenInput> {
    match drained {
        Ok(events) => events.into_iter().map(ScreenInput::Key).collect(),
        Err(error) => vec![ScreenInput::Failed(error.into())],
    }
}

pub fn key_inputs() -> io::Result<impl Stream<Item = ScreenInput>> {
    Ok(
        items_when_readable(io::stdin(), |_| drain_terminal_events())?
            .map(terminal_inputs)
            .flat_map(stream::iter),
    )
}

pub fn termination_inputs() -> io::Result<impl Stream<Item = ScreenInput>> {
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(stream::once(async move {
        terminate.recv().await;
        ScreenInput::Terminate
    }))
}
