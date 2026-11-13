use std::{
    future::ready,
    io, mem,
    ops::ControlFlow,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use crossterm::event::Event;
use futures::{Stream, StreamExt, channel::mpsc::UnboundedSender, stream};
use rustix::stdio::stdin;
use tracing::{info, warn};

use crate::{
    boot_model::BootChange,
    boot_view::PasswordPrompt,
    console_session::{
        ask_password::{self, PasswordEntry},
        console_palette::ConsoleLook,
        console_takeover::{
            Foreground, KernelConsole, KernelLoudness, StdinTerminal, VtOwnership, console_look,
        },
        passphrase_input::asks_to_quit,
        resume_console::{RESUME_CONSOLE_MARKER, resume_console_is_open},
        stream_sources::{
            ConsoleContentWatch, DirectoryChange, DirectoryWatch, console_overwrite_inputs,
            items_when_readable,
        },
        watch_screen::{ConsoleDesk, Refresh, ScreenInput, WatchEnding},
    },
    systemd_link::systemd_bus::{Scope, Session},
};

pub struct LiveDesk {
    quiet_kernel: KernelConsole,
    foreground: Foreground,
    scope: Scope,
    stdin_terminal: StdinTerminal,
    look: ConsoleLook,
    entry: Option<PasswordEntry>,
    handover_deadline: Option<Duration>,
    resume_marker: PathBuf,
    console_lent: bool,
    console_was_lent: bool,
    prompt_checks: Option<UnboundedSender<()>>,
    watched_request: Option<PathBuf>,
    console_contents: ConsoleContents,
}

#[derive(Clone)]
pub enum ConsoleContents {
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

    pub fn overwrite_inputs(self) -> impl Stream<Item = ScreenInput> {
        match self {
            Self::Watched(watch) => console_overwrite_inputs(watch).left_stream(),
            Self::Unwatched => stream::pending().right_stream(),
        }
    }
}

impl LiveDesk {
    #[must_use]
    pub fn take_console(scope: Scope, prompt_checks: Option<UnboundedSender<()>>) -> Self {
        let stdin_terminal = StdinTerminal::try_from(stdin())
            .expect("naming the terminal on stdin fails only when there is none");
        let (quiet_kernel, foreground) = match scope {
            Scope::System if ask_password::requested() => {
                info!("a password prompt is already showing, staying behind it");
                (KernelConsole::quiet(), Foreground::StayingBehind)
            }
            Scope::System => (KernelConsole::quiet(), Foreground::take(stdin_terminal)),
            Scope::User => (KernelConsole::LeftAsIs, Foreground::take(stdin_terminal)),
        };
        Self {
            quiet_kernel,
            foreground,
            scope,
            stdin_terminal,
            look: console_look(stdin_terminal),
            entry: None,
            handover_deadline: None,
            resume_marker: PathBuf::from(RESUME_CONSOLE_MARKER),
            console_lent: resume_console_is_open(Path::new(RESUME_CONSOLE_MARKER)),
            console_was_lent: false,
            prompt_checks,
            watched_request: None,
            console_contents: ConsoleContents::of(stdin_terminal),
        }
    }

    #[must_use]
    pub fn console_contents(&self) -> ConsoleContents {
        self.console_contents.clone()
    }

    fn watch_entry_until_abandoned(&mut self) {
        let current_request = self
            .entry
            .as_ref()
            .map(|entry| entry.request_file().to_owned());
        if current_request == self.watched_request {
            return;
        }
        if let (Some(entry), Some(prompt_checks)) = (&self.entry, &self.prompt_checks) {
            let abandoned = entry.until_abandoned();
            let prompt_checks = prompt_checks.clone();
            tokio::spawn(async move {
                abandoned.await;
                let _ = prompt_checks.unbounded_send(());
            });
        }
        self.watched_request = current_request;
    }

    fn come_forward_once_prompts_are_gone(&mut self) -> Refresh {
        if self.scope != Scope::System
            || matches!(self.foreground, Foreground::Holding(_))
            || ask_password::requested()
        {
            return Refresh::Steady;
        }
        self.foreground = Foreground::take(self.stdin_terminal);
        match self.foreground {
            Foreground::Holding(_) => {
                info!("password prompt answered, taking the console back");
                Refresh::Clear
            }
            Foreground::StayingBehind => Refresh::Steady,
        }
    }

    fn follow_requests(&mut self, requests_changed: bool) -> Refresh {
        let refresh = if requests_changed {
            self.come_forward_once_prompts_are_gone()
        } else {
            Refresh::Steady
        };
        self.entry =
            if self.scope == Scope::System && matches!(self.foreground, Foreground::Holding(_)) {
                PasswordEntry::follow(self.entry.take(), requests_changed)
            } else {
                None
            };
        self.watch_entry_until_abandoned();
        refresh
    }
}

impl ConsoleDesk for LiveDesk {
    fn look(&self) -> ConsoleLook {
        self.look
    }

    fn prompt(&self, _now: Duration) -> Option<PasswordPrompt<'_>> {
        self.entry.as_ref().map(PasswordEntry::prompt)
    }

    fn press_key(&mut self, event: &Event, _now: Duration) -> ControlFlow<WatchEnding> {
        let Some(entry) = &mut self.entry else {
            return if asks_to_quit(event, false) {
                ControlFlow::Break(WatchEnding::ViewerLeft)
            } else {
                ControlFlow::Continue(())
            };
        };
        if let Err(error) = entry.type_key(event) {
            warn!(error = %error, "cannot answer the password prompt");
        }
        ControlFlow::Continue(())
    }

    fn begin_handover(&mut self, reached: &str, deadline: Duration) {
        if self.handover_deadline.is_some() {
            return;
        }
        info!(unit = reached, "reached, handing the console over");
        self.handover_deadline = Some(deadline);
    }

    fn frame_passed(&mut self, now: Duration) -> ControlFlow<WatchEnding, Vec<BootChange>> {
        let Some(deadline) = self.handover_deadline else {
            return ControlFlow::Continue(Vec::new());
        };
        if matches!(&self.foreground, Foreground::Holding(foreground) if foreground.ownership() == VtOwnership::TakenAway)
        {
            info!("another program took the console, leaving it there");
            self.foreground.stay_switched();
            ControlFlow::Break(WatchEnding::BootCompleted)
        } else if now >= deadline {
            info!("nothing took the console, switching back");
            ControlFlow::Break(WatchEnding::BootCompleted)
        } else {
            ControlFlow::Continue(Vec::new())
        }
    }

    fn password_requests_changed(&mut self) -> Refresh {
        self.follow_requests(true)
    }

    fn check_prompts(&mut self) {
        self.follow_requests(false);
    }

    fn shell_takes_over(&mut self) -> bool {
        if self.scope != Scope::System || matches!(self.foreground, Foreground::StayingBehind) {
            return false;
        }
        Session::open(Scope::System)
            .and_then(|session| session.shell_is_starting())
            .expect("the private socket of systemd answers while a shell is about to start")
    }

    fn stay_in_front(&mut self) {
        self.foreground.stay_switched();
    }

    fn forget_own_writes(&mut self) {
        if let ConsoleContents::Watched(watch) = &self.console_contents {
            watch.forget_own_writes();
        }
    }

    fn lends_console(&mut self) -> bool {
        self.console_lent
    }

    fn keep_console_up(&mut self) -> Refresh {
        self.console_lent = resume_console_is_open(&self.resume_marker);
        self.console_was_lent |= self.console_lent;
        if self.console_lent {
            return Refresh::Steady;
        }
        let console_returned = mem::take(&mut self.console_was_lent);
        let kernel_got_loud = self.quiet_kernel.keep_quiet() == KernelLoudness::QuietedAgain;
        let terminal_was_taken = self.handover_deadline.is_none()
            && matches!(&self.foreground, Foreground::Holding(foreground) if foreground.insist() == VtOwnership::TakenAway);
        let look = console_look(self.stdin_terminal);
        let restyled = look != self.look;
        self.look = look;
        if console_returned || kernel_got_loud || terminal_was_taken || restyled {
            Refresh::Repaint
        } else {
            Refresh::Steady
        }
    }
}

