use std::{
    collections::{HashMap, HashSet},
    iter::once,
    pin::pin,
    process,
    time::{Duration, Instant},
};

use bootvine_screen::{
    boot_model::{
        ActiveState, Activity, BootChange, BootProgress, FailureCause, JobKind, JobResult, Phase,
        RootRequest, Scope, UnitFacts, UnitTimeouts,
    },
    screen_components::handover::WatchFailure,
};
use dbus::Path;
use futures::{
    FutureExt, Stream, StreamExt, TryStreamExt,
    future::{BoxFuture, Either, select},
    stream,
    stream::FuturesUnordered,
};
use tokio::time::sleep;
use tracing::info;

use crate::{
    descriptor_streams::{BACKGROUND_CHECK_INTERVAL, CarriedItem, unfold_carrying},
    journal_tail::settled_unit_messages,
    link_error::LinkFailure,
    monotonic_clock::monotonic_now,
    switch_root::Sysroot,
    systemd_bus::{
        BusConnection, FailureReason, FetchedUnit, JobListing, Session, Signal, SignalBatch,
        UnitLookup,
    },
    systemd_names::systemd_unit_facts,
    systemd_vocabulary::{HANDOFF_UNIT, systemd_vocabulary},
};

const RECONNECT_PATIENCE: Duration = Duration::from_secs(60);
const JOURNAL_SETTLING_PATIENCE: Duration = Duration::from_secs(1);
const LOG_LINES_KEPT: usize = 6;
const SWITCH_ROOT_UNIT: &str = "initrd-switch-root.service";
const SHUTDOWN_ROOTS: [&str; 5] = [
    "poweroff.target",
    "reboot.target",
    "halt.target",
    "kexec.target",
    "soft-reboot.target",
];
const SHUTDOWN_FALLBACK_ROOT: &str = "shutdown.target";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectAttempt {
    First,
    Again,
}

enum Link {
    Connected(BusConnection),
    Connecting {
        deadline: Instant,
        attempt: ConnectAttempt,
    },
    Finished,
}

impl Link {
    fn connecting(attempt: ConnectAttempt) -> Self {
        Self::Connecting {
            deadline: Instant::now() + RECONNECT_PATIENCE,
            attempt,
        }
    }
}

enum RootSwitch {
    NotQueued,
    Queued(Sysroot),
}

enum TrackedUnit {
    Known(String),
    New {
        facts: Box<UnitFacts>,
        activity: Activity,
    },
    NotLoaded(String),
}

impl TrackedUnit {
    fn name(&self) -> &str {
        match self {
            Self::Known(name) | Self::NotLoaded(name) => name,
            Self::New { facts, .. } => &facts.name,
        }
    }

    fn into_change(self) -> Option<BootChange> {
        match self {
            Self::Known(_) | Self::NotLoaded(_) => None,
            Self::New { facts, activity } => Some(BootChange::TrackUnit {
                facts: *facts,
                activity,
            }),
        }
    }
}

fn queued_units(signals: &[Signal]) -> impl Iterator<Item = &str> {
    signals.iter().filter_map(|signal| match signal {
        Signal::JobNew { unit, .. } => Some(unit.as_str()),
        _ => None,
    })
}

pub(crate) type FeedBatch = Result<Vec<BootChange>, WatchFailure>;

fn connected_inputs(changes: Vec<BootChange>) -> Vec<FeedBatch> {
    (!changes.is_empty())
        .then_some(Ok(changes))
        .into_iter()
        .collect()
}

struct BootTracker {
    scope: Scope,
    target: RootRequest,
    phase: Phase,
    unit_paths: HashMap<Path<'static>, String>,
    explained_failures: HashSet<String>,
    units_in_failed_state: HashSet<String>,
    root_switch: RootSwitch,
    pending_journal_reads: FuturesUnordered<BoxFuture<'static, BootChange>>,
}

struct BusFeed {
    link: Link,
    tracker: BootTracker,
}

struct FeedProgress<'feed> {
    tracker: &'feed mut BootTracker,
    connection: &'feed mut BusConnection,
    changes: Vec<BootChange>,
}

