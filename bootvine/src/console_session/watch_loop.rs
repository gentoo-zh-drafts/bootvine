use std::mem;

use bootvine_screen::{
    boot_model::{Phase, RootRequest, Scope},
    screen_components::handover::{Handover, StatusRelease, WatchEnding},
    screen_settings::{color_scheme::Colors, config_file::Timing},
};
use bootvine_systemd_link::link_error::Error;
use tokio::runtime::Builder;
use tracing::info;

use crate::console_session::{
    live_runtime::{LiveRuntime, LiveSettings},
    screen_loop::{ScreenRuntime, print_shell_summary, run_screen},
    service_manager::{ServiceManager, SystemdManager},
};

pub struct Settings {
    pub scope: Scope,
    pub shutdown: bool,
    pub target: RootRequest,
    pub timing: Timing,
    pub colors: Colors,
}

fn boot_already_finished(manager: &impl ServiceManager, settings: &Settings) -> bool {
    settings.scope == Scope::System && !settings.shutdown && !manager.boot_in_progress()
}

fn starting_phase(manager: &impl ServiceManager, settings: &Settings) -> Phase {
    match settings.scope {
        _ if settings.shutdown => Phase::Shutdown,
        Scope::User => Phase::User,
        Scope::System if manager.initrd_running() => Phase::Initrd,
        Scope::System => Phase::Host,
    }
}

pub fn run(settings: &Settings) -> Result<(), Error> {
    let manager = SystemdManager;
    if boot_already_finished(&manager, settings) {
        info!("boot already finished, leaving the console alone");
        return Ok(());
    }
    let phase = starting_phase(&manager, settings);
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(watch_live_boot(manager, settings, phase))
}

async fn watch_live_boot<Manager: ServiceManager + Copy>(
    manager: Manager,
    settings: &Settings,
    phase: Phase,
) -> Result<(), Error> {
    match settings.scope {
        Scope::System => {
            manager
                .release_status_output_after(watch_on_console(manager, settings, phase))
                .await
        }
        Scope::User => watch_on_console(manager, settings, phase).await.map(drop),
    }
}

async fn watch_on_console<Manager: ServiceManager>(
    manager: Manager,
    settings: &Settings,
    phase: Phase,
) -> Result<StatusRelease, Error> {
    let (mut runtime, mut model) = LiveRuntime::start(
        manager,
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
        Handover::Ended(WatchEnding::Failed(failure)) => Err(Error::Watch(failure)),
        Handover::Ended(WatchEnding::BootCompleted) => Ok(StatusRelease::AfterBoot),
        Handover::Ended(WatchEnding::ViewerLeft | WatchEnding::ShellTookOver) => {
            Ok(StatusRelease::Immediately)
        }
        Handover::Watching | Handover::HandingOver { .. } | Handover::SettlingForShell { .. } => {
            unreachable!("the screen loop returns only once the watch has ended")
        }
    }
}
