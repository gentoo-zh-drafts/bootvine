use std::{mem, path::Path};

use tokio::runtime::Builder;
use tracing::info;

use crate::{
    boot_model::Phase,
    console_session::{
        live_runtime::{LiveRuntime, LiveSettings},
        screen_loop::{ScreenRuntime, print_shell_summary, run_screen},
    },
    screen_components::handover::{Handover, WatchEnding},
    screen_settings::{color_scheme::Colors, config_file::Timing},
    systemd_link::{
        boot_feed::RootRequest,
        link_error::Error,
        switch_root::INITRD_RELEASE,
        systemd_bus::{Scope, Session, StatusRelease, restore_status_after_watch},
    },
};

pub struct Settings {
    pub scope: Scope,
    pub shutdown: bool,
    pub target: RootRequest,
    pub timing: Timing,
    pub colors: Colors,
}

fn boot_already_finished(settings: &Settings) -> bool {
    settings.scope == Scope::System
        && !settings.shutdown
        && Session::open(Scope::System)
            .and_then(|session| session.boot_in_progress())
            .is_ok_and(|booting| !booting)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the initrd announces itself with /etc/initrd-release"
)]
fn starting_phase(settings: &Settings) -> Phase {
    match settings.scope {
        _ if settings.shutdown => Phase::Shutdown,
        Scope::User => Phase::User,
        Scope::System if Path::new(INITRD_RELEASE).exists() => Phase::Initrd,
        Scope::System => Phase::Host,
    }
}

pub fn run(settings: &Settings) -> Result<(), Error> {
    if boot_already_finished(settings) {
        info!("boot already finished, leaving the console alone");
        return Ok(());
    }
    Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?
        .block_on(watch_live_boot(settings, starting_phase(settings)))
}

async fn watch_live_boot(settings: &Settings, phase: Phase) -> Result<(), Error> {
    match settings.scope {
        Scope::System => restore_status_after_watch(watch_on_console(settings, phase)).await,
        Scope::User => watch_on_console(settings, phase).await.map(drop),
    }
}

async fn watch_on_console(settings: &Settings, phase: Phase) -> Result<StatusRelease, Error> {
    let (mut runtime, mut model) = LiveRuntime::start(LiveSettings {
        scope: settings.scope,
        phase,
        target: settings.target.clone(),
        colors: settings.colors,
        timing: settings.timing,
    });
    let mut terminal = ratatui::try_init()?;
    let outcome = run_screen(&mut model, &mut runtime, &mut terminal).await;
    ratatui::restore();
    outcome?;
    print_shell_summary(&model, runtime.now())?;
    match mem::replace(&mut model.handover, Handover::Watching) {
        Handover::Ended(WatchEnding::Failed(error)) => Err(error),
        Handover::Ended(WatchEnding::BootCompleted) => Ok(StatusRelease::AfterBoot),
        Handover::Ended(WatchEnding::ViewerLeft | WatchEnding::ShellTookOver) => {
            Ok(StatusRelease::Immediately)
        }
        Handover::Watching | Handover::HandingOver { .. } | Handover::SettlingForShell { .. } => {
            unreachable!("the screen loop returns only once the watch has ended")
        }
    }
}