impl BusFeed {
    fn new(phase: Phase, scope: Scope, target: RootRequest) -> Self {
        Self {
            link: Link::connecting(ConnectAttempt::First),
            tracker: BootTracker {
                scope,
                target,
                phase,
                unit_paths: HashMap::new(),
                explained_failures: HashSet::new(),
                units_in_failed_state: HashSet::new(),
                root_switch: RootSwitch::NotQueued,
                pending_journal_reads: FuturesUnordered::new(),
            },
        }
    }

    async fn next_inputs(&mut self) -> Option<Vec<FeedBatch>> {
        match &mut self.link {
            Link::Finished => None,
            Link::Connecting { deadline, attempt } => {
                let deadline = *deadline;
                let attempt = *attempt;
                Some(self.connect(deadline, attempt).await)
            }
            Link::Connected(connection) => {
                let received = self.tracker.receive(connection).await;
                Some(match received {
                    Ok(mut changes) => {
                        changes.extend(self.tracker.finished_journal_reads());
                        connected_inputs(changes)
                    }
                    Err(_) => self.lose_connection(),
                })
            }
        }
    }

    async fn open_watched(
        &mut self,
        attempt: ConnectAttempt,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let mut connection = BusConnection::register(Session::open(self.tracker.scope)?)?;
        let changes = self
            .tracker
            .start_watching(&mut connection, attempt)
            .await?;
        self.link = Link::Connected(connection);
        Ok(changes)
    }

    async fn connect(&mut self, deadline: Instant, attempt: ConnectAttempt) -> Vec<FeedBatch> {
        match self.open_watched(attempt).await {
            Ok(changes) => vec![Ok(changes)],
            Err(_) if Instant::now() < deadline => {
                sleep(BACKGROUND_CHECK_INTERVAL).await;
                Vec::new()
            }
            Err(_) => {
                self.link = Link::Finished;
                vec![Err(LinkFailure::ManagerUnreachable.into())]
            }
        }
    }

    fn lose_connection(&mut self) -> Vec<FeedBatch> {
        let at = monotonic_now();
        let tracker = &mut self.tracker;
        let RootSwitch::Queued(sysroot) = &tracker.root_switch else {
            if tracker.phase == Phase::Shutdown {
                self.link = Link::Finished;
                return vec![Ok(vec![BootChange::ManagerExited { at }])];
            }
            tracker.unit_paths.clear();
            self.link = Link::connecting(ConnectAttempt::Again);
            return vec![Ok(vec![BootChange::ForgetJobs])];
        };
        if let Err(error) = sysroot.enter() {
            self.link = Link::Finished;
            return vec![Err(LinkFailure::from(error).into())];
        }
        tracker.root_switch = RootSwitch::NotQueued;
        tracker.unit_paths.clear();
        tracker.explained_failures.clear();
        tracker.units_in_failed_state.clear();
        tracker.phase = Phase::Host;
        self.link = Link::connecting(ConnectAttempt::Again);
        vec![Ok(vec![BootChange::SwitchRoot { at }])]
    }
}

impl BootTracker {
    async fn wait_for_bus_or_journal(&mut self, connection: &BusConnection) -> Vec<BootChange> {
        if self.pending_journal_reads.is_empty() {
            connection.wait_for_signals().await;
            return Vec::new();
        }
        match select(
            pin!(connection.wait_for_signals()),
            self.pending_journal_reads.next(),
        )
        .await
        {
            Either::Right((Some(change), _)) => vec![change],
            Either::Left(_) | Either::Right((None, _)) => Vec::new(),
        }
    }

    async fn receive(
        &mut self,
        connection: &mut BusConnection,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let finished_reads = self.wait_for_bus_or_journal(connection).await;
        match connection.take_signals() {
            SignalBatch::Open(signals) => {
                self.arm_root_switch(queued_units(&signals));
                let mut changes = self
                    .collect_changes(connection, signals, async |tracker, connection, signal| {
                        tracker.changes_for(connection, signal).await
                    })
                    .await?;
                changes.extend(finished_reads);
                Ok(changes)
            }
            SignalBatch::Closed(signals) => {
                self.arm_root_switch(queued_units(&signals));
                Err(LinkFailure::Disconnected)
            }
        }
    }