pub fn password_request_inputs(
    watch: DirectoryWatch,
) -> io::Result<impl Stream<Item = ScreenInput>> {
    Ok(items_when_readable(watch, DirectoryWatch::changed)?
        .filter(|change| ready(*change == DirectoryChange::Changed))
        .map(|_| ScreenInput::PasswordRequests))
}

#[cfg(test)]
mod tests {
    use std::{env, fs, process};

    use super::*;

    fn desk_watching(resume_marker: PathBuf) -> LiveDesk {
        LiveDesk {
            quiet_kernel: KernelConsole::LeftAsIs,
            foreground: Foreground::StayingBehind,
            scope: Scope::System,
            stdin_terminal: StdinTerminal::NotATerminal,
            look: console_look(StdinTerminal::NotATerminal),
            entry: None,
            handover_deadline: None,
            resume_marker,
            console_lent: false,
            console_was_lent: false,
            prompt_checks: None,
            watched_request: None,
            console_contents: ConsoleContents::Unwatched,
        }
    }

    #[test]
    fn the_console_is_repainted_only_after_it_was_lent_for_resume() {
        let marker = env::temp_dir().join(format!("bootvine-resume-marker-{}", process::id()));
        let mut desk = desk_watching(marker.clone());
        assert!(!desk.lends_console());

        fs::write(&marker, "1").expect("create the marker");
        assert_eq!(desk.keep_console_up(), Refresh::Steady);
        assert!(desk.lends_console());

        fs::remove_file(&marker).expect("remove the marker");
        assert_eq!(desk.keep_console_up(), Refresh::Repaint);
        assert!(!desk.lends_console());
        assert_eq!(desk.keep_console_up(), Refresh::Steady);
    }
}
