use std::{
    io::{self, Write},
    path::Path,
    pin::{Pin, pin},
    time::Duration,
};

use crossterm::{
    cursor::{MoveTo, Show},
    execute,
    style::ResetColor,
    terminal::{Clear, ClearType, size},
};
use futures::{Stream, StreamExt, stream};
use ratatui::backend::CrosstermBackend;
use tokio::{runtime::Builder, time::timeout};

use crate::{
    boot_model::{BootState, Phase, boot_origin::BootOrigin},
    console_session::{
        ask_password::RequestWatch,
        live_desk::{LiveDesk, password_request_inputs},
        stream_sources::{
            BACKGROUND_CHECK_INTERVAL, frame_inputs, key_inputs, periodic_inputs,
            termination_inputs,
        },
        watch_screen::{
            ConsoleDesk, ScreenInput, WatchEnding, WatchScreen, overwrite_every_cell,
            watch_until_ending,
        },
    },
    screen_settings::{color_scheme::Colors, config_file::Timing},
    systemd_link::{
        boot_feed::{RootRequest, bus_inputs},
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

const FULL_REPAINT_INTERVAL: Duration = Duration::from_secs(5);
const LATE_FAILURES_PATIENCE: Duration = Duration::from_millis(1200);
const ROWS_LEFT_FOR_SHELL: u16 = 8;

async fn settle_failures_before_shell<Desk: ConsoleDesk>(
    screen: &mut WatchScreen<Desk>,
    mut inputs: Pin<&mut impl Stream<Item = ScreenInput>>,
) {
    let _ = timeout(LATE_FAILURES_PATIENCE, async {
        while !screen.state.shell_start_queued() || screen.state.awaits_journal_lines() {
            let Some(input) = inputs.next().await else {
                return;
            };
            screen.absorb_late_boot_changes(input);
        }
    })
    .await;
}

fn print_shell_handover_summary<Desk: ConsoleDesk>(screen: &WatchScreen<Desk>) -> io::Result<()> {
    let (columns, rows) = size()?;
    let summary = screen.shell_handover_summary(columns, rows.saturating_sub(ROWS_LEFT_FOR_SHELL));
    let mut console = io::stdout();
    execute!(console, Clear(ClearType::All), MoveTo(0, 0))?;
    overwrite_every_cell(&mut CrosstermBackend::new(io::stdout()), &summary)?;
    execute!(console, ResetColor, MoveTo(0, summary.area.height + 1), Show)?;
    console.flush()
}

fn boot_already_finished(settings: &Settings) -> bool {
    settings.scope == Scope::System
        && !settings.shutdown
        && Session::open(Scope::System)
            .and_then(|session| session.boot_in_progress())
            .is_ok_and(|booting| !booting)
}

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
        eprintln!("bootvine: boot already finished, leaving the console alone");
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
    let for_system = settings.scope == Scope::System;
    let requests = RequestWatch::open();
    let prompt_check: fn() -> ScreenInput = if requests.is_some() {
        || ScreenInput::PromptCheck
    } else {
        || ScreenInput::PasswordRequests
    };
    let desk = LiveDesk::take_console(for_system);
    let password_inputs = requests.map(password_request_inputs).transpose()?;
    let event_inputs = stream::select(
        stream::select(
            bus_inputs(phase, settings.scope, settings.target.clone()),
            key_inputs()?,
        ),
        stream::select(
            termination_inputs()?,
            stream::iter(password_inputs).flatten(),
        ),
    );
    let timer_inputs = stream::select(
        stream::select(
            frame_inputs(),
            periodic_inputs(BACKGROUND_CHECK_INTERVAL, prompt_check),
        ),
        stream::select(
            periodic_inputs(BACKGROUND_CHECK_INTERVAL, || ScreenInput::ConsoleUpkeep),
            periodic_inputs(FULL_REPAINT_INTERVAL, || ScreenInput::FullRepaint),
        ),
    );
    let mut inputs = pin!(stream::select(event_inputs, timer_inputs));
    let mut screen = WatchScreen::new(
        BootState::new(phase),
        desk,
        BootOrigin::read,
        settings.colors,
        settings.timing,
        Duration::ZERO,
    );
    let mut terminal = ratatui::try_init()?;
    let ending = watch_until_ending(&mut screen, &mut terminal, inputs.as_mut()).await;
    if matches!(ending, WatchEnding::ShellTookOver) {
        settle_failures_before_shell(&mut screen, inputs.as_mut()).await;
    }
    ratatui::restore();
    match ending {
        WatchEnding::Failed(error) => Err(error),
        WatchEnding::BootCompleted => Ok(StatusRelease::AfterBoot),
        WatchEnding::ViewerLeft => Ok(StatusRelease::Immediately),
        WatchEnding::ShellTookOver => {
            eprintln!("bootvine: a shell is starting, leaving the failures above it");
            print_shell_handover_summary(&screen)?;
            screen.desk.stay_in_front();
            Ok(StatusRelease::Immediately)
        }
    }
}