    fn arm_root_switch<'unit>(&mut self, mut queued_units: impl Iterator<Item = &'unit str>) {
        if self.phase == Phase::Initrd
            && matches!(self.root_switch, RootSwitch::NotQueued)
            && queued_units.any(|unit| unit == SWITCH_ROOT_UNIT)
            && let Ok(sysroot) = Sysroot::hold()
        {
            self.root_switch = RootSwitch::Queued(sysroot);
        }
    }

    async fn collect_changes<Item>(
        &mut self,
        connection: &mut BusConnection,
        items: Vec<Item>,
        step: impl AsyncFn(&mut Self, &mut BusConnection, Item) -> Result<Vec<BootChange>, LinkFailure>,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let step = &step;
        let progress = stream::iter(items)
            .map(Ok::<Item, LinkFailure>)
            .try_fold(
                FeedProgress {
                    tracker: self,
                    connection,
                    changes: Vec::new(),
                },
                |mut progress, item| async move {
                    let changes = step(progress.tracker, progress.connection, item).await?;
                    progress.changes.extend(changes);
                    Ok(progress)
                },
            )
            .await?;
        Ok(progress.changes)
    }

    async fn requested_root(
        &self,
        connection: &mut BusConnection,
        jobs: &[JobListing],
    ) -> Result<String, LinkFailure> {
        let RootRequest::Named(target) = &self.target else {
            if self.phase != Phase::Shutdown {
                return connection.default_target().await;
            }
            return Ok(SHUTDOWN_ROOTS
                .into_iter()
                .find(|root| {
                    jobs.iter()
                        .any(|job| job.unit == *root && job.kind == JobKind::BringsUp)
                })
                .unwrap_or(SHUTDOWN_FALLBACK_ROOT)
                .to_owned());
        };
        Ok(target.clone())
    }

    async fn boot_progress_changes(
        &self,
        connection: &mut BusConnection,
        attempt: ConnectAttempt,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let progress = connection.boot_progress().await?;
        let origin = (attempt == ConnectAttempt::First).then(|| {
            BootChange::SetOrigin(match progress {
                BootProgress::InProgress => Duration::ZERO,
                BootProgress::Finished => monotonic_now(),
            })
        });
        let finished = match progress {
            BootProgress::InProgress => Vec::new(),
            BootProgress::Finished => self.boot_finished(),
        };
        Ok(origin.into_iter().chain(finished).collect())
    }

    async fn start_watching(
        &mut self,
        connection: &mut BusConnection,
        attempt: ConnectAttempt,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        connection.subscribe().await?;
        if self.scope == Scope::System {
            connection.silence_status_output().await?;
        }
        let shutting_down = self.phase == Phase::Shutdown;
        let jobs: Vec<JobListing> = connection
            .list_jobs()
            .await?
            .into_iter()
            .filter(|job| shutting_down || job.kind == JobKind::BringsUp)
            .collect();
        self.arm_root_switch(jobs.iter().map(|job| job.unit.as_str()));
        let requested_root = self.requested_root(connection, &jobs).await?;
        let boot_progress = self.boot_progress_changes(connection, attempt).await?;
        let tracked_root = self.track(connection, &requested_root).await?;
        let root = tracked_root.name().to_owned();
        let tracked_handoff = self.track(connection, HANDOFF_UNIT).await?;
        let handoff = match &tracked_handoff {
            TrackedUnit::NotLoaded(_) => None,
            loaded => Some(BootChange::SetHandoff(loaded.name().to_owned())),
        };
        let watched_units = jobs
            .iter()
            .map(|job| job.unit.clone())
            .chain(systemd_vocabulary().milestones_for(self.phase))
            .collect();
        let tracked_changes = self
            .collect_changes(
                connection,
                watched_units,
                async |tracker, connection, unit| {
                    let tracked = tracker.track(connection, &unit).await?;
                    Ok(tracked.into_change().into_iter().collect())
                },
            )
            .await?;
        info!(
            unit = %root,
            phase = self.phase.label(),
            pid = process::id(),
            transport = if connection.peer_to_peer() {
                "the private socket"
            } else {
                "the bus"
            },
            "watching the boot",
        );
        let queued_at = monotonic_now();
        Ok(boot_progress
            .into_iter()
            .chain(tracked_root.into_change())
            .chain(tracked_handoff.into_change())
            .chain(handoff)
            .chain(tracked_changes)
            .chain(once(BootChange::SetRoot(root)))
            .chain(jobs.into_iter().map(|job| BootChange::QueueJob {
                id: job.id,
                unit: job.unit,
                kind: job.kind,
                at: queued_at,
            }))
            .collect())
    }

    async fn track(
        &mut self,
        connection: &mut BusConnection,
        unit: &str,
    ) -> Result<TrackedUnit, LinkFailure> {
        if self.unit_paths.values().any(|known| known == unit) {
            return Ok(TrackedUnit::Known(unit.to_owned()));
        }
        let UnitLookup::Loaded(fetched) = connection.fetch_unit(unit).await? else {
            return Ok(TrackedUnit::NotLoaded(unit.to_owned()));
        };
        let FetchedUnit {
            path,
            facts,
            activity,
        } = *fetched;
        if self.unit_paths.values().any(|known| *known == facts.name) {
            return Ok(TrackedUnit::Known(facts.name));
        }
        self.unit_paths.insert(path, facts.name.clone());
        Ok(TrackedUnit::New {
            facts: Box::new(facts),
            activity,
        })
    }

    async fn changes_for(
        &mut self,
        connection: &mut BusConnection,
        signal: Signal,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let at = monotonic_now();
        Ok(match signal {
            Signal::JobNew { id, job, unit } => {
                self.queue_new_job(connection, id, &job, unit, at).await?
            }
            Signal::JobRemoved { id, unit, result } => {
                let explanation = match result {
                    JobResult::Failed => self.explain_failure(connection, unit, "failed").await?,
                    JobResult::Timeout => {
                        self.explain_failure(connection, unit, "timed out").await?
                    }
                    _ => None,
                };
                once(BootChange::FinishJob { id, result, at })
                    .chain(explanation)
                    .collect()
            }
            Signal::UnitChanged { path, activity } => {
                let Some(unit) = self.unit_paths.get(&path).cloned() else {
                    return Ok(Vec::new());
                };
                let explanation = if self.enters_failed_state(&unit, activity) {
                    self.explain_failure(connection, unit.clone(), "failed")
                        .await?
                } else {
                    None
                };
                once(BootChange::UpdateActivity { unit, activity })
                    .chain(explanation)
                    .collect()
            }
            Signal::BootFinished => self.boot_finished(),
        })
    }

    fn enters_failed_state(&mut self, unit: &str, activity: Activity) -> bool {
        match activity.state {
            Some(ActiveState::Failed) => self.units_in_failed_state.insert(unit.to_owned()),
            Some(_) => {
                self.units_in_failed_state.remove(unit);
                false
            }
            None => false,
        }
    }

    async fn explain_failure(
        &mut self,
        connection: &mut BusConnection,
        unit: String,
        fallback_reason: &str,
    ) -> Result<Option<BootChange>, LinkFailure> {
        if !self.unit_paths.values().any(|known| *known == unit) {
            return Ok(None);
        }
        self.explained_failures.insert(unit.clone());
        let reason = match connection.failure_reason(&unit).await? {
            FailureReason::Explained(reason) => reason,
            FailureReason::Unexplained => fallback_reason.to_owned(),
        };
        let settled_unit = unit.clone();
        let settled_reason = reason.clone();
        self.pending_journal_reads.push(
            async move {
                let log_lines = settled_unit_messages(
                    settled_unit.clone(),
                    LOG_LINES_KEPT,
                    JOURNAL_SETTLING_PATIENCE,
                )
                .await;
                BootChange::ExplainFailure {
                    unit: settled_unit,
                    cause: FailureCause {
                        reason: settled_reason,
                        log_lines,
                    },
                }
            }
            .boxed(),
        );
        Ok(Some(BootChange::ReasonFailure { unit, reason }))
    }

    fn finished_journal_reads(&mut self) -> Vec<BootChange> {
        let mut finished_reads = Vec::new();
        while let Some(Some(change)) = self.pending_journal_reads.next().now_or_never() {
            finished_reads.push(change);
        }
        finished_reads
    }

    fn boot_finished(&self) -> Vec<BootChange> {
        (self.phase == Phase::Host)
            .then_some(BootChange::BootFinished)
            .into_iter()
            .collect()
    }

    async fn reload(
        &mut self,
        connection: &mut BusConnection,
        unit: &str,
    ) -> Result<TrackedUnit, LinkFailure> {
        let UnitLookup::Loaded(fetched) = connection.load_unit(unit).await? else {
            return Ok(TrackedUnit::New {
                facts: Box::new(systemd_unit_facts(
                    unit.to_owned(),
                    None,
                    Vec::new(),
                    Vec::new(),
                    UnitTimeouts::default(),
                )),
                activity: Activity::default(),
            });
        };
        let FetchedUnit {
            path,
            facts,
            activity,
        } = *fetched;
        self.unit_paths.insert(path, facts.name.clone());
        Ok(TrackedUnit::New {
            facts: Box::new(facts),
            activity,
        })
    }

    async fn queue_new_job(
        &mut self,
        connection: &mut BusConnection,
        id: u32,
        job: &Path<'static>,
        unit: String,
        at: Duration,
    ) -> Result<Vec<BootChange>, LinkFailure> {
        let kind = connection.job_kind(job).await?;
        if kind == JobKind::TakesDown && self.phase != Phase::Shutdown {
            return Ok(Vec::new());
        }
        let tracked = match self.track(connection, &unit).await? {
            TrackedUnit::NotLoaded(unloaded) => self.reload(connection, &unloaded).await?,
            tracked => tracked,
        };
        let unit = tracked.name().to_owned();
        Ok(tracked
            .into_change()
            .into_iter()
            .chain(once(BootChange::QueueJob { id, unit, kind, at }))
            .collect())
    }
}

