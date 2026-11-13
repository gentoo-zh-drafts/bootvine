use std::{future::ready, io, path::Path, rc::Rc, time::Duration};

use bootvine_screen::{
    boot_model::{BootState, ConsolePrompt, Phase, RootRequest, Scope},
    screen_components::{
        console_claim::{ConsoleClaim, Foreground, ForegroundOutcome, MarkerPresence, OwnConsole},
        handover::Handover,
        prompt::Prompt,
        screen_application::{Command, Message, Model, Subscription},
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
    service_manager::ServiceManager,
};
use futures::{FutureExt, StreamExt, stream, stream::LocalBoxStream};
use rustix::stdio::stdin;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::console_session::{
    console_takeover::{HeldTerminal, KernelConsole, StdinTerminal, console_look},
    monotonic_clock::monotonic_now,
    origin_reading::read_boot_origin,
    resume_console::{RESUME_CONSOLE_MARKER, resume_console_is_open},
    run_error::RunError,
    screen_loop::{Performed, ScreenRuntime},
    stream_sources::{
        ConsoleContentWatch, console_overwrite_inputs, frame_inputs, key_inputs,
        resume_marker_inputs, terminal_switch_inputs, termination_inputs,
    },
};

#[derive(Clone)]
enum ConsoleContents {
    Watched(Rc<ConsoleContentWatch>),
    Unwatched,
}

impl ConsoleContents {
    fn of(stdin_terminal: StdinTerminal) -> Self {
        let StdinTerminal::VirtualConsole(console_number) = stdin_terminal else {
            info!("not on a virtual console, not watching for writes by other programs");
            return Self::Unwatched;
        };
        Self::Watched(
            ConsoleContentWatch::open(console_number)
                .expect("the contents of bootvine's virtual console are readable"),
        )
    }
}

pub struct LiveSettings {
    pub scope: Scope,
    pub phase: Phase,
    pub target: RootRequest,
    pub colors: Colors,
    pub timing: Timing,
}

pub struct LiveRuntime<Manager: ServiceManager> {
    service_manager: Manager,
    scope: Scope,
    phase: Phase,
    target: RootRequest,
    stdin_terminal: StdinTerminal,
    held_terminal: HeldTerminal,
    kernel: KernelConsole,
    console_contents: ConsoleContents,
}

fn foreground_of(held_terminal: &HeldTerminal) -> Foreground {
    match held_terminal {
        HeldTerminal::Holding(_) => Foreground::Holding,
        HeldTerminal::StayingBehind => Foreground::StayingBehind,
    }
}

fn marker_presence() -> MarkerPresence {
    if resume_console_is_open(Path::new(RESUME_CONSOLE_MARKER)) {
        MarkerPresence::Present
    } else {
        MarkerPresence::Absent
    }
}

fn failure_stream(error: io::Error) -> LocalBoxStream<'static, Message> {
    stream::once(ready(Message::Failed(RunError::from(error).into()))).boxed_local()
}

impl<Manager: ServiceManager> LiveRuntime<Manager> {
    #[must_use]
    pub fn start(service_manager: Manager, settings: LiveSettings) -> (Self, Model) {
        let stdin_terminal = StdinTerminal::try_from(stdin())
            .expect("naming the terminal on stdin fails only when there is none");
        let (kernel, held_terminal) = match settings.scope {
            Scope::System if service_manager.console_prompt() == ConsolePrompt::Showing => {
                info!("a password prompt is already showing, staying behind it");
                (KernelConsole::quiet(), HeldTerminal::StayingBehind)
            }
            Scope::System => (KernelConsole::quiet(), HeldTerminal::take(stdin_terminal)),
            Scope::User => (KernelConsole::LeftAsIs, HeldTerminal::take(stdin_terminal)),
        };
        let own_console = match stdin_terminal {
            StdinTerminal::VirtualConsole(number) => OwnConsole::Virtual(number),
            StdinTerminal::OtherTerminal | StdinTerminal::NotATerminal => OwnConsole::Elsewhere,
        };
        let model = Model {
            boot: BootState::new(settings.phase, &service_manager.vocabulary()),
            origin: read_boot_origin(),
            colors: settings.colors,
            timing: settings.timing,
            scope: settings.scope,
            prompt: Prompt::default(),
            console: ConsoleClaim::new(
                own_console,
                foreground_of(&held_terminal),
                marker_presence(),
                console_look(stdin_terminal),
            ),
            handover: Handover::Watching,
        };
        let runtime = Self {
            service_manager,
            scope: settings.scope,
            phase: settings.phase,
            target: settings.target,
            stdin_terminal,
            held_terminal,
            kernel,
            console_contents: ConsoleContents::of(stdin_terminal),
        };
        (runtime, model)
    }
}

impl<Manager: ServiceManager> Drop for LiveRuntime<Manager> {
    fn drop(&mut self) {
        if self.service_manager.console_prompt() == ConsolePrompt::Showing {
            info!("leaving a password prompt to the console agent on this terminal");
            self.held_terminal.stay_switched();
        }
    }
}

