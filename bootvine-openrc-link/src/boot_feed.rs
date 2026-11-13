use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::ready,
    time::Duration,
};

use bootvine_screen::{
    boot_model::{
        ActiveState, Activity, BootChange, JobKind, JobResult, Phase, UnitFacts, UnitKind,
        UnitTimeouts,
    },
    screen_components::handover::WatchFailure,
};
use futures::{Stream, StreamExt, stream};

use crate::{
    dependency_tree::{DependencyTree, ServiceOrdering},
    monotonic_clock::monotonic_now,
    openrc_error::OpenrcError,
    openrc_vocabulary::milestone_runlevels,
    state_directory::{
        OpenrcPaths, Runlevel, RunlevelChange, ServiceState, StateSnapshot, read_dependency_cache,
        read_snapshot, runlevel_members, state_changes,
    },
};

type FeedBatch = Result<Vec<BootChange>, WatchFailure>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootCompletion {
    Running,
    Reported,
}

pub(crate) struct BootFeed {
    tree: DependencyTree,
    runlevels: &'static [&'static str],
    tracked: HashSet<String>,
    jobs: HashMap<String, u32>,
    next_job: u32,
    previous: StateSnapshot,
    completion: BootCompletion,
}

fn unit_facts(name: &str, kind: UnitKind, ordering: ServiceOrdering) -> UnitFacts {
    UnitFacts {
        name: name.to_owned(),
        short_name: name.to_owned(),
        kind,
        description: None,
        after: ordering.after,
        before: ordering.before,
        timeouts: UnitTimeouts::default(),
    }
}

fn activity_entering(state: ServiceState, at: Duration) -> Activity {
    match state {
        ServiceState::Starting => Activity {
            state: Some(ActiveState::Activating),
            inactive_exit: Some(at),
            ..Activity::default()
        },
        ServiceState::Started | ServiceState::Inactive => Activity {
            state: Some(ActiveState::Active),
            active_enter: Some(at),
            ..Activity::default()
        },
        ServiceState::Stopping => Activity {
            state: Some(ActiveState::Deactivating),
            ..Activity::default()
        },
        ServiceState::Stopped => Activity {
            state: Some(ActiveState::Inactive),
            active_exit: Some(at),
            ..Activity::default()
        },
        ServiceState::Failed => Activity {
            state: Some(ActiveState::Failed),
            ..Activity::default()
        },
    }
}