pub(crate) fn boot_change_stream(
    phase: Phase,
    scope: Scope,
    target: RootRequest,
) -> impl Stream<Item = FeedBatch> + 'static {
    unfold_carrying(BusFeed::new(phase, scope, target), |mut feed| async move {
        let inputs = feed.next_inputs().await?;
        Some(CarriedItem {
            item: inputs,
            state: feed,
        })
    })
    .flat_map(stream::iter)
}

#[cfg(test)]
mod tests {
    use futures::future::{pending, ready};

    use super::*;

    fn host_tracker() -> BootTracker {
        BootTracker {
            scope: Scope::System,
            target: RootRequest::ManagerDefault,
            phase: Phase::Host,
            unit_paths: HashMap::new(),
            explained_failures: HashSet::new(),
            units_in_failed_state: HashSet::new(),
            root_switch: RootSwitch::NotQueued,
            pending_journal_reads: FuturesUnordered::new(),
        }
    }

    #[test]
    fn only_batches_with_changes_reach_the_screen() {
        assert!(connected_inputs(Vec::new()).is_empty());
        assert!(matches!(
            connected_inputs(vec![BootChange::ForgetJobs]).as_slice(),
            [Ok(_)]
        ));
    }

    fn in_state(state: ActiveState) -> Activity {
        Activity {
            state: Some(state),
            ..Activity::default()
        }
    }

