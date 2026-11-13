use std::mem;

use bootvine_screen::{
    boot_model::{BootProgress, Phase, RootRequest, RunRequest, Scope},
    screen_components::handover::{Handover, StatusRelease, WatchEnding},
    screen_settings::{color_scheme::Colors, config_file::Timing},
    service_manager::ServiceManager,
};
use tokio::runtime::Builder;
use tracing::info;

use crate::console_session::{
    live_runtime::{LiveRuntime, LiveSettings},
    run_error::RunError,
    screen_loop::{ScreenRuntime, print_shell_summary, run_screen},
    service_manager::DetectedManager,
};

pub struct Settings {
    pub scope: Scope,
    pub request: RunRequest,
    pub target: RootRequest,
    pub timing: Timing,
    pub colors: Colors,
}

pub fn run(settings: &Settings) -> Result<(), RunError> {
    let runtime = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    match DetectedManager::running()? {
        #[cfg(feature = "systemd")]
        DetectedManager::Systemd(service_manager) => {
            runtime.block_on(watch_with(service_manager, settings))
        }
        #[cfg(feature = "openrc")]
        DetectedManager::Openrc(service_manager) => {
            runtime.block_on(watch_with(service_manager, settings))
        }
    }
}

async fn watch_with<Manager: ServiceManager>(
    service_manager: Manager,
    settings: &Settings,
) -> Result<(), RunError>
where
    RunError: From<Manager::Failure>,
{
    let request = settings.request;
    if settings.scope == Scope::System
        && request == RunRequest::Boot
        && service_manager.boot_progress().await == BootProgress::Finished
    {
        info!("boot already finished, leaving the console alone");
        return Ok(());
    }
    let phase = service_manager.starting_phase(settings.scope, request);
    watch_live_boot(service_manager, settings, phase).await
}

async fn watch_live_boot<Manager: ServiceManager>(
    service_manager: Manager,
    settings: &Settings,
    phase: Phase,
) -> Result<(), RunError>
where
    RunError: From<Manager::Failure>,
{
    match settings.scope {
        Scope::System => {
            service_manager
                .clone()
                .quiet_status_output_during(watch_on_console(service_manager, settings, phase))
                .await
        }
        Scope::User => watch_on_console(service_manager, settings, phase)
            .await
            .map(drop),
    }
}

async fn watch_on_console<Manager: ServiceManager>(
    service_manager: Manager,
    settings: &Settings,
    phase: Phase,
) -> Result<StatusRelease, RunError> {
    let (mut runtime, mut model) = LiveRuntime::start(
        service_manager,
        LiveSettings {
            scope: settings.scope,
            phase,
            target: settings.target.clone(),
            colors: settings.colors,
            timing: settings.timing,
        },
    );
    let mut terminal = ratatui::try_init()?;
    let outcome = run_screen(&mut model, &mut runtime, &mut terminal).await;
    ratatui::restore();
    outcome?;
    print_shell_summary(&model, runtime.now())?;
    match mem::replace(&mut model.handover, Handover::Watching) {
        Handover::Ended(WatchEnding::Failed(failure)) => Err(RunError::Watch(failure)),
        Handover::Ended(WatchEnding::BootCompleted) => Ok(StatusRelease::AfterBoot),
        Handover::Ended(WatchEnding::ViewerLeft | WatchEnding::ShellTookOver) => {
            Ok(StatusRelease::Immediately)
        }
        Handover::Watching | Handover::HandingOver { .. } | Handover::SettlingForShell { .. } => {
            unreachable!("the screen loop returns only once the watch has ended")
        }
    }
}
