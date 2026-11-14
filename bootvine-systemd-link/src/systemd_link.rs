use std::{
    future::{Future, ready},
    io,
    path::Path,
};

use bootvine_screen::{
    boot_model::{
        ActiveState, BootChange, BootProgress, ConsolePrompt, JobKind, ManagerVocabulary, Phase,
        RootRequest, RunRequest, Scope,
    },
    screen_components::{
        handover::{DisplayManager, ShellStart, StatusRelease, WatchFailure},
        password_request::{PasswordAnswer, PasswordRequest, PromptAsker, PromptDeadline},
    },
    service_manager::ServiceManager,
};
use futures::{Stream, stream};
use tracing::warn;

use crate::{
    boot_feed::boot_change_stream,
    link_error::{LinkError, LinkFailure},
    password_agent::{console_prompt, password_request_stream, send_answer, until_abandoned},
    switch_root::INITRD_RELEASE,
    systemd_bus::{
        BusConnection, StatusOutput, UnitLookup, change_status_output_now,
        restore_status_after_watch,
    },
    systemd_vocabulary::{HANDOFF_UNIT, SHELL_UNITS, systemd_vocabulary},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemdLink;

async fn system_connection() -> Result<BusConnection, LinkFailure> {
    BusConnection::open(Scope::System).await
}

async fn queried_shell_start() -> Result<ShellStart, LinkFailure> {
    let jobs = system_connection().await?.list_jobs().await?;
    let starting = jobs
        .iter()
        .any(|job| job.kind == JobKind::BringsUp && SHELL_UNITS.contains(&job.unit.as_str()));
    Ok(if starting {
        ShellStart::Starting
    } else {
        ShellStart::NotStarting
    })
}

async fn queried_display_manager() -> Result<DisplayManager, LinkFailure> {
    let UnitLookup::Loaded(fetched) = system_connection().await?.fetch_unit(HANDOFF_UNIT).await?
    else {
        return Ok(DisplayManager::NotRunning);
    };
    Ok(match fetched.activity.state {
        Some(ActiveState::Active | ActiveState::Activating | ActiveState::Reloading) => {
            DisplayManager::Running
        }
        _ => DisplayManager::NotRunning,
    })
}

impl ServiceManager for SystemdLink {
    type Failure = LinkError;

    fn vocabulary(&self) -> ManagerVocabulary {
        systemd_vocabulary()
    }

    fn starting_phase(&self, scope: Scope, request: RunRequest) -> Phase {
        match (request, scope) {
            (RunRequest::Shutdown, _) => Phase::Shutdown,
            (RunRequest::Boot, Scope::User) => Phase::User,
            (RunRequest::Boot, Scope::System) if Path::new(INITRD_RELEASE).exists() => {
                Phase::Initrd
            }
            (RunRequest::Boot, Scope::System) => Phase::Host,
        }
    }

    async fn boot_progress(&self) -> BootProgress {
        let asked_progress = match system_connection().await {
            Ok(mut connection) => connection.boot_progress().await,
            Err(unreachable) => Err(unreachable),
        };
        match asked_progress {
            Ok(boot_progress) => boot_progress,
            Err(error) => {
                warn!(error = %error, "could not ask systemd whether the boot finished");
                BootProgress::Finished
            }
        }
    }

    fn boot_changes(
        &self,
        phase: Phase,
        scope: Scope,
        root: RootRequest,
    ) -> impl Stream<Item = Result<Vec<BootChange>, WatchFailure>> + 'static {
        boot_change_stream(phase, scope, root)
    }

    fn password_requests(&self) -> impl Stream<Item = Vec<PasswordRequest>> + 'static {
        password_request_stream()
    }

    fn password_abandoned(
        &self,
        deadline: PromptDeadline,
        asker: PromptAsker,
    ) -> impl Future<Output = ()> + 'static {
        until_abandoned(deadline, asker)
    }

    fn console_prompt(&self) -> ConsolePrompt {
        console_prompt()
    }

    fn console_prompts(&self) -> impl Stream<Item = ConsolePrompt> + 'static {
        stream::pending()
    }

    fn answer_password(&self, answer: PasswordAnswer) -> impl Future<Output = io::Result<()>> {
        ready(send_answer(&answer))
    }

    async fn shell_start(&self) -> ShellStart {
        queried_shell_start()
            .await
            .expect("the private socket of systemd answers while a shell is about to start")
    }

    async fn display_manager(&self) -> DisplayManager {
        queried_display_manager()
            .await
            .expect("the private socket of systemd answers while the boot runs")
    }

    fn restore_status_output(&self) -> Result<(), LinkError> {
        Ok(change_status_output_now(StatusOutput::Configured)?)
    }

    fn silence_status_output(&self) -> Result<(), LinkError> {
        Ok(change_status_output_now(StatusOutput::Silent)?)
    }

    async fn quiet_status_output_during<Outer: From<LinkError>>(
        &self,
        watch: impl Future<Output = Result<StatusRelease, Outer>>,
    ) -> Result<(), Outer> {
        restore_status_after_watch(watch).await
    }
}
