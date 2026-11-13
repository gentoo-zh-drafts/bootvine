use std::time::Duration;

use tracing::info;

use crate::{
    boot_model::BootState, screen_settings::config_file::Timing, systemd_link::link_error::Error,
};

pub const LATE_FAILURES_PATIENCE: Duration = Duration::from_millis(1200);

#[derive(Debug)]
pub enum WatchEnding {
    BootCompleted,
    ViewerLeft,
    ShellTookOver,
    Failed(Error),
}

#[derive(Debug)]
pub enum Handover {
    Watching,
    HandingOver { deadline: Duration },
    SettlingForShell { deadline: Duration },
    Ended(WatchEnding),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellStart {
    Starting,
    NotStarting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayManager {
    Running,
    NotRunning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellReadiness {
    Settled,
    Waiting,
}

impl Handover {
    #[must_use]
    pub fn deadline(&self) -> Option<Duration> {
        match self {
            Self::HandingOver { deadline } | Self::SettlingForShell { deadline } => Some(*deadline),
            Self::Watching | Self::Ended(_) => None,
        }
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Watching | Self::HandingOver { .. })
    }

    #[must_use]
    pub fn has_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }

    pub fn end(&mut self, ending: WatchEnding) {
        *self = Self::Ended(ending);
    }

    pub fn follow_boot(&mut self, boot: &BootState, timing: &Timing, now: Duration) {
        if !matches!(self, Self::Watching) {
            return;
        }
        let (reached, wait) = if let Some(display_manager) = boot.started_display_manager() {
            (display_manager, timing.wait_for_display_manager)
        } else if boot.is_complete() {
            (
                boot.root.unit().unwrap_or("boot"),
                timing.keep_final_frame_for,
            )
        } else {
            return;
        };
        info!(unit = reached, "reached, handing the console over");
        *self = Self::HandingOver {
            deadline: now + wait,
        };
    }

    pub fn settle_for_shell(&mut self, now: Duration) {
        *self = Self::SettlingForShell {
            deadline: now + LATE_FAILURES_PATIENCE,
        };
    }

    #[must_use]
    pub fn is_settling(&self) -> bool {
        matches!(self, Self::SettlingForShell { .. })
    }
}

#[must_use]
pub fn shell_readiness(boot: &BootState) -> ShellReadiness {
    if boot.shell_start_queued() && !boot.awaits_journal_lines() {
        ShellReadiness::Settled
    } else {
        ShellReadiness::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        boot_model::{ActiveState, Activity, BootChange, HANDOFF_UNIT},
        boot_test_fixtures::{MS, nested_boot_sample, queue_unit, waiting},
    };

    #[test]
    fn display_manager_start_hands_over_before_the_boot_completes() {
        let mut state = nested_boot_sample();
        queue_unit(&mut state, 20, waiting(HANDOFF_UNIT, &[]), 500 * MS);
        let timing = Timing::default();
        let mut handover = Handover::Watching;
        handover.follow_boot(&state, &timing, 1000 * MS);
        assert!(matches!(handover, Handover::Watching));
        state.apply(BootChange::UpdateActivity {
            unit: HANDOFF_UNIT.to_owned(),
            activity: Activity {
                state: Some(ActiveState::Active),
                ..Activity::default()
            },
        });
        handover.follow_boot(&state, &timing, 6000 * MS);
        assert!(!state.is_complete());
        assert_eq!(
            handover.deadline(),
            Some(6000 * MS + timing.wait_for_display_manager)
        );
        handover.follow_boot(&state, &timing, 9000 * MS);
        assert_eq!(
            handover.deadline(),
            Some(6000 * MS + timing.wait_for_display_manager)
        );
    }
}
