use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    iter::once,
    mem,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use bootvine::{
    boot_model::{
        ActiveState, Activity, BootChange, BootState, FailureCause, JobKind, JobResult, Phase,
        UnitFacts, UnitTimeouts, boot_origin::BootOrigin,
    },
    console_session::{
        console_palette::ConsoleLook,
        screen_loop::{ScreenRuntime, run_screen},
        stream_sources::{frame_inputs, key_inputs},
    },
    screen_components::{
        console_claim::{ConsoleClaim, Foreground, MarkerPresence, OwnConsole},
        handover::{Handover, WatchEnding},
        password_request::{PasswordRequest, PromptAsker, PromptDeadline, RequestHint},
        prompt::Prompt,
        screen_app::{Cmd, Model, Msg, Sub},
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
    systemd_link::{monotonic_clock::monotonic_now, systemd_bus::Scope},
};
use futures::{FutureExt, StreamExt, stream, stream::LocalBoxStream};
use ratatui::DefaultTerminal;
use tokio::{
    runtime::Builder,
    time::{interval, sleep},
};

const FINAL_FRAME: Duration = Duration::from_secs(3);
const FRAME_TICK: Duration = Duration::from_millis(16);

pub const DEMO_PASSPHRASE: &str = "demo";
const DEMO_PASSPHRASE_HINT: &str = "This demo unlocks with the passphrase \"demo\"";

#[derive(Clone, Copy)]
pub struct ScriptedFailure {
    pub reason: &'static str,
    pub log_lines: &'static [&'static str],
}

#[derive(Clone, Copy)]
enum StepEnding {
    Succeeds,
    Fails(ScriptedFailure),
}

pub struct Step {
    unit: &'static str,
    after: &'static [&'static str],
    work: Duration,
    ending: StepEnding,
    stops: bool,
    asks: Option<&'static str>,
    timeout: Option<Duration>,
}

impl Step {
    #[must_use]
    pub const fn timing_out_after(self, limit: Duration) -> Self {
        Self {
            timeout: Some(limit),
            ..self
        }
    }
}

#[must_use]
pub const fn step(unit: &'static str, after: &'static [&'static str], work: Duration) -> Step {
    Step {
        unit,
        after,
        work,
        ending: StepEnding::Succeeds,
        stops: false,
        asks: None,
        timeout: None,
    }
}

#[must_use]
pub const fn failing(
    unit: &'static str,
    after: &'static [&'static str],
    work: Duration,
    failure: ScriptedFailure,
) -> Step {
    Step {
        ending: StepEnding::Fails(failure),
        ..step(unit, after, work)
    }
}

#[must_use]
pub const fn stopping(unit: &'static str, after: &'static [&'static str], work: Duration) -> Step {
    Step {
        stops: true,
        ..step(unit, after, work)
    }
}

#[must_use]
pub const fn unlocking(
    unit: &'static str,
    after: &'static [&'static str],
    message: &'static str,
    key_derivation: Duration,
) -> Step {
    Step {
        asks: Some(message),
        ..step(unit, after, key_derivation)
    }
}

pub struct Scenario {
    pub phase: Phase,
    pub steps: &'static [Step],
    pub after_switch_root: &'static [Step],
    pub describe: fn(&str) -> Option<&'static str>,
}

struct ScriptedJob {
    id: u32,
    step: &'static Step,
}

impl ScriptedJob {
    fn facts(&self, steps: &[Step], describe: fn(&str) -> Option<&'static str>) -> UnitFacts {
        let is_stopping = |unit: &&str| steps.iter().any(|step| step.unit == *unit && step.stops);
        let (before, after): (Vec<&str>, Vec<&str>) =
            self.step.after.iter().copied().partition(is_stopping);
        UnitFacts {
            name: self.step.unit.to_owned(),
            description: describe(self.step.unit).map(str::to_owned),
            after: after.into_iter().map(ToString::to_string).collect(),
            before: before.into_iter().map(ToString::to_string).collect(),
            timeouts: UnitTimeouts {
                job_running: self.step.timeout,
                ..UnitTimeouts::default()
            },
        }
    }

    fn kind(&self) -> JobKind {
        if self.step.stops {
            JobKind::TakesDown
        } else {
            JobKind::BringsUp
        }
    }

    fn outcome(&self) -> JobResult {
        if matches!(self.step.ending, StepEnding::Fails(_)) {
            JobResult::Failed
        } else {
            JobResult::Done
        }
    }