fn runlevel_position(runlevels: &[&str], runlevel: &Runlevel) -> Option<usize> {
    let Runlevel::Named(name) = runlevel else {
        return None;
    };
    runlevels.iter().position(|known| known == name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunlevelProgress {
    Reached,
    Pending,
}

fn runlevel_progress(
    runlevels: &[&str],
    index: usize,
    snapshot: &StateSnapshot,
) -> RunlevelProgress {
    match runlevel_position(runlevels, &snapshot.runlevel) {
        Some(current) if current > index => RunlevelProgress::Reached,
        Some(current) if current == index && snapshot.change == RunlevelChange::Settled => {
            RunlevelProgress::Reached
        }
        Some(_) | None => RunlevelProgress::Pending,
    }
}

impl BootFeed {
    pub(crate) fn new(tree: DependencyTree, phase: Phase) -> Self {
        Self {
            tree,
            runlevels: milestone_runlevels(phase),
            tracked: HashSet::new(),
            jobs: HashMap::new(),
            next_job: 1,
            previous: StateSnapshot::default(),
            completion: BootCompletion::Running,
        }
    }

    pub(crate) fn opening_changes(
        &mut self,
        members: impl Fn(&str) -> Vec<String>,
        at: Duration,
    ) -> Vec<BootChange> {
        let root = self
            .runlevels
            .last()
            .expect("every phase has a runlevel to reach");
        let runlevel_units: Vec<BootChange> = self
            .runlevels
            .iter()
            .scan(Vec::new(), |earlier: &mut Vec<String>, runlevel| {
                let after = members(runlevel)
                    .into_iter()
                    .chain(earlier.clone())
                    .collect();
                earlier.push((*runlevel).to_owned());
                Some(BootChange::TrackUnit {
                    facts: unit_facts(
                        runlevel,
                        UnitKind::Target,
                        ServiceOrdering {
                            after,
                            before: Vec::new(),
                        },
                    ),
                    activity: Activity::default(),
                })
            })
            .collect();
        let runlevel_jobs: Vec<BootChange> = self
            .runlevels
            .iter()
            .map(|runlevel| self.queue(runlevel, JobKind::BringsUp, at))
            .collect();
        [BootChange::SetOrigin(Duration::ZERO)]
            .into_iter()
            .chain(runlevel_units)
            .chain([BootChange::SetRoot((*root).to_owned())])
            .chain(runlevel_jobs)
            .collect()
    }

    fn queue(&mut self, unit: &str, kind: JobKind, at: Duration) -> BootChange {
        let id = self.next_job;
        self.next_job += 1;
        self.jobs.insert(unit.to_owned(), id);
        BootChange::QueueJob {
            id,
            unit: unit.to_owned(),
            kind,
            at,
        }
    }

    fn finish(&mut self, unit: &str, result: JobResult, at: Duration) -> Option<BootChange> {
        let id = self.jobs.remove(unit)?;
        Some(BootChange::FinishJob { id, result, at })
    }

    fn track(&mut self, service: &str) -> Option<BootChange> {
        self.tracked
            .insert(service.to_owned())
            .then(|| BootChange::TrackUnit {
                facts: unit_facts(service, UnitKind::Service, self.tree.ordering_of(service)),
                activity: Activity::default(),
            })
    }

    fn service_moved(
        &mut self,
        service: &str,
        state: ServiceState,
        at: Duration,
    ) -> Vec<BootChange> {
        let tracked = self.track(service);
        let job = match state {
            ServiceState::Starting if !self.jobs.contains_key(service) => {
                Some(self.queue(service, JobKind::BringsUp, at))
            }
            ServiceState::Stopping if !self.jobs.contains_key(service) => {
                Some(self.queue(service, JobKind::TakesDown, at))
            }
            ServiceState::Started | ServiceState::Inactive | ServiceState::Stopped => {
                self.finish(service, JobResult::Done, at)
            }
            ServiceState::Failed => self.finish(service, JobResult::Failed, at),
            ServiceState::Starting | ServiceState::Stopping => None,
        };
        let activity = BootChange::UpdateActivity {
            unit: service.to_owned(),
            activity: activity_entering(state, at),
        };
        tracked.into_iter().chain(job).chain([activity]).collect()
    }

    fn reached_runlevels(&mut self, snapshot: &StateSnapshot, at: Duration) -> Vec<BootChange> {
        let runlevels = self.runlevels;
        runlevels
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                runlevel_progress(runlevels, *index, snapshot) == RunlevelProgress::Reached
            })
            .filter_map(|(_, runlevel)| self.finish(runlevel, JobResult::Done, at))
            .collect()
    }

    fn boot_finished(&mut self, snapshot: &StateSnapshot) -> Option<BootChange> {
        let root_index = self.runlevels.len() - 1;
        let services_starting = snapshot
            .services
            .values()
            .any(|state| *state == ServiceState::Starting);
        match (
            self.completion,
            runlevel_progress(self.runlevels, root_index, snapshot),
        ) {
            (BootCompletion::Running, RunlevelProgress::Reached) if !services_starting => {
                self.completion = BootCompletion::Reported;
                Some(BootChange::BootFinished)
            }
            _ => None,
        }
    }

    pub(crate) fn changes_towards(
        &mut self,
        current: StateSnapshot,
        at: Duration,
    ) -> Vec<BootChange> {
        let names: BTreeSet<&String> = self
            .previous
            .services
            .keys()
            .chain(current.services.keys())
            .collect();
        let moved: Vec<String> = names
            .into_iter()
            .filter(|name| self.previous.state_of(name) != current.state_of(name))
            .cloned()
            .collect();
        let service_changes: Vec<BootChange> = moved
            .iter()
            .flat_map(|name| self.service_moved(name, current.state_of(name), at))
            .collect();
        let runlevel_changes = self.reached_runlevels(&current, at);
        let finished = self.boot_finished(&current);
        self.previous = current;
        service_changes
            .into_iter()
            .chain(runlevel_changes)
            .chain(finished)
            .collect()
    }
}

