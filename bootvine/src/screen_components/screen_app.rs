use std::{path::PathBuf, time::Duration};

use crossterm::event::Event;
use tracing::info;

use crate::{
    boot_model::{BootChange, BootState, boot_origin::BootOrigin},
    boot_view::{BootProjection, BootView, ShellHandoverSummary},
    console_session::{console_palette::ConsoleLook, passphrase_input::asks_to_quit},
    screen_components::{
        console_claim::{
            ConsoleClaim, ForegroundOutcome, KernelLevel, MarkerPresence, OwnConsole, Refresh,
            TerminalSwitch,
        },
        handover::{
            DisplayManager, Handover, ShellReadiness, ShellStart, WatchEnding, shell_readiness,
        },
        password_request::{PasswordRequest, PromptAsker, PromptDeadline},
        prompt::{AnswerState, Passphrase, Prompt, TypedKey},
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
    systemd_link::{link_error::Error, systemd_bus::Scope},
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
pub enum Msg {
    Boot(Vec<BootChange>),
    Tick,
    KeyPressed(Event),
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
    Failed(Error),
    Drawn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    TakeForeground,
    ActivateOwnTerminal,
    QuietKernel,
    InspectConsole,
    ReadOrigin,
    SendAnswer {
        file: PathBuf,
        socket: PathBuf,
        passphrase: Passphrase,
    },
    AskShellStarting,
    AskDisplayManagerState,
    StayInFront,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Sub {
    BootFeed,
    Keys,
    Termination,
    PasswordRequests,
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

pub enum ScreenView<'a> {
    Boot(BootView<'a>),
    ShellSummary(ShellHandoverSummary<'a>),
    Unchanged,
}

pub fn update(model: &mut Model, msg: Msg, now: Duration) -> Vec<Cmd> {
    match msg {
        Msg::Boot(changes) => apply_boot_changes(model, changes, now),
        Msg::Tick | Msg::AnswerUndelivered(_) => Vec::new(),
        Msg::KeyPressed(event) => press_key(model, &event),
        Msg::PasswordRequestsListed(listed) => follow_requests(model, listed),
        Msg::PromptAbandoned(file) => {
            model.prompt.abandon(&file);
            Vec::new()
        }
        Msg::AnswerDelivered(file) => {
            model.prompt.mark_answer(&file, AnswerState::Sent);
            Vec::new()
        }
        Msg::AnswerRejected(file) => {
            model.prompt.mark_answer(&file, AnswerState::Rejected);
            Vec::new()
        }
        Msg::ForegroundTaken(outcome) => {
            model.console.foreground_taken(outcome);
            Vec::new()
        }
        Msg::TerminalActivated(active) => switch_terminal(model, active),
        Msg::ResumeMarker(marker) => {
            model.console.marker_seen(marker);
            Vec::new()
        }
        Msg::ConsoleInspected { kernel, look } => match model.console.inspected(kernel, look) {
            KernelLevel::Raised => vec![Cmd::QuietKernel],
            KernelLevel::Quiet | KernelLevel::Untouched => Vec::new(),
        },
        Msg::ConsoleOverwritten => {
            model.console.request(Refresh::Repaint);
            Vec::new()
        }
        Msg::OriginRead(origin) => {
            model.origin = origin;
            Vec::new()
        }
        Msg::ShellQueryAnswered(ShellStart::Starting) => {
            info!("a shell is starting, leaving the failures above it");
            model.handover.settle_for_shell(now);
            finish_when_settled(model)
        }
        Msg::ShellQueryAnswered(ShellStart::NotStarting) => {
            model.handover.end(WatchEnding::ViewerLeft);
            Vec::new()
        }
        Msg::DisplayManagerAnswered(display_manager) => {
            display_manager_answered(model, display_manager)
        }
        Msg::DeadlinePassed => deadline_passed(model, now),
        Msg::Terminated => terminate(model),
        Msg::BootSourceEnded => {
            model.handover.end(WatchEnding::BootCompleted);
            Vec::new()
        }
        Msg::Failed(error) => {
            model.handover.end(WatchEnding::Failed(error));
            Vec::new()
        }
        Msg::Drawn => {
            model.console.refresh = Refresh::Steady;
            Vec::new()
        }
    }
}

fn apply_boot_changes(model: &mut Model, changes: Vec<BootChange>, now: Duration) -> Vec<Cmd> {
    let phase_before = model.boot.phase;
    changes
        .into_iter()
        .for_each(|change| model.boot.apply(change));
    let origin = (model.boot.phase != phase_before).then_some(Cmd::ReadOrigin);
    model.handover.follow_boot(&model.boot, &model.timing, now);
    origin
        .into_iter()
        .chain([Cmd::InspectConsole])
        .chain(finish_when_settled(model))
        .collect()
}

fn finish_when_settled(model: &mut Model) -> Vec<Cmd> {
    if model.handover.is_settling() && shell_readiness(&model.boot) == ShellReadiness::Settled {
        model.handover.end(WatchEnding::ShellTookOver);
        return vec![Cmd::StayInFront];
    }
    Vec::new()
}

fn press_key(model: &mut Model, event: &Event) -> Vec<Cmd> {
    if asks_to_quit(event, model.prompt.is_asking()) {
        model.handover.end(WatchEnding::ViewerLeft);
        return Vec::new();
    }
    match model.prompt.type_key(event) {
        TypedKey::Submitted {
            request,
            passphrase,
        } => vec![Cmd::SendAnswer {
            file: request.file,
            socket: request.socket,
            passphrase,
        }],
        TypedKey::Absorbed => Vec::new(),
    }
}

fn follow_requests(model: &mut Model, listed: Vec<PasswordRequest>) -> Vec<Cmd> {
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
        vec![Cmd::TakeForeground]
    } else {
        Vec::new()
    }
}

fn leave_console_to_another_program(model: &mut Model) -> Vec<Cmd> {
    info!("another program took the console, leaving it there");
    model.console.release();
    model.handover.end(WatchEnding::BootCompleted);
    vec![Cmd::StayInFront]
}

fn switch_terminal(model: &mut Model, active: u16) -> Vec<Cmd> {
    if model.console.terminal_switched(active) == TerminalSwitch::Ignored {
        return Vec::new();
    }
    match model.handover {
        Handover::Watching => vec![Cmd::AskDisplayManagerState],
        Handover::HandingOver { .. } => leave_console_to_another_program(model),
        Handover::SettlingForShell { .. } | Handover::Ended(_) => Vec::new(),
    }
}

fn display_manager_answered(model: &mut Model, display_manager: DisplayManager) -> Vec<Cmd> {
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
            vec![Cmd::ActivateOwnTerminal]
        }
    }
}

fn deadline_passed(model: &mut Model, now: Duration) -> Vec<Cmd> {
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
            vec![Cmd::StayInFront]
        }
        Handover::Watching | Handover::Ended(_) => Vec::new(),
    }
}

