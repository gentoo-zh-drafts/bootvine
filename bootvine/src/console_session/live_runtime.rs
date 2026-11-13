use std::{future::ready, io, path::Path, rc::Rc, time::Duration};

use futures::{FutureExt, StreamExt, stream, stream::LocalBoxStream};
use rustix::stdio::stdin;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::{
    boot_model::{BootState, Phase},
    console_session::{
        ask_password::{self, awaited_requests, send_answer, until_abandoned, watch_requests},
        console_takeover::{HeldTerminal, KernelConsole, StdinTerminal, console_look},
        origin_reading::read_boot_origin,
        resume_console::{RESUME_CONSOLE_MARKER, resume_console_is_open},
        screen_loop::ScreenRuntime,
        stream_sources::{
            BACKGROUND_CHECK_INTERVAL, ConsoleContentWatch, DirectoryChange, DirectoryWatch,
            console_overwrite_inputs, frame_inputs, items_when_readable, key_inputs,
            periodic_inputs, resume_marker_inputs, terminal_switch_inputs, termination_inputs,
        },
    },
    screen_components::{
        console_claim::{ConsoleClaim, Foreground, ForegroundOutcome, MarkerPresence, OwnConsole},
        handover::{DisplayManager, Handover, ShellStart},
        prompt::Prompt,
        screen_app::{Cmd, Model, Msg, Sub},
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
    systemd_link::{
        boot_feed::{RootRequest, bus_inputs},
        monotonic_clock::monotonic_now,
        systemd_bus::{Scope, Session},
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

pub struct LiveRuntime {
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

fn failure_stream(error: io::Error) -> LocalBoxStream<'static, Msg> {
    stream::once(ready(Msg::Failed(error.into()))).boxed_local()
}

fn list_awaited_requests() -> Msg {
    Msg::PasswordRequestsListed(awaited_requests())
}

fn password_request_inputs() -> LocalBoxStream<'static, Msg> {
    let watch = match watch_requests() {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            info!("not allowed to watch password prompts, polling for them instead");
            return periodic_inputs(BACKGROUND_CHECK_INTERVAL, list_awaited_requests).boxed_local();
        }
        watched => watched.expect("the password request directory can be watched"),
    };
    let changes = items_when_readable(watch, DirectoryWatch::changed)
        .expect("the runtime accepts an inotify descriptor")
        .filter(|change| ready(*change == DirectoryChange::Changed))
        .map(|_| list_awaited_requests());
    stream::once(ready(list_awaited_requests()))
        .chain(changes)
        .boxed_local()
}

impl LiveRuntime {
    #[must_use]
    pub fn start(settings: LiveSettings) -> (Self, Model) {
        let stdin_terminal = StdinTerminal::try_from(stdin())
            .expect("naming the terminal on stdin fails only when there is none");
        let (kernel, held_terminal) = match settings.scope {
            Scope::System if ask_password::requested() => {
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
            boot: BootState::new(settings.phase),
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

impl ScreenRuntime for LiveRuntime {
    fn perform(&mut self, command: Cmd) -> Vec<Msg> {
        match command {
            Cmd::TakeForeground => {
                self.held_terminal = HeldTerminal::take(self.stdin_terminal);
                vec![Msg::ForegroundTaken(match self.held_terminal {
                    HeldTerminal::Holding(_) => ForegroundOutcome::Taken,
                    HeldTerminal::StayingBehind => ForegroundOutcome::Refused,
                })]
            }
            Cmd::ActivateOwnTerminal => {
                if let HeldTerminal::Holding(held) = &self.held_terminal {
                    let _ = held.insist();
                }
                Vec::new()
            }
            Cmd::QuietKernel => {
                let _ = self.kernel.keep_quiet();
                Vec::new()
            }
            Cmd::InspectConsole => vec![Msg::ConsoleInspected {
                kernel: self.kernel.level(),
                look: console_look(self.stdin_terminal),
            }],
            Cmd::ReadOrigin => vec![Msg::OriginRead(read_boot_origin())],
            Cmd::SendAnswer {
                file,
                socket,
                passphrase,
            } => match send_answer(&socket, &passphrase) {
                Ok(()) => {
                    info!(path = %file.display(), "answered a password prompt");
                    vec![Msg::AnswerDelivered(file)]
                }
                Err(error) => {
                    warn!(error = %error, "cannot answer the password prompt");
                    vec![Msg::AnswerUndelivered(file)]
                }
            },
            Cmd::AskShellStarting => {
                let starting = Session::open(Scope::System)
                    .and_then(|session| session.shell_is_starting())
                    .expect(
                        "the private socket of systemd answers while a shell is about to start",
                    );
                vec![Msg::ShellQueryAnswered(if starting {
                    ShellStart::Starting
                } else {
                    ShellStart::NotStarting
                })]
            }
            Cmd::AskDisplayManagerState => {
                let running = Session::open(Scope::System)
                    .and_then(|session| session.display_manager_running())
                    .expect("the private socket of systemd answers while the boot runs");
                vec![Msg::DisplayManagerAnswered(if running {
                    DisplayManager::Running
                } else {
                    DisplayManager::NotRunning
                })]
            }
            Cmd::StayInFront => {
                self.held_terminal.stay_switched();
                Vec::new()
            }
        }
    }

    fn subscribe(&mut self, subscription: &Sub) -> LocalBoxStream<'static, Msg> {
        match subscription {
            Sub::BootFeed => bus_inputs(self.phase, self.scope, self.target.clone()).boxed_local(),
            Sub::Keys => key_inputs().map_or_else(failure_stream, StreamExt::boxed_local),
            Sub::Termination => {
                termination_inputs().map_or_else(failure_stream, StreamExt::boxed_local)
            }
            Sub::PasswordRequests => password_request_inputs(),
            Sub::AbandonmentOf {
                file,
                deadline,
                asker,
            } => {
                let file = file.clone();
                stream::once(
                    until_abandoned(*deadline, *asker).map(move |()| Msg::PromptAbandoned(file)),
                )
                .boxed_local()
            }
            Sub::TerminalSwitches => terminal_switch_inputs().boxed_local(),
            Sub::ResumeMarker => resume_marker_inputs().boxed_local(),
            Sub::ConsoleWrites => match self.console_contents.clone() {
                ConsoleContents::Watched(watch) => console_overwrite_inputs(watch).boxed_local(),
                ConsoleContents::Unwatched => stream::pending().boxed_local(),
            },
            Sub::FrameTicks => frame_inputs().boxed_local(),
            Sub::DeadlineAt(deadline) => {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "a deadline already behind the clock passes at once"
                )]
                let remaining = deadline.saturating_sub(monotonic_now());
                stream::once(sleep(remaining).map(|()| Msg::DeadlinePassed)).boxed_local()
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