fn non_empty(batch: &FeedBatch) -> bool {
    !matches!(batch, Ok(changes) if changes.is_empty())
}

pub(crate) fn boot_change_stream(
    paths: OpenrcPaths,
    phase: Phase,
) -> impl Stream<Item = FeedBatch> + 'static {
    let tree = read_dependency_cache(&paths)
        .parse()
        .expect("parsing the OpenRC dependency cache never fails");
    let mut feed = BootFeed::new(tree, phase);
    let at = monotonic_now();
    let opening = feed
        .opening_changes(|runlevel| runlevel_members(&paths, runlevel), at)
        .into_iter()
        .chain(feed.changes_towards(read_snapshot(&paths), at))
        .collect();
    let opened = stream::iter([Ok(opening)]);
    match state_changes(&paths) {
        Err(error) => opened
            .chain(stream::iter([Err(OpenrcError::from(error).into())]))
            .left_stream(),
        Ok(changes) => {
            opened
                .chain(changes.map(move |()| {
                    Ok(feed.changes_towards(read_snapshot(&paths), monotonic_now()))
                }))
                .filter(|batch| ready(non_empty(batch)))
                .right_stream()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        env, fs,
        os::unix::fs::symlink,
        path::{Path, PathBuf},
        process,
    };

    use bootvine_screen::boot_model::BootState;
    use tokio::{runtime::Builder, time::timeout};

    use super::*;
    use crate::openrc_vocabulary::openrc_vocabulary;

    const MS: Duration = Duration::from_millis(1);
    const STREAM_PATIENCE: Duration = Duration::from_secs(5);

    fn snapshot(
        runlevel: &str,
        change: RunlevelChange,
        services: &[(&str, ServiceState)],
    ) -> StateSnapshot {
        StateSnapshot {
            services: services
                .iter()
                .fold(BTreeMap::new(), |mut listed, (name, state)| {
                    listed.insert((*name).to_owned(), *state);
                    listed
                }),
            runlevel: Runlevel::Named(runlevel.to_owned()),
            change,
        }
    }

    fn opened_feed(state: &mut BootState) -> BootFeed {
        let mut feed = BootFeed::new(DependencyTree::default(), Phase::Host);
        feed.opening_changes(|_| Vec::new(), MS)
            .into_iter()
            .for_each(|change| state.apply(change));
        feed
    }

    fn advance(
        feed: &mut BootFeed,
        state: &mut BootState,
        next: StateSnapshot,
        at: Duration,
    ) -> Vec<BootChange> {
        let changes = feed.changes_towards(next, at);
        changes
            .iter()
            .cloned()
            .for_each(|change| state.apply(change));
        changes
    }

    fn queued_units(changes: &[BootChange]) -> Vec<&str> {
        changes
            .iter()
            .filter_map(|change| match change {
                BootChange::QueueJob { unit, .. } => Some(unit.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn opening_queues_every_runlevel_and_roots_the_tree_at_default() {
        let mut feed = BootFeed::new(DependencyTree::default(), Phase::Host);
        let changes = feed.opening_changes(|_| Vec::new(), MS);
        assert_eq!(queued_units(&changes), BOOT_RUNLEVEL_NAMES);
        assert!(changes.contains(&BootChange::SetRoot("default".to_owned())));
    }

    const BOOT_RUNLEVEL_NAMES: [&str; 3] = ["sysinit", "boot", "default"];

    #[test]
    fn a_starting_service_is_queued_then_finished_once_started() {
        let mut state = BootState::new(Phase::Host, &openrc_vocabulary());
        let mut feed = opened_feed(&mut state);
        let started = advance(
            &mut feed,
            &mut state,
            snapshot(
                "boot",
                RunlevelChange::Starting,
                &[("sshd", ServiceState::Starting)],
            ),
            2 * MS,
        );
        assert_eq!(queued_units(&started), ["sshd"]);
        let finished = advance(
            &mut feed,
            &mut state,
            snapshot(
                "boot",
                RunlevelChange::Starting,
                &[("sshd", ServiceState::Started)],
            ),
            3 * MS,
        );
        assert!(finished.iter().any(|change| matches!(
            change,
            BootChange::FinishJob {
                result: JobResult::Done,
                ..
            }
        )));
    }

    #[test]
    fn a_failed_service_is_recorded_as_a_failure() {
        let mut state = BootState::new(Phase::Host, &openrc_vocabulary());
        let mut feed = opened_feed(&mut state);
        advance(
            &mut feed,
            &mut state,
            snapshot(
                "default",
                RunlevelChange::Starting,
                &[("broken", ServiceState::Starting)],
            ),
            2 * MS,
        );
        advance(
            &mut feed,
            &mut state,
            snapshot(
                "default",
                RunlevelChange::Starting,
                &[("broken", ServiceState::Failed)],
            ),
            3 * MS,
        );
        assert!(state.has_failed("broken"));
    }

    #[test]
    fn the_boot_finishes_once_default_settles_and_reports_it_once() {
        let mut state = BootState::new(Phase::Host, &openrc_vocabulary());
        let mut feed = opened_feed(&mut state);
        let settled = snapshot(
            "default",
            RunlevelChange::Settled,
            &[("sshd", ServiceState::Started)],
        );
        let changes = advance(&mut feed, &mut state, settled.clone(), 2 * MS);
        assert!(changes.contains(&BootChange::BootFinished));
        assert!(state.is_complete());
        assert!(advance(&mut feed, &mut state, settled, 3 * MS).is_empty());
    }

    #[test]
    fn a_service_still_starting_holds_the_boot_open() {
        let mut state = BootState::new(Phase::Host, &openrc_vocabulary());
        let mut feed = opened_feed(&mut state);
        let changes = advance(
            &mut feed,
            &mut state,
            snapshot(
                "default",
                RunlevelChange::Settled,
                &[("ntpd", ServiceState::Starting)],
            ),
            2 * MS,
        );
        assert!(!changes.contains(&BootChange::BootFinished));
    }

    struct ScratchTree {
        root: PathBuf,
    }

    impl ScratchTree {
        fn create(test_name: &str) -> Self {
            let root =
                env::temp_dir().join(format!("bootvine-openrc-{test_name}-{}", process::id()));
            drop(fs::remove_dir_all(&root));
            [
                "run/starting",
                "run/started",
                "run/failed",
                "run/stopping",
                "run/inactive",
                "runlevels/default",
            ]
            .iter()
            .try_for_each(|directory| fs::create_dir_all(root.join(directory)))
            .expect("the scratch tree can be created");
            Self { root }
        }

        fn paths(&self) -> OpenrcPaths {
            OpenrcPaths {
                run_root: self.root.join("run"),
                runlevel_root: self.root.join("runlevels"),
            }
        }

        fn mark(&self, state: &str, service: &str) {
            symlink(
                Path::new("/etc/init.d").join(service),
                self.root.join("run").join(state).join(service),
            )
            .expect("the scratch state directory accepts links");
        }

        fn unmark(&self, state: &str, service: &str) {
            fs::remove_file(self.root.join("run").join(state).join(service))
                .expect("the scratch link exists");
        }
    }

    impl Drop for ScratchTree {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.root));
        }
    }

    async fn next_batch(batches: &mut (impl Stream<Item = FeedBatch> + Unpin)) -> Vec<BootChange> {
        timeout(STREAM_PATIENCE, batches.next())
            .await
            .expect("a batch arrives in time")
            .expect("the stream stays open")
            .expect("the state directory stays readable")
    }

    #[test]
    fn links_appearing_in_the_state_directories_stream_as_jobs() {
        let scratch = ScratchTree::create("stream");
        fs::write(scratch.root.join("run/softlevel"), "boot\n").expect("softlevel is writable");
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .expect("a test runtime starts");
        runtime.block_on(async {
            let mut batches = Box::pin(boot_change_stream(scratch.paths(), Phase::Host));
            let opening = next_batch(&mut batches).await;
            assert_eq!(queued_units(&opening), BOOT_RUNLEVEL_NAMES);
            scratch.mark("starting", "sshd");
            assert_eq!(queued_units(&next_batch(&mut batches).await), ["sshd"]);
            scratch.mark("started", "sshd");
            scratch.unmark("starting", "sshd");
            let finished = next_batch(&mut batches).await;
            assert!(
                finished
                    .iter()
                    .any(|change| matches!(change, BootChange::FinishJob { .. }))
            );
        });
    }
}