    fn explanation(&self) -> Option<BootChange> {
        let StepEnding::Fails(failure) = self.step.ending else {
            return None;
        };
        Some(BootChange::ExplainFailure {
            unit: self.step.unit.to_owned(),
            cause: FailureCause {
                reason: failure.reason.to_owned(),
                log_lines: failure.log_lines.iter().map(ToString::to_string).collect(),
            },
        })
    }

    fn initial_activity(&self) -> Activity {
        Activity {
            state: self.step.stops.then_some(ActiveState::Active),
            ..Activity::default()
        }
    }

    fn started_activity(&self, now: Duration) -> Activity {
        if self.step.stops {
            Activity {
                state: Some(ActiveState::Deactivating),
                active_exit: Some(now),
                ..Activity::default()
            }
        } else {
            Activity {
                state: Some(ActiveState::Activating),
                inactive_exit: Some(now),
                ..Activity::default()
            }
        }
    }

    fn settled_activity(&self, now: Duration) -> Activity {
        let state = if matches!(self.step.ending, StepEnding::Fails(_)) {
            ActiveState::Failed
        } else if self.step.stops {
            ActiveState::Inactive
        } else {
            ActiveState::Active
        };
        Activity {
            state: Some(state),
            active_enter: (!self.step.stops).then_some(now),
            ..Activity::default()
        }
    }
}

struct Script {
    steps: &'static [Step],
    jobs: Vec<ScriptedJob>,
    started: HashMap<&'static str, Duration>,
    finished: HashSet<&'static str>,
    answer: Option<Answer>,
}

#[derive(Clone, Copy)]
struct Answer {
    checked_at: Duration,
    unlocks: bool,
}

impl Script {
    fn begin(steps: &'static [Step], first_job: u32) -> Self {
        Self {
            steps,
            jobs: steps
                .iter()
                .zip(first_job..)
                .map(|(step, id)| ScriptedJob { id, step })
                .collect(),
            started: HashMap::new(),
            finished: HashSet::new(),
            answer: None,
        }
    }

    fn opening_changes(
        &self,
        describe: fn(&str) -> Option<&'static str>,
        now: Duration,
    ) -> Vec<BootChange> {
        once(BootChange::SetRoot(self.steps[0].unit.to_owned()))
            .chain(self.jobs.iter().map(|job| BootChange::TrackUnit {
                facts: job.facts(self.steps, describe),
                activity: job.initial_activity(),
            }))
            .chain(self.jobs.iter().map(|job| BootChange::QueueJob {
                id: job.id,
                unit: job.step.unit.to_owned(),
                kind: job.kind(),
                at: now,
            }))
            .collect()
    }

    fn asking(&self) -> Option<&'static Step> {
        self.jobs.iter().map(|job| job.step).find(|step| {
            step.asks.is_some()
                && self.started.contains_key(step.unit)
                && !self.finished.contains(step.unit)
        })
    }

    fn was_rejected(&self, now: Duration) -> bool {
        self.answer
            .is_some_and(|answer| !answer.unlocks && now >= answer.checked_at)
    }

    fn asked_request(&self) -> Option<PasswordRequest> {
        let step = self.asking()?;
        Some(PasswordRequest {
            file: PathBuf::from(step.unit),
            message: step.asks?.to_owned(),
            socket: PathBuf::new(),
            echo: false,
            silent: false,
            not_after: PromptDeadline::Never,
            asker: PromptAsker::Unnamed,
            hint: RequestHint::Shown(DEMO_PASSPHRASE_HINT.to_owned()),
        })
    }

    fn answer(&mut self, submitted: &str, now: Duration) {
        let Some(step) = self.asking() else {
            return;
        };
        self.answer = Some(Answer {
            checked_at: now + step.work,
            unlocks: submitted == DEMO_PASSPHRASE,
        });
    }

    fn is_ready(&self, job: &ScriptedJob) -> bool {
        !self.started.contains_key(job.step.unit)
            && job
                .step
                .after
                .iter()
                .all(|dependency| self.finished.contains(dependency))
    }

    fn is_done(&self) -> bool {
        self.finished.len() == self.jobs.len()
    }

    fn is_due(&self, job: &ScriptedJob, now: Duration) -> bool {
        let done_at = match job.step.asks {
            Some(_) => self
                .answer
                .filter(|answer| answer.unlocks)
                .map(|answer| answer.checked_at),
            None => self
                .started
                .get(job.step.unit)
                .map(|started| *started + job.step.work),
        };
        !self.finished.contains(job.step.unit) && done_at.is_some_and(|done_at| now >= done_at)
    }

    fn advance(&mut self, now: Duration) -> Vec<BootChange> {
        let starting: HashSet<&'static str> = self
            .jobs
            .iter()
            .filter(|job| self.is_ready(job))
            .map(|job| job.step.unit)
            .collect();
        self.started
            .extend(starting.iter().map(|unit| (*unit, now)));
        let due: HashSet<&'static str> = self
            .jobs
            .iter()
            .filter(|job| self.is_due(job, now))
            .map(|job| job.step.unit)
            .collect();
        let changes = self
            .jobs
            .iter()
            .filter(|job| starting.contains(job.step.unit))
            .map(|job| BootChange::UpdateActivity {
                unit: job.step.unit.to_owned(),
                activity: job.started_activity(now),
            })
            .chain(
                self.jobs
                    .iter()
                    .filter(|job| due.contains(job.step.unit))
                    .flat_map(|job| {
                        [
                            BootChange::UpdateActivity {
                                unit: job.step.unit.to_owned(),
                                activity: job.settled_activity(now),
                            },
                            BootChange::FinishJob {
                                id: job.id,
                                result: job.outcome(),
                                at: now,
                            },
                        ]
                        .into_iter()
                        .chain(job.explanation())
                    }),
            )
            .collect();
        self.finished.extend(due);
        changes
    }
}

