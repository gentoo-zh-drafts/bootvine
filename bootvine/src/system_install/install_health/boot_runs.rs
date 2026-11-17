use bootvine_screen::boot_model::Phase;
use bootvine_systemd_link::{
    BootvineRun, JournalReadError, LoggedVersion, this_boot_bootvine_runs,
};

use super::{
    INSTALL_GUIDE, InitrdSetup, REBUILD_INITRAMFS, RUNNING_VERSION, health_findings::HealthFinding,
};

#[derive(Debug, PartialEq, Eq)]
enum BootvineInBoot {
    NotRun,
    AfterInitrdOnly,
    InInitrd(LoggedVersion),
}

impl From<Vec<BootvineRun>> for BootvineInBoot {
    fn from(bootvine_runs: Vec<BootvineRun>) -> Self {
        if bootvine_runs.is_empty() {
            return Self::NotRun;
        }
        bootvine_runs
            .into_iter()
            .find(|bootvine_run| bootvine_run.phase == Phase::Initrd)
            .map_or(Self::AfterInitrdOnly, |initrd_run| {
                Self::InInitrd(initrd_run.version)
            })
    }
}

pub(super) fn initrd_run_finding(
    initrd_setup: InitrdSetup,
) -> Result<HealthFinding, JournalReadError> {
    Ok(
        match (
            BootvineInBoot::from(this_boot_bootvine_runs()?),
            initrd_setup,
        ) {
            (BootvineInBoot::InInitrd(LoggedVersion::Logged(version)), _)
                if version == RUNNING_VERSION =>
            {
                HealthFinding::healthy(format!("this boot's initrd ran bootvine {version}"))
            }
            (BootvineInBoot::InInitrd(LoggedVersion::Logged(version)), _) => {
                HealthFinding::unhealthy(
                    format!("this boot's initrd ran bootvine {version}, not {RUNNING_VERSION}"),
                    REBUILD_INITRAMFS.to_owned(),
                )
            }
            (BootvineInBoot::InInitrd(LoggedVersion::Unlogged), _) => HealthFinding::unhealthy(
                format!("this boot's initrd ran a bootvine older than {RUNNING_VERSION}"),
                REBUILD_INITRAMFS.to_owned(),
            ),
            (BootvineInBoot::AfterInitrdOnly, InitrdSetup::Prepared) => HealthFinding::unhealthy(
                "bootvine is set up for the initramfs but did not run in this boot's initrd"
                    .to_owned(),
                REBUILD_INITRAMFS.to_owned(),
            ),
            (BootvineInBoot::AfterInitrdOnly, InitrdSetup::Unprepared) => HealthFinding::advice(
                "bootvine ran only after the initrd in this boot".to_owned(),
                format!("to show the boot from the initrd, see {INSTALL_GUIDE}"),
            ),
            (BootvineInBoot::NotRun, _) => HealthFinding::unhealthy(
                "bootvine did not run during this boot".to_owned(),
                "reboot, or read journalctl -b -u bootvine".to_owned(),
            ),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_in(phase: Phase) -> BootvineRun {
        BootvineRun {
            phase,
            version: LoggedVersion::Logged("0.2.0".to_owned()),
        }
    }

    #[test]
    fn the_initrd_run_decides_where_bootvine_ran() {
        assert_eq!(BootvineInBoot::from(Vec::new()), BootvineInBoot::NotRun);
        assert_eq!(
            BootvineInBoot::from(vec![run_in(Phase::Host)]),
            BootvineInBoot::AfterInitrdOnly
        );
        assert_eq!(
            BootvineInBoot::from(vec![run_in(Phase::Initrd), run_in(Phase::Host)]),
            BootvineInBoot::InInitrd(LoggedVersion::Logged("0.2.0".to_owned()))
        );
    }
}
