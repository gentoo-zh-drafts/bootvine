use std::{
    fs,
    future::Future,
    io, iter,
    os::fd::{AsRawFd, OwnedFd, RawFd},
    time::Duration,
};

use futures::{FutureExt, Stream, stream};
use rustix::{
    fs::inotify::{self, CreateFlags, WatchFlags},
    io::{Errno, read},
};
use tokio::{
    io::{Interest, unix::AsyncFd},
    time::{Interval, MissedTickBehavior, interval},
};

pub(crate) const BACKGROUND_CHECK_INTERVAL: Duration = Duration::from_millis(100);
const INOTIFY_BUFFER: usize = 4096;

pub(crate) struct DirectoryWatch {
    inotify: OwnedFd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectoryChange {
    Changed,
    Unchanged,
}

impl DirectoryWatch {
    pub(crate) fn open(directory: &str) -> io::Result<Self> {
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
    pub(crate) fn changed(&self) -> DirectoryChange {
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

pub(crate) struct CarriedItem<Item, State> {
    pub(crate) item: Item,
    pub(crate) state: State,
}

pub(crate) fn unfold_carrying<State, Item, Next, Pending>(
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

pub(crate) fn readable_source<Source: AsRawFd>(source: Source) -> io::Result<AsyncFd<Source>> {
    Ok(unsafe { AsyncFd::register_with_interest(source, Interest::READABLE) }?)
}

pub(crate) fn items_when_readable<Source: AsRawFd, Item>(
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

pub(crate) fn periodic_inputs<Item>(
    period: Duration,
    mut input: impl FnMut() -> Item,
) -> impl Stream<Item = Item> {
    let mut ticks = steady_interval(period);
    stream::poll_fn(move |context| ticks.poll_tick(context).map(|_| Some(input())))
}