struct DemoBoot {
    script: Script,
    describe: fn(&str) -> Option<&'static str>,
    next_stage: &'static [Step],
    settled_at: Option<Duration>,
    listed: Option<PasswordRequest>,
    rejection_reported: bool,
}

impl DemoBoot {
    fn enter_next_stage(&mut self, now: Duration) -> Vec<BootChange> {
        if !self.script.is_done() || self.next_stage.is_empty() {
            return Vec::new();
        }
        self.script = Script::begin(mem::take(&mut self.next_stage), 1000);
        once(BootChange::SwitchRoot { at: now })
            .chain(self.script.opening_changes(self.describe, now))
            .collect()
    }

    fn advance(&mut self, now: Duration) -> Vec<Msg> {
        let changes: Vec<BootChange> = self
            .script
            .advance(now)
            .into_iter()
            .chain(self.enter_next_stage(now))
            .collect();
        let asked = self.script.asked_request();
        let listing = (asked != self.listed).then(|| {
            self.listed.clone_from(&asked);
            Msg::PasswordRequestsListed(asked.into_iter().collect())
        });
        let rejected = self.script.was_rejected(now) && !self.rejection_reported;
        let rejection = rejected
            .then(|| {
                self.listed
                    .as_ref()
                    .map(|request| Msg::AnswerRejected(request.file.clone()))
            })
            .flatten();
        self.rejection_reported |= rejected;
        let ended = self.script.is_done() && {
            let settled_at = *self.settled_at.get_or_insert(now);
            now.checked_sub(settled_at)
                .expect("frame times only move forward")
                >= FINAL_FRAME
        };
        (!changes.is_empty())
            .then_some(Msg::Boot(changes))
            .into_iter()
            .chain(listing)
            .chain(rejection)
            .chain(ended.then_some(Msg::BootSourceEnded))
            .collect()
    }
}

struct DemoRuntime {
    boot: Rc<RefCell<DemoBoot>>,
    epoch: Duration,
}

fn demo_now(epoch: Duration) -> Duration {
    monotonic_now()
        .checked_sub(epoch)
        .expect("the demo clock starts before its first frame")
}

impl ScreenRuntime for DemoRuntime {
    fn perform(&mut self, command: Cmd) -> Vec<Msg> {
        match command {
            Cmd::SendAnswer {
                file, passphrase, ..
            } => {
                let mut boot = self.boot.borrow_mut();
                boot.script
                    .answer(passphrase.expose(), demo_now(self.epoch));
                boot.rejection_reported = false;
                vec![Msg::AnswerDelivered(file)]
            }
            Cmd::ReadOrigin => vec![Msg::OriginRead(demo_origin())],
            Cmd::TakeForeground
            | Cmd::ActivateOwnTerminal
            | Cmd::QuietKernel
            | Cmd::InspectConsole
            | Cmd::AskShellStarting
            | Cmd::AskDisplayManagerState
            | Cmd::StayInFront => Vec::new(),
        }
    }

