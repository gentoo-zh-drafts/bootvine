use std::{fmt::Debug, hash::Hash, path::PathBuf, time::Duration};

use tracing::info;

use crate::{
    boot_model::{BootChange, BootState, ConsolePrompt, Scope, boot_origin::BootOrigin},
    boot_view::{BootProjection, BootView, ShellHandoverSummary, console_palette::ConsoleLook},
    screen_components::{
        console_claim::{
            ConsoleClaim, ForegroundOutcome, KernelLevel, MarkerPresence, OwnConsole, Refresh,
            TerminalSwitch,
        },
        handover::{
            DisplayManager, Handover, ShellReadiness, ShellStart, WatchEnding, WatchFailure,
            shell_readiness,
        },
        passphrase_input::{KeyPress, asks_to_quit},
        password_request::{PasswordAnswer, PasswordRequest, PromptAsker, PromptDeadline},
        prompt::{AnswerState, Prompt, TypedKey},
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
};

pub struct Model {
    pub boot: BootState,
    pub origin: BootOrigin,
    pub colors: Colors,
    pub timing: Timing,
    pub scope: Scope,
    pub prompt: Prompt,
    pub console: ConsoleClaim,
    pub handover: Handover,
}

#[derive(Debug)]
pub enum Message {
    Boot(Vec<BootChange>),
    Tick,
    KeyPressed(KeyPress),
    PasswordRequestsListed(Vec<PasswordRequest>),
    PromptAbandoned(PathBuf),
    AnswerDelivered(PathBuf),
    AnswerUndelivered(PathBuf),
    AnswerRejected(PathBuf),
    ForegroundTaken(ForegroundOutcome),
    TerminalActivated(u16),
    ResumeMarker(MarkerPresence),
    ConsoleInspected {
        kernel: KernelLevel,
        look: ConsoleLook,
    },
    ConsoleOverwritten,
    OriginRead(BootOrigin),
    ShellQueryAnswered(ShellStart),
    DisplayManagerAnswered(DisplayManager),
    DeadlinePassed,
    Terminated,
    BootSourceEnded,
    Failed(WatchFailure),
    ConsolePromptChanged(ConsolePrompt),
    Drawn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    TakeForeground,
    ActivateOwnTerminal,
    QuietKernel,
    InspectConsole,
    ReadOrigin,
    SendAnswer {
        file: PathBuf,
        answer: PasswordAnswer,
    },
    AskShellStarting,
    AskDisplayManagerState,
    StayInFront,
    StepAside,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subscription {
    BootFeed,
    Keys,
    Termination,
    PasswordRequests,
    ConsolePrompts,
    AbandonmentOf {
        file: PathBuf,
        deadline: PromptDeadline,
        asker: PromptAsker,
    },
    TerminalSwitches,
    ResumeMarker,
    ConsoleWrites,
    FrameTicks,
    DeadlineAt(Duration),
}

const _: () = {
    fn plain_data<Value: Clone + Eq + Debug>() {}
    fn diffable<Value: Clone + Eq + Hash + Debug>() {}
    let _ = plain_data::<Command>;
    let _ = diffable::<Subscription>;
};

pub enum ScreenView<'a> {
    Boot(BootView<'a>),
    ShellSummary(ShellHandoverSummary<'a>),
    Unchanged,
}

pub fn update(model: &mut Model, message: Message, now: Duration) -> Vec<Command> {
    match message {
        Message::Boot(changes) => apply_boot_changes(model, changes, now),
        Message::Tick | Message::AnswerUndelivered(_) => Vec::new(),
        Message::KeyPressed(key) => press_key(model, key),
        Message::PasswordRequestsListed(listed) => follow_requests(model, listed),
        Message::PromptAbandoned(file) => {
            model.prompt.abandon(&file);
            Vec::new()
        }
        Message::AnswerDelivered(file) => {
            model.prompt.mark_answer(&file, AnswerState::Sent);
            Vec::new()
        }
        Message::AnswerRejected(file) => {
            model.prompt.mark_answer(&file, AnswerState::Rejected);
            Vec::new()
        }
        Message::ForegroundTaken(outcome) => {
            model.console.foreground_taken(outcome);
            Vec::new()
        }
        Message::TerminalActivated(active) => switch_terminal(model, active),
        Message::ResumeMarker(marker) => {
            model.console.marker_seen(marker);
            Vec::new()
        }
        Message::ConsoleInspected { kernel, look } => match model.console.inspected(kernel, look) {
            KernelLevel::Raised => vec![Command::QuietKernel],
            KernelLevel::Quiet | KernelLevel::Untouched => Vec::new(),
        },
        Message::ConsoleOverwritten => {
            model.console.request(Refresh::Repaint);
            Vec::new()
        }
        Message::ConsolePromptChanged(prompt) => follow_console_prompt(model, prompt),
        Message::OriginRead(origin) => {
            model.origin = origin;
            Vec::new()
        }
        Message::ShellQueryAnswered(ShellStart::Starting) => {
            info!("a shell is starting, leaving the failures above it");
            model.handover.settle_for_shell(now);
            finish_when_settled(model)
        }
        Message::ShellQueryAnswered(ShellStart::NotStarting) => {
            model.handover.end(WatchEnding::ViewerLeft);
            Vec::new()
        }
        Message::DisplayManagerAnswered(display_manager) => {
            display_manager_answered(model, display_manager)
        }
        Message::DeadlinePassed => deadline_passed(model, now),
        Message::Terminated => terminate(model),
        Message::BootSourceEnded => {
            model.handover.end(WatchEnding::BootCompleted);
            Vec::new()
        }
        Message::Failed(error) => {
            model.handover.end(WatchEnding::Failed(error));
            Vec::new()
        }
        Message::Drawn => {
            model.console.refresh = Refresh::Steady;
            Vec::new()
        }
    }
}

fn apply_boot_changes(model: &mut Model, changes: Vec<BootChange>, now: Duration) -> Vec<Command> {
    let phase_before = model.boot.phase;
    changes
        .into_iter()
        .for_each(|change| model.boot.apply(change));
    let origin = (model.boot.phase != phase_before).then_some(Command::ReadOrigin);
    model.handover.follow_boot(&model.boot, &model.timing, now);
    origin
        .into_iter()
        .chain([Command::InspectConsole])
        .chain(finish_when_settled(model))
        .collect()
}

fn finish_when_settled(model: &mut Model) -> Vec<Command> {
    if model.handover.is_settling() && shell_readiness(&model.boot) == ShellReadiness::Settled {
        model.handover.end(WatchEnding::ShellTookOver);
        return vec![Command::StayInFront];
    }
    Vec::new()
}

fn press_key(model: &mut Model, key: KeyPress) -> Vec<Command> {
    if asks_to_quit(key, model.prompt.is_asking()) {
        model.handover.end(WatchEnding::ViewerLeft);
        return Vec::new();
    }
    match model.prompt.type_key(key) {
        TypedKey::Submitted {
            request,
            passphrase,
        } => vec![Command::SendAnswer {
            file: request.file,
            answer: PasswordAnswer {
                socket: request.socket,
                passphrase,
            },
        }],
        TypedKey::Absorbed => Vec::new(),
    }
}

fn follow_requests(model: &mut Model, listed: Vec<PasswordRequest>) -> Vec<Command> {
    if model.scope != Scope::System {
        return Vec::new();
    }
    let comes_forward = model.console.comes_forward(listed.len());
    if model.console.is_holding() {
        model.prompt.follow(listed);
    } else {
        model.prompt.hide(listed);
    }
    if comes_forward {
        vec![Command::TakeForeground]
    } else {
        Vec::new()
    }
}

fn follow_console_prompt(model: &mut Model, prompt: ConsolePrompt) -> Vec<Command> {
    match prompt {
        ConsolePrompt::Showing if model.console.is_holding() => {
            info!("another program is prompting on the console, stepping aside");
            model.console.release();
            vec![Command::StepAside]
        }
        ConsolePrompt::Absent if model.console.comes_forward(0) => vec![Command::TakeForeground],
        ConsolePrompt::Showing | ConsolePrompt::Absent => Vec::new(),
    }
}

fn leave_console_to_another_program(model: &mut Model) -> Vec<Command> {
    info!("another program took the console, leaving it there");
    model.console.release();
    model.handover.end(WatchEnding::BootCompleted);
    vec![Command::StayInFront]
}

fn switch_terminal(model: &mut Model, active: u16) -> Vec<Command> {
    if model.console.terminal_switched(active) == TerminalSwitch::Ignored {
        return Vec::new();
    }
    match model.handover {
        Handover::Watching => vec![Command::AskDisplayManagerState],
        Handover::HandingOver { .. } => leave_console_to_another_program(model),
        Handover::SettlingForShell { .. } | Handover::Ended(_) => Vec::new(),
    }
}

fn display_manager_answered(model: &mut Model, display_manager: DisplayManager) -> Vec<Command> {
    if !matches!(
        model.handover,
        Handover::Watching | Handover::HandingOver { .. }
    ) {
        return Vec::new();
    }
    match display_manager {
        DisplayManager::Running => leave_console_to_another_program(model),
        DisplayManager::NotRunning => {
            model.console.reclaimed();
            vec![Command::ActivateOwnTerminal]
        }
    }
}

fn deadline_passed(model: &mut Model, now: Duration) -> Vec<Command> {
    if model
        .handover
        .deadline()
        .is_none_or(|deadline| now < deadline)
    {
        return Vec::new();
    }
    match model.handover {
        Handover::HandingOver { .. } => {
            info!("nothing took the console, switching back");
            model.handover.end(WatchEnding::BootCompleted);
            Vec::new()
        }
        Handover::SettlingForShell { .. } => {
            model.handover.end(WatchEnding::ShellTookOver);
            vec![Command::StayInFront]
        }
        Handover::Watching | Handover::Ended(_) => Vec::new(),
    }
}

fn terminate(model: &mut Model) -> Vec<Command> {
    if model.scope == Scope::System
        && model.console.is_holding()
        && matches!(model.console.own_console, OwnConsole::Virtual(_))
        && model.handover.is_live()
    {
        return vec![Command::AskShellStarting];
    }
    model.handover.end(WatchEnding::ViewerLeft);
    Vec::new()
}

#[must_use]
pub fn view<'a>(model: &'a Model, projection: &'a BootProjection, now: Duration) -> ScreenView<'a> {
    let glyphs = &model.console.look.glyphs;
    match &model.handover {
        Handover::Ended(WatchEnding::ShellTookOver) => {
            ScreenView::ShellSummary(ShellHandoverSummary {
                state: &model.boot,
                colors: &model.colors,
                glyphs,
                now,
                origin: &model.origin,
            })
        }
        Handover::Watching | Handover::HandingOver { .. } if !model.console.is_lent() => {
            ScreenView::Boot(BootView {
                state: &model.boot,
                projection,
                colors: &model.colors,
                timing: &model.timing,
                glyphs,
                now,
                origin: &model.origin,
                password_prompt: model.prompt.view(),
            })
        }
        Handover::Watching
        | Handover::HandingOver { .. }
        | Handover::SettlingForShell { .. }
        | Handover::Ended(_) => ScreenView::Unchanged,
    }
}

#[must_use]
pub fn subscriptions(model: &Model) -> Vec<Subscription> {
    let animating =
        model.handover.is_live() && !model.console.is_lent() && !model.boot.is_complete();
    let system = model.scope == Scope::System;
    [
        Subscription::BootFeed,
        Subscription::Keys,
        Subscription::Termination,
        Subscription::TerminalSwitches,
        Subscription::ConsoleWrites,
    ]
    .into_iter()
    .chain(system.then_some(Subscription::PasswordRequests))
    .chain(system.then_some(Subscription::ConsolePrompts))
    .chain(system.then_some(Subscription::ResumeMarker))
    .chain(
        model
            .prompt
            .asked()
            .map(|asked| Subscription::AbandonmentOf {
                file: asked.request.file.clone(),
                deadline: asked.request.not_after,
                asker: asked.request.asker,
            }),
    )
    .chain(animating.then_some(Subscription::FrameTicks))
    .chain(model.handover.deadline().map(Subscription::DeadlineAt))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        boot_model::{Activity, JobKind, Phase},
        boot_test_fixtures::{MS, facts, test_vocabulary},
        boot_view::console_palette::ConsoleLook,
        screen_components::{
            console_claim::{Foreground, MarkerPresence},
            handover::LATE_FAILURES_PATIENCE,
            password_request::tests::cryptsetup_request,
            prompt::Passphrase,
        },
    };

    fn watching_model() -> Model {
        Model {
            boot: BootState::new(Phase::Host, &test_vocabulary()),
            origin: BootOrigin::default(),
            colors: Colors::default(),
            timing: Timing::default(),
            scope: Scope::System,
            prompt: Prompt::default(),
            console: ConsoleClaim::new(
                OwnConsole::Virtual(12),
                Foreground::Holding,
                MarkerPresence::Absent,
                ConsoleLook::default(),
            ),
            handover: Handover::Watching,
        }
    }

    #[test]
    fn a_console_prompt_by_another_program_sends_the_screen_behind_until_it_ends() {
        let mut model = watching_model();
        assert_eq!(
            update(
                &mut model,
                Message::ConsolePromptChanged(ConsolePrompt::Showing),
                MS
            ),
            vec![Command::StepAside]
        );
        assert!(!model.console.is_holding());
        assert_eq!(
            update(
                &mut model,
                Message::ConsolePromptChanged(ConsolePrompt::Showing),
                MS
            ),
            Vec::new()
        );
        assert_eq!(
            update(
                &mut model,
                Message::ConsolePromptChanged(ConsolePrompt::Absent),
                MS
            ),
            vec![Command::TakeForeground]
        );
    }

    fn shell_job_queued() -> Message {
        Message::Boot(vec![
            BootChange::TrackUnit {
                facts: facts("emergency.service", &[]),
                activity: Activity::default(),
            },
            BootChange::QueueJob {
                id: 90,
                unit: "emergency.service".to_owned(),
                kind: JobKind::BringsUp,
                at: 2000 * MS,
            },
        ])
    }

    fn press(key: KeyPress) -> Message {
        Message::KeyPressed(key)
    }

    #[test]
    fn a_shell_takes_over_once_its_start_job_is_in_the_model() {
        let mut model = watching_model();
        assert_eq!(
            update(&mut model, Message::Terminated, MS),
            vec![Command::AskShellStarting]
        );
        assert_eq!(
            update(
                &mut model,
                Message::ShellQueryAnswered(ShellStart::Starting),
                MS
            ),
            Vec::<Command>::new()
        );
        assert!(model.handover.is_settling());
        assert!(update(&mut model, shell_job_queued(), 2 * MS).contains(&Command::StayInFront));
        assert!(matches!(
            model.handover,
            Handover::Ended(WatchEnding::ShellTookOver)
        ));
    }

    #[test]
    fn a_shell_that_never_shows_up_takes_over_at_the_deadline() {
        let mut model = watching_model();
        let _ = update(
            &mut model,
            Message::ShellQueryAnswered(ShellStart::Starting),
            MS,
        );
        assert_eq!(
            update(&mut model, Message::DeadlinePassed, 2 * MS),
            Vec::<Command>::new()
        );
        assert_eq!(
            update(
                &mut model,
                Message::DeadlinePassed,
                MS + LATE_FAILURES_PATIENCE
            ),
            vec![Command::StayInFront]
        );
    }

    #[test]
    fn stopping_without_a_shell_leaves_at_once() {
        let mut model = watching_model();
        let _ = update(&mut model, Message::Terminated, MS);
        let _ = update(
            &mut model,
            Message::ShellQueryAnswered(ShellStart::NotStarting),
            MS,
        );
        assert!(matches!(
            model.handover,
            Handover::Ended(WatchEnding::ViewerLeft)
        ));
    }

    #[test]
    fn another_terminal_is_reclaimed_while_watching_unless_a_display_manager_runs() {
        let mut model = watching_model();
        assert_eq!(
            update(&mut model, Message::TerminalActivated(1), MS),
            vec![Command::AskDisplayManagerState]
        );
        assert_eq!(
            update(
                &mut model,
                Message::DisplayManagerAnswered(DisplayManager::NotRunning),
                MS
            ),
            vec![Command::ActivateOwnTerminal]
        );
        assert_eq!(model.console.refresh, Refresh::Repaint);
        let _ = update(&mut model, Message::TerminalActivated(1), 2 * MS);
        assert_eq!(
            update(
                &mut model,
                Message::DisplayManagerAnswered(DisplayManager::Running),
                2 * MS
            ),
            vec![Command::StayInFront]
        );
        assert!(matches!(
            model.handover,
            Handover::Ended(WatchEnding::BootCompleted)
        ));
    }

    #[test]
    fn a_terminal_taken_while_handing_over_is_left_to_the_display_manager() {
        let mut model = watching_model();
        model.handover = Handover::HandingOver { deadline: 10 * MS };
        assert_eq!(
            update(&mut model, Message::TerminalActivated(1), 2 * MS),
            vec![Command::StayInFront]
        );
        assert!(matches!(
            model.handover,
            Handover::Ended(WatchEnding::BootCompleted)
        ));
    }

    #[test]
    fn a_typed_passphrase_is_sent_to_the_requests_socket() {
        let mut model = watching_model();
        let _ = update(
            &mut model,
            Message::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
            MS,
        );
        let _ = update(&mut model, press(KeyPress::Character('q')), MS);
        assert!(
            !model.handover.has_ended(),
            "q is typed into the prompt, not a quit"
        );
        assert_eq!(
            update(&mut model, press(KeyPress::Enter), MS),
            vec![Command::SendAnswer {
                file: PathBuf::from("ask.x"),
                answer: PasswordAnswer {
                    socket: PathBuf::from("/run/systemd/ask-password/sck.1"),
                    passphrase: Passphrase::from("q"),
                },
            }]
        );
        let _ = update(&mut model, press(KeyPress::Escape), MS);
        assert!(matches!(
            model.handover,
            Handover::Ended(WatchEnding::ViewerLeft)
        ));
    }

    #[test]
    fn bootvine_comes_forward_once_another_agents_prompt_is_gone() {
        let mut model = watching_model();
        model.console.release();
        assert_eq!(
            update(
                &mut model,
                Message::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
                MS
            ),
            Vec::<Command>::new()
        );
        assert!(!model.prompt.is_asking());
        assert_eq!(
            update(&mut model, Message::PasswordRequestsListed(Vec::new()), MS),
            vec![Command::TakeForeground]
        );
    }

    #[test]
    fn subscriptions_follow_the_model() {
        let mut model = watching_model();
        assert!(subscriptions(&model).contains(&Subscription::FrameTicks));
        let _ = update(
            &mut model,
            Message::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
            MS,
        );
        assert!(
            subscriptions(&model)
                .iter()
                .any(|subscription| matches!(subscription, Subscription::AbandonmentOf { .. }))
        );
        let _ = update(
            &mut model,
            Message::ResumeMarker(MarkerPresence::Present),
            MS,
        );
        assert!(!subscriptions(&model).contains(&Subscription::FrameTicks));
        model.handover = Handover::HandingOver { deadline: 10 * MS };
        assert!(subscriptions(&model).contains(&Subscription::DeadlineAt(10 * MS)));
    }
}