fn terminate(model: &mut Model) -> Vec<Cmd> {
    if model.scope == Scope::System
        && model.console.is_holding()
        && matches!(model.console.own_console, OwnConsole::Virtual(_))
        && model.handover.is_live()
    {
        return vec![Cmd::AskShellStarting];
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
pub fn subscriptions(model: &Model) -> Vec<Sub> {
    let animating =
        model.handover.is_live() && !model.console.is_lent() && !model.boot.is_complete();
    let system = model.scope == Scope::System;
    [
        Sub::BootFeed,
        Sub::Keys,
        Sub::Termination,
        Sub::TerminalSwitches,
        Sub::ConsoleWrites,
    ]
    .into_iter()
    .chain(system.then_some(Sub::PasswordRequests))
    .chain(system.then_some(Sub::ResumeMarker))
    .chain(model.prompt.asked().map(|asked| Sub::AbandonmentOf {
        file: asked.request.file.clone(),
        deadline: asked.request.not_after,
        asker: asked.request.asker,
    }))
    .chain(animating.then_some(Sub::FrameTicks))
    .chain(model.handover.deadline().map(Sub::DeadlineAt))
    .collect()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};

    use super::*;
    use crate::{
        boot_model::{Activity, JobKind, Phase},
        boot_test_fixtures::{MS, facts},
        console_session::console_palette::ConsoleLook,
        screen_components::{
            console_claim::{Foreground, MarkerPresence},
            handover::LATE_FAILURES_PATIENCE,
            password_request::tests::cryptsetup_request,
        },
    };

    fn watching_model() -> Model {
        Model {
            boot: BootState::new(Phase::Host),
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

    fn shell_job_queued() -> Msg {
        Msg::Boot(vec![
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

    fn press(code: KeyCode) -> Msg {
        Msg::KeyPressed(Event::Key(KeyEvent::from(code)))
    }

    #[test]
    fn a_shell_takes_over_once_its_start_job_is_in_the_model() {
        let mut model = watching_model();
        assert_eq!(
            update(&mut model, Msg::Terminated, MS),
            vec![Cmd::AskShellStarting]
        );
        assert_eq!(
            update(
                &mut model,
                Msg::ShellQueryAnswered(ShellStart::Starting),
                MS
            ),
            Vec::<Cmd>::new()
        );
        assert!(model.handover.is_settling());
        assert!(update(&mut model, shell_job_queued(), 2 * MS).contains(&Cmd::StayInFront));
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
            Msg::ShellQueryAnswered(ShellStart::Starting),
            MS,
        );
        assert_eq!(
            update(&mut model, Msg::DeadlinePassed, 2 * MS),
            Vec::<Cmd>::new()
        );
        assert_eq!(
            update(&mut model, Msg::DeadlinePassed, MS + LATE_FAILURES_PATIENCE),
            vec![Cmd::StayInFront]
        );
    }

    #[test]
    fn stopping_without_a_shell_leaves_at_once() {
        let mut model = watching_model();
        let _ = update(&mut model, Msg::Terminated, MS);
        let _ = update(
            &mut model,
            Msg::ShellQueryAnswered(ShellStart::NotStarting),
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
            update(&mut model, Msg::TerminalActivated(1), MS),
            vec![Cmd::AskDisplayManagerState]
        );
        assert_eq!(
            update(
                &mut model,
                Msg::DisplayManagerAnswered(DisplayManager::NotRunning),
                MS
            ),
            vec![Cmd::ActivateOwnTerminal]
        );
        assert_eq!(model.console.refresh, Refresh::Repaint);
        let _ = update(&mut model, Msg::TerminalActivated(1), 2 * MS);
        assert_eq!(
            update(
                &mut model,
                Msg::DisplayManagerAnswered(DisplayManager::Running),
                2 * MS
            ),
            vec![Cmd::StayInFront]
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
            update(&mut model, Msg::TerminalActivated(1), 2 * MS),
            vec![Cmd::StayInFront]
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
            Msg::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
            MS,
        );
        let _ = update(&mut model, press(KeyCode::Char('q')), MS);
        assert!(
            !model.handover.has_ended(),
            "q is typed into the prompt, not a quit"
        );
        assert_eq!(
            update(&mut model, press(KeyCode::Enter), MS),
            vec![Cmd::SendAnswer {
                file: PathBuf::from("ask.x"),
                socket: PathBuf::from("/run/systemd/ask-password/sck.1"),
                passphrase: Passphrase::from("q"),
            }]
        );
        let _ = update(&mut model, press(KeyCode::Esc), MS);
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
                Msg::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
                MS
            ),
            Vec::<Cmd>::new()
        );
        assert!(!model.prompt.is_asking());
        assert_eq!(
            update(&mut model, Msg::PasswordRequestsListed(Vec::new()), MS),
            vec![Cmd::TakeForeground]
        );
    }

    #[test]
    fn subscriptions_follow_the_model() {
        let mut model = watching_model();
        assert!(subscriptions(&model).contains(&Sub::FrameTicks));
        let _ = update(
            &mut model,
            Msg::PasswordRequestsListed(vec![cryptsetup_request("ask.x")]),
            MS,
        );
        assert!(
            subscriptions(&model)
                .iter()
                .any(|subscription| matches!(subscription, Sub::AbandonmentOf { .. }))
        );
        let _ = update(&mut model, Msg::ResumeMarker(MarkerPresence::Present), MS);
        assert!(!subscriptions(&model).contains(&Sub::FrameTicks));
        model.handover = Handover::HandingOver { deadline: 10 * MS };
        assert!(subscriptions(&model).contains(&Sub::DeadlineAt(10 * MS)));
    }
}