    #[test]
    fn only_entering_the_failed_state_asks_for_a_fresh_explanation() {
        let mut tracker = host_tracker();
        let unit = "http-listener.service";
        assert!(tracker.enters_failed_state(unit, in_state(ActiveState::Failed)));
        assert!(!tracker.enters_failed_state(unit, in_state(ActiveState::Failed)));
        assert!(!tracker.enters_failed_state(unit, Activity::default()));
        assert!(!tracker.enters_failed_state(unit, in_state(ActiveState::Activating)));
        assert!(tracker.enters_failed_state(unit, in_state(ActiveState::Failed)));
    }

    #[test]
    fn journal_reads_reach_the_screen_once_finished_without_holding_back_others() {
        let mut tracker = host_tracker();
        tracker.pending_journal_reads.push(pending().boxed());
        tracker.pending_journal_reads.push(
            ready(BootChange::ExplainFailure {
                unit: "http-listener.service".to_owned(),
                cause: FailureCause {
                    reason: "exited with status 1".to_owned(),
                    log_lines: vec!["port 80 already in use".to_owned()],
                },
            })
            .boxed(),
        );
        let finished_reads = tracker.finished_journal_reads();
        assert!(matches!(
            finished_reads.as_slice(),
            [BootChange::ExplainFailure { unit, cause }]
                if unit == "http-listener.service" && cause.log_lines == ["port 80 already in use"]
        ));
        assert!(tracker.finished_journal_reads().is_empty());
        assert_eq!(tracker.pending_journal_reads.len(), 1);
    }
}
