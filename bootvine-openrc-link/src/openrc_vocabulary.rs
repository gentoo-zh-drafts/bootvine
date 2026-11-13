use bootvine_screen::boot_model::{ManagerVocabulary, Phase};

pub(crate) const HANDOFF_SERVICE: &str = "display-manager";
pub(crate) const PROMPTING_SERVICES: [&str; 1] = ["dmcrypt"];
pub(crate) const BOOT_RUNLEVELS: [&str; 3] = ["sysinit", "boot", "default"];
pub(crate) const SHUTDOWN_RUNLEVELS: [&str; 1] = ["shutdown"];

pub(crate) fn milestone_runlevels(phase: Phase) -> &'static [&'static str] {
    match phase {
        Phase::Shutdown => &SHUTDOWN_RUNLEVELS,
        Phase::Initrd | Phase::Host | Phase::User => &BOOT_RUNLEVELS,
    }
}

#[must_use]
pub fn openrc_vocabulary() -> ManagerVocabulary {
    ManagerVocabulary {
        host_milestones: BOOT_RUNLEVELS.map(str::to_owned).to_vec(),
        shutdown_milestones: SHUTDOWN_RUNLEVELS.map(str::to_owned).to_vec(),
        handoff_unit: HANDOFF_SERVICE.to_owned(),
        shell_units: Vec::new(),
        shutdown_successor: "openrc-shutdown".to_owned(),
    }
}