impl<Manager: ServiceManager> ScreenRuntime for LiveRuntime<Manager> {
    fn perform(&mut self, command: Command) -> Performed {
        match command {
            Command::TakeForeground => {
                self.held_terminal = HeldTerminal::take(self.stdin_terminal);
                Performed::Now(vec![Message::ForegroundTaken(match self.held_terminal {
                    HeldTerminal::Holding(_) => ForegroundOutcome::Taken,
                    HeldTerminal::StayingBehind => ForegroundOutcome::Refused,
                })])
            }
            Command::ActivateOwnTerminal => {
                if let HeldTerminal::Holding(held) = &self.held_terminal {
                    let _ = held.insist();
                }
                Performed::Now(Vec::new())
            }
            Command::QuietKernel => {
                let _ = self.kernel.keep_quiet();
                Performed::Now(Vec::new())
            }
            Command::InspectConsole => Performed::Now(vec![Message::ConsoleInspected {
                kernel: self.kernel.level(),
                look: console_look(self.stdin_terminal),
            }]),
            Command::ReadOrigin => Performed::Now(vec![Message::OriginRead(read_boot_origin())]),
            Command::StayInFront => {
                self.held_terminal.stay_switched();
                Performed::Now(Vec::new())
            }
            Command::StepAside => {
                self.held_terminal = HeldTerminal::StayingBehind;
                Performed::Now(Vec::new())
            }
            Command::SendAnswer { file, answer } => {
                let service_manager = self.service_manager.clone();
                Performed::Later(
                    async move {
                        match service_manager.answer_password(answer).await {
                            Ok(()) => {
                                info!(path = %file.display(), "answered a password prompt");
                                vec![Message::AnswerDelivered(file)]
                            }
                            Err(error) => {
                                warn!(error = %error, "cannot answer the password prompt");
                                vec![Message::AnswerUndelivered(file)]
                            }
                        }
                    }
                    .boxed_local(),
                )
            }
            Command::AskShellStarting => {
                let service_manager = self.service_manager.clone();
                Performed::Later(
                    async move {
                        vec![Message::ShellQueryAnswered(
                            service_manager.shell_start().await,
                        )]
                    }
                    .boxed_local(),
                )
            }
            Command::AskDisplayManagerState => {
                let service_manager = self.service_manager.clone();
                Performed::Later(
                    async move {
                        vec![Message::DisplayManagerAnswered(
                            service_manager.display_manager().await,
                        )]
                    }
                    .boxed_local(),
                )
            }
        }
    }

    fn subscribe(&mut self, subscription: &Subscription) -> LocalBoxStream<'static, Message> {
        match subscription {
            Subscription::BootFeed => self
                .service_manager
                .boot_changes(self.phase, self.scope, self.target.clone())
                .map(|batch| match batch {
                    Ok(changes) => Message::Boot(changes),
                    Err(failure) => Message::Failed(failure),
                })
                .boxed_local(),
            Subscription::Keys => key_inputs().map_or_else(failure_stream, StreamExt::boxed_local),
            Subscription::Termination => {
                termination_inputs().map_or_else(failure_stream, StreamExt::boxed_local)
            }
            Subscription::ConsolePrompts => self
                .service_manager
                .console_prompts()
                .map(Message::ConsolePromptChanged)
                .boxed_local(),
            Subscription::PasswordRequests => self
                .service_manager
                .password_requests()
                .map(Message::PasswordRequestsListed)
                .boxed_local(),
            Subscription::AbandonmentOf {
                file,
                deadline,
                asker,
            } => {
                let file = file.clone();
                stream::once(
                    self.service_manager
                        .password_abandoned(*deadline, *asker)
                        .map(move |()| Message::PromptAbandoned(file)),
                )
                .boxed_local()
            }
            Subscription::TerminalSwitches => terminal_switch_inputs().boxed_local(),
            Subscription::ResumeMarker => resume_marker_inputs().boxed_local(),
            Subscription::ConsoleWrites => match self.console_contents.clone() {
                ConsoleContents::Watched(watch) => console_overwrite_inputs(watch).boxed_local(),
                ConsoleContents::Unwatched => stream::pending().boxed_local(),
            },
            Subscription::FrameTicks => frame_inputs().boxed_local(),
            Subscription::DeadlineAt(deadline) => {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "a deadline already behind the clock passes at once"
                )]
                let remaining = deadline.saturating_sub(monotonic_now());
                stream::once(sleep(remaining).map(|()| Message::DeadlinePassed)).boxed_local()
            }
        }
    }

    fn now(&self) -> Duration {
        monotonic_now()
    }

    fn drawn(&mut self) {
        if let ConsoleContents::Watched(watch) = &self.console_contents {
            watch.forget_own_writes();
        }
    }
}