    fn subscribe(&mut self, subscription: &Sub) -> LocalBoxStream<'static, Msg> {
        let epoch = self.epoch;
        match subscription {
            Sub::BootFeed => {
                let boot = Rc::clone(&self.boot);
                let mut ticks = interval(FRAME_TICK);
                stream::poll_fn(move |context| ticks.poll_tick(context).map(|_| Some(())))
                    .flat_map(move |()| stream::iter(boot.borrow_mut().advance(demo_now(epoch))))
                    .boxed_local()
            }
            Sub::Keys => match key_inputs() {
                Ok(keys) => keys.boxed_local(),
                Err(error) => stream::once(async move { Msg::Failed(error.into()) }).boxed_local(),
            },
            Sub::FrameTicks => frame_inputs().boxed_local(),
            Sub::DeadlineAt(deadline) => {
                let remaining = deadline
                    .checked_sub(demo_now(epoch))
                    .unwrap_or(Duration::ZERO);
                stream::once(sleep(remaining).map(|()| Msg::DeadlinePassed)).boxed_local()
            }
            Sub::Termination
            | Sub::PasswordRequests
            | Sub::AbandonmentOf { .. }
            | Sub::TerminalSwitches
            | Sub::ResumeMarker
            | Sub::ConsoleWrites => stream::pending().boxed_local(),
        }
    }

    fn now(&self) -> Duration {
        demo_now(self.epoch)
    }
}

fn demo_origin() -> BootOrigin {
    BootOrigin {
        host: "demo".to_owned(),
        system: Some("NixOS 26.11.20261007.39ad350".to_owned()),
        generation: Some("42".to_owned()),
        kernel: "Linux 7.2.9".to_owned(),
    }
}

async fn play_scenario(terminal: &mut DefaultTerminal, scenario: &Scenario) -> io::Result<()> {
    let script = Script::begin(scenario.steps, 1);
    let mut boot = BootState::new(scenario.phase);
    script
        .opening_changes(scenario.describe, Duration::ZERO)
        .into_iter()
        .for_each(|change| boot.apply(change));
    let mut model = Model {
        boot,
        origin: demo_origin(),
        colors: Colors::default(),
        timing: Timing {
            keep_final_frame_for: FINAL_FRAME,
            ..Timing::default()
        },
        scope: Scope::System,
        prompt: Prompt::default(),
        console: ConsoleClaim::new(
            OwnConsole::Elsewhere,
            Foreground::Holding,
            MarkerPresence::Absent,
            ConsoleLook::default(),
        ),
        handover: Handover::Watching,
    };
    let mut runtime = DemoRuntime {
        boot: Rc::new(RefCell::new(DemoBoot {
            script,
            describe: scenario.describe,
            next_stage: scenario.after_switch_root,
            settled_at: None,
            listed: None,
            rejection_reported: false,
        })),
        epoch: monotonic_now(),
    };
    run_screen(&mut model, &mut runtime, terminal)
        .await
        .map_err(io::Error::other)?;
    match model.handover {
        Handover::Ended(WatchEnding::Failed(error)) => Err(io::Error::other(error)),
        Handover::Ended(_)
        | Handover::Watching
        | Handover::HandingOver { .. }
        | Handover::SettlingForShell { .. } => Ok(()),
    }
}

pub fn play(terminal: &mut DefaultTerminal, scenario: &Scenario) -> io::Result<()> {
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(play_scenario(terminal, scenario))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_DERIVATION: Duration = Duration::from_millis(500);
    const UNLOCK_ONLY: &[Step] = &[unlocking(
        "systemd-cryptsetup@root.service",
        &[],
        "Please enter passphrase for disk root:",
        KEY_DERIVATION,
    )];

    fn asking_demo() -> Script {
        let mut script = Script::begin(UNLOCK_ONLY, 1);
        script.advance(Duration::ZERO);
        script
    }

    #[test]
    fn the_hint_names_the_demo_passphrase() {
        assert!(DEMO_PASSPHRASE_HINT.contains(&format!("\"{DEMO_PASSPHRASE}\"")));
    }

    #[test]
    fn a_wrong_answer_is_checked_then_called_out() {
        let mut script = asking_demo();
        let asked = script.asked_request().expect("asking");
        assert_eq!(
            asked.hint,
            RequestHint::Shown(DEMO_PASSPHRASE_HINT.to_owned())
        );
        assert!(!script.was_rejected(Duration::ZERO));
        script.answer("wrong", Duration::ZERO);
        assert!(!script.was_rejected(KEY_DERIVATION / 2));
        assert!(script.was_rejected(KEY_DERIVATION));
    }

    #[test]
    fn the_right_answer_unlocks_and_closes_the_prompt() {
        let mut script = asking_demo();
        script.answer(DEMO_PASSPHRASE, Duration::ZERO);
        script.advance(KEY_DERIVATION);
        assert!(script.is_done());
        assert!(script.asked_request().is_none());
    }
}
