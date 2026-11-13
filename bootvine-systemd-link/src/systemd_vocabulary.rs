use bootvine_screen::boot_model::ManagerVocabulary;

pub const HANDOFF_UNIT: &str = "display-manager.service";
pub const SHELL_UNITS: [&str; 2] = ["emergency.service", "rescue.service"];
const HOST_MILESTONES: [&str; 4] = [
    "sysinit.target",
    "basic.target",
    "multi-user.target",
    "graphical.target",
];
const SHUTDOWN_MILESTONES: [&str; 3] = ["shutdown.target", "umount.target", "final.target"];

#[must_use]
pub fn systemd_vocabulary() -> ManagerVocabulary {
    ManagerVocabulary {
        host_milestones: HOST_MILESTONES.map(str::to_owned).to_vec(),
        shutdown_milestones: SHUTDOWN_MILESTONES.map(str::to_owned).to_vec(),
        handoff_unit: HANDOFF_UNIT.to_owned(),
        shell_units: SHELL_UNITS.map(str::to_owned).to_vec(),
        shutdown_successor: "systemd-shutdown".to_owned(),
    }
}
