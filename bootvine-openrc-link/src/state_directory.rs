use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use futures::{Stream, StreamExt};
use inotify::{Inotify, WatchMask};
use tracing::{info, warn};

const SYSTEM_RUN_ROOT: &str = "/run/openrc";
const SYSTEM_RUNLEVEL_ROOT: &str = "/etc/runlevels";
const SOFTLEVEL_FILE: &str = "softlevel";
const STARTING_MARKER: &str = "rc.starting";
const STOPPING_MARKER: &str = "rc.stopping";
const DEPENDENCY_CACHE: &str = "deptree";
const INOTIFY_BUFFER: usize = 4096;
const EVENTS_PER_REFRESH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenrcPaths {
    pub(crate) run_root: PathBuf,
    pub(crate) runlevel_root: PathBuf,
}

impl Default for OpenrcPaths {
    fn default() -> Self {
        Self {
            run_root: PathBuf::from(SYSTEM_RUN_ROOT),
            runlevel_root: PathBuf::from(SYSTEM_RUNLEVEL_ROOT),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ServiceState {
    Stopped,
    Inactive,
    Started,
    Starting,
    Stopping,
    Failed,
}

impl ServiceState {
    const ALL: [Self; 5] = [
        Self::Inactive,
        Self::Started,
        Self::Starting,
        Self::Stopping,
        Self::Failed,
    ];

    fn directory_name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Inactive => "inactive",
            Self::Started => "started",
            Self::Starting => "starting",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Runlevel {
    Named(String),
    #[default]
    Unset,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RunlevelChange {
    Starting,
    Stopping,
    #[default]
    Settled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StateSnapshot {
    pub(crate) services: BTreeMap<String, ServiceState>,
    pub(crate) runlevel: Runlevel,
    pub(crate) change: RunlevelChange,
}

impl StateSnapshot {
    pub(crate) fn state_of(&self, service: &str) -> ServiceState {
        self.services
            .get(service)
            .copied()
            .unwrap_or(ServiceState::Stopped)
    }
}

pub(crate) struct ListedService {
    pub(crate) name: String,
    pub(crate) state: ServiceState,
}

fn entry_names(directory: &Path) -> Vec<String> {
    let entries = match fs::read_dir(directory) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        listing => listing.expect("OpenRC's state and runlevel directories are readable"),
    };
    entries
        .map(|entry| entry.expect("the entries of OpenRC's directories are readable"))
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect()
}

fn listed_services(run_root: &Path) -> impl Iterator<Item = ListedService> {
    ServiceState::ALL.into_iter().flat_map(move |state| {
        entry_names(&run_root.join(state.directory_name()))
            .into_iter()
            .map(move |name| ListedService { name, state })
    })
}

fn read_runlevel(run_root: &Path) -> Runlevel {
    match fs::read_to_string(run_root.join(SOFTLEVEL_FILE)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Runlevel::Unset,
        softlevel => Runlevel::Named(
            softlevel
                .expect("OpenRC's softlevel file is readable")
                .trim()
                .to_owned(),
        ),
    }
}

fn read_change(run_root: &Path) -> RunlevelChange {
    if run_root.join(STARTING_MARKER).exists() {
        RunlevelChange::Starting
    } else if run_root.join(STOPPING_MARKER).exists() {
        RunlevelChange::Stopping
    } else {
        RunlevelChange::Settled
    }
}

pub(crate) fn services_by_name(
    listed_services: impl IntoIterator<Item = ListedService>,
) -> BTreeMap<String, ServiceState> {
    listed_services
        .into_iter()
        .fold(BTreeMap::new(), |mut services, listed| {
            services
                .entry(listed.name)
                .and_modify(|known: &mut ServiceState| *known = (*known).max(listed.state))
                .or_insert(listed.state);
            services
        })
}

pub(crate) fn read_snapshot(paths: &OpenrcPaths) -> StateSnapshot {
    StateSnapshot {
        services: services_by_name(listed_services(&paths.run_root)),
        runlevel: read_runlevel(&paths.run_root),
        change: read_change(&paths.run_root),
    }
}

pub(crate) fn read_dependency_cache(paths: &OpenrcPaths) -> String {
    let cache_path = paths.run_root.join(DEPENDENCY_CACHE);
    match fs::read_to_string(&cache_path) {
        Ok(cache) => cache,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info!(path = %cache_path.display(), "OpenRC has no dependency cache yet, drawing services without orderings");
            String::new()
        }
        Err(error) => {
            warn!(path = %cache_path.display(), error = %error, "cannot read the OpenRC dependency cache, drawing services without orderings");
            String::new()
        }
    }
}

pub(crate) fn runlevel_members(paths: &OpenrcPaths, runlevel: &str) -> Vec<String> {
    entry_names(&paths.runlevel_root.join(runlevel))
}

const WATCHED_EVENTS: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::ONLYDIR);

pub(crate) fn state_changes(paths: &OpenrcPaths) -> io::Result<impl Stream<Item = ()> + use<>> {
    let inotify = Inotify::init()?;
    inotify.watches().add(&paths.run_root, WATCHED_EVENTS)?;
    ServiceState::ALL
        .iter()
        .map(|state| paths.run_root.join(state.directory_name()))
        .filter(|directory| directory.is_dir())
        .try_for_each(|directory| inotify.watches().add(directory, WATCHED_EVENTS).map(drop))?;
    Ok(inotify
        .into_event_stream([0_u8; INOTIFY_BUFFER])?
        .ready_chunks(EVENTS_PER_REFRESH)
        .map(drop))
}
