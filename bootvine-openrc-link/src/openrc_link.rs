use std::{
    future::{Future, pending, ready},
    io,
};

use bootvine_screen::{
    boot_model::{
        BootChange, BootProgress, ConsolePrompt, ManagerVocabulary, Phase, RootRequest, RunRequest,
        Scope,
    },
    screen_components::{
        handover::{DisplayManager, ShellStart, StatusRelease, WatchFailure},
        password_request::{PasswordAnswer, PasswordRequest, PromptAsker, PromptDeadline},
    },
    service_manager::ServiceManager,
};
use futures::{Stream, StreamExt, stream};
use tracing::warn;

use crate::{
    boot_feed::boot_change_stream,
    openrc_error::OpenrcError,
    openrc_vocabulary::{BOOT_RUNLEVELS, HANDOFF_SERVICE, PROMPTING_SERVICES, openrc_vocabulary},
    state_directory::{
        OpenrcPaths, Runlevel, RunlevelChange, ServiceState, StateSnapshot, read_snapshot,
        state_changes,
    },
};

#[derive(Clone, Debug, Default)]
pub struct OpenrcLink {
    paths: OpenrcPaths,
}

impl From<&StateSnapshot> for ConsolePrompt {
    fn from(snapshot: &StateSnapshot) -> Self {
        if PROMPTING_SERVICES
            .iter()
            .any(|service| snapshot.state_of(service) == ServiceState::Starting)
        {
            Self::Showing
        } else {
            Self::Absent
        }
    }
}

impl From<&StateSnapshot> for BootProgress {
    fn from(snapshot: &StateSnapshot) -> Self {
        let last_runlevel = BOOT_RUNLEVELS[BOOT_RUNLEVELS.len() - 1];
        match (&snapshot.runlevel, snapshot.change) {
            (Runlevel::Named(runlevel), RunlevelChange::Settled) if runlevel == last_runlevel => {
                Self::Finished
            }
            _ => Self::InProgress,
        }
    }
}

impl ServiceManager for OpenrcLink {
    type Failure = OpenrcError;

    fn vocabulary(&self) -> ManagerVocabulary {
        openrc_vocabulary()
    }

    fn starting_phase(&self, scope: Scope, request: RunRequest) -> Phase {
        match (request, scope) {
            (RunRequest::Shutdown, _) => Phase::Shutdown,
            (RunRequest::Boot, Scope::User) => Phase::User,
            (RunRequest::Boot, Scope::System) => Phase::Host,
        }
    }

    fn boot_progress(&self) -> impl Future<Output = BootProgress> {
        ready(BootProgress::from(&read_snapshot(&self.paths)))
    }

    fn boot_changes(
        &self,
        phase: Phase,
        _scope: Scope,
        _root: RootRequest,
    ) -> impl Stream<Item = Result<Vec<BootChange>, WatchFailure>> + 'static {
        boot_change_stream(self.paths.clone(), phase)
    }

    fn password_requests(&self) -> impl Stream<Item = Vec<PasswordRequest>> + 'static {
        stream::pending()
    }

    fn password_abandoned(
        &self,
        _deadline: PromptDeadline,
        _asker: PromptAsker,
    ) -> impl Future<Output = ()> + 'static {
        pending()
    }

    fn console_prompt(&self) -> ConsolePrompt {
        ConsolePrompt::from(&read_snapshot(&self.paths))
    }

    fn console_prompts(&self) -> impl Stream<Item = ConsolePrompt> + 'static {
        let paths = self.paths.clone();
        match state_changes(&paths) {
            Err(error) => {
                warn!(error = %error, "cannot watch OpenRC for console prompts");
                stream::pending().left_stream()
            }
            Ok(state_change_events) => state_change_events
                .map(move |()| ConsolePrompt::from(&read_snapshot(&paths)))
                .right_stream(),
        }
    }

    fn answer_password(&self, _answer: PasswordAnswer) -> impl Future<Output = io::Result<()>> {
        ready(Err(io::ErrorKind::Unsupported.into()))
    }

    fn shell_start(&self) -> impl Future<Output = ShellStart> {
        ready(ShellStart::NotStarting)
    }

    fn display_manager(&self) -> impl Future<Output = DisplayManager> {
        ready(match read_snapshot(&self.paths).state_of(HANDOFF_SERVICE) {
            ServiceState::Starting | ServiceState::Started => DisplayManager::Running,
            ServiceState::Stopped
            | ServiceState::Inactive
            | ServiceState::Stopping
            | ServiceState::Failed => DisplayManager::NotRunning,
        })
    }

    fn restore_status_output(&self) -> Result<(), OpenrcError> {
        Ok(())
    }

    fn silence_status_output(&self) -> Result<(), OpenrcError> {
        Ok(())
    }

    async fn quiet_status_output_during<Outer: From<OpenrcError>>(
        &self,
        watch: impl Future<Output = Result<StatusRelease, Outer>>,
    ) -> Result<(), Outer> {
        watch.await.map(drop)
    }
}
