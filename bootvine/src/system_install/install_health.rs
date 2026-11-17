#[cfg(feature = "systemd")]
mod boot_runs;
mod health_findings;
mod initramfs_console;
mod installed_units;
mod mkinitcpio_hooks;

use std::{
    env, fs,
    io::{self, ErrorKind},
    iter::once,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(feature = "systemd")]
use bootvine_systemd_link::JournalReadError;
use rustix::process::geteuid;

pub use self::health_findings::OverallHealth;
use self::{
    health_findings::HealthFinding,
    initramfs_console::initramfs_console_finding,
    installed_units::{enabled_unit_finding, installed_unit_finding},
    mkinitcpio_hooks::mkinitcpio_hook_check,
};
use crate::{
    config_reading::{ConfigError, read_config_file},
    system_install::{
        install_error::InstallError,
        install_files::{
            InitrdGenerator, InstalledManager, OPENRC_RUNLEVEL, OPENRC_SCRIPT, OPENRC_SERVICE,
            PACKAGED_UNITS, initrd_hook_files, running_manager,
        },
    },
};

const CONFIG_PATH: &str = "/etc/bootvine.toml";
const RUNNING_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTALL_GUIDE: &str = "https://codeberg.org/wvhulle/bootvine/src/branch/main/docs/install.md";
const ISSUE_TRACKER: &str = "https://codeberg.org/wvhulle/bootvine/issues";
const REBUILD_INITRAMFS: &str =
    "rebuild the initramfs (dracut --force, mkinitcpio -P, or nixos-rebuild) and reboot";

#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    #[error(transparent)]
    Install(#[from] InstallError),
    #[cfg(feature = "systemd")]
    #[error("reading this boot's journal: {0}")]
    Journal(#[from] JournalReadError),
    #[error("cannot rerun as root with sudo: {0}")]
    Elevation(io::Error),
}

#[derive(Clone, Copy)]
enum InitramfsTool {
    Dracut,
    Mkinitcpio,
}

enum ToolAvailability {
    OnPath,
    Missing,
}

enum HookFiles {
    Installed,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InitrdSetup {
    Prepared,
    Unprepared,
}

struct InitrdHookCheck {
    finding: HealthFinding,
    initrd_setup: InitrdSetup,
}

impl InitramfsTool {
    const ALL: [Self; 2] = [Self::Dracut, Self::Mkinitcpio];

    fn command(self) -> &'static str {
        match self {
            Self::Dracut => "dracut",
            Self::Mkinitcpio => "mkinitcpio",
        }
    }
}

impl From<InitramfsTool> for InitrdGenerator {
    fn from(initramfs_tool: InitramfsTool) -> Self {
        match initramfs_tool {
            InitramfsTool::Dracut => Self::Dracut,
            InitramfsTool::Mkinitcpio => Self::Mkinitcpio,
        }
    }
}

fn tool_availability(command: &str) -> ToolAvailability {
    let found = env::var_os("PATH").is_some_and(|search_path| {
        env::split_paths(&search_path).any(|directory| directory.join(command).is_file())
    });
    if found {
        ToolAvailability::OnPath
    } else {
        ToolAvailability::Missing
    }
}

fn hook_files(initramfs_tool: InitramfsTool) -> HookFiles {
    if initrd_hook_files(InitrdGenerator::from(initramfs_tool))
        .iter()
        .all(|hook_file| hook_file.destination.exists())
    {
        HookFiles::Installed
    } else {
        HookFiles::Missing
    }
}

fn command_stdout(program: &str, arguments: &[&str]) -> Result<String, InstallError> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|source| InstallError::Spawn {
            program: program.to_owned(),
            source,
        })?;
    Ok(String::from_utf8(output.stdout)
        .expect("systemctl prints UTF-8")
        .trim()
        .to_owned())
}

fn read_file(path: &Path) -> Result<String, InstallError> {
    fs::read_to_string(path).map_err(|source| InstallError::File {
        path: path.to_owned(),
        source,
    })
}

fn config_finding() -> HealthFinding {
    match read_config_file(Path::new(CONFIG_PATH)) {
        Ok(_) => HealthFinding::healthy(format!("{CONFIG_PATH} is valid")),
        Err(ConfigError::Unreadable(error)) if error.kind() == ErrorKind::NotFound => {
            HealthFinding::healthy(format!("no {CONFIG_PATH}, using the defaults"))
        }
        Err(error) => HealthFinding::unhealthy(
            format!("{CONFIG_PATH}: {error}"),
            "fix the file; `bootvine defaults` prints every key with its default".to_owned(),
        ),
    }
}

fn initrd_hook_check(initramfs_tool: InitramfsTool) -> Result<InitrdHookCheck, InstallError> {
    let command = initramfs_tool.command();
    match (hook_files(initramfs_tool), initramfs_tool) {
        (HookFiles::Missing, _) => Ok(InitrdHookCheck {
            finding: HealthFinding::advice(
                format!(
                    "{command} is installed without bootvine's hook, so the initrd is not shown"
                ),
                format!("add the hook as described in {INSTALL_GUIDE}"),
            ),
            initrd_setup: InitrdSetup::Unprepared,
        }),
        (HookFiles::Installed, InitramfsTool::Mkinitcpio) => mkinitcpio_hook_check(),
        (HookFiles::Installed, InitramfsTool::Dracut) => Ok(InitrdHookCheck {
            finding: HealthFinding::healthy(format!("{command} has bootvine's module")),
            initrd_setup: InitrdSetup::Prepared,
        }),
    }
}

fn systemd_findings() -> Result<Vec<HealthFinding>, HealthError> {
    let unit_findings = PACKAGED_UNITS
        .iter()
        .map(|unit| Ok([installed_unit_finding(unit)?, enabled_unit_finding(unit)?]))
        .collect::<Result<Vec<_>, InstallError>>()?;
    let hook_checks = InitramfsTool::ALL
        .into_iter()
        .filter(|initramfs_tool| {
            matches!(
                tool_availability(initramfs_tool.command()),
                ToolAvailability::OnPath
            )
        })
        .map(initrd_hook_check)
        .collect::<Result<Vec<_>, _>>()?;
    let initrd_setup = if hook_checks
        .iter()
        .any(|hook_check| hook_check.initrd_setup == InitrdSetup::Prepared)
    {
        InitrdSetup::Prepared
    } else {
        InitrdSetup::Unprepared
    };
    #[cfg(feature = "systemd")]
    let initrd_run_finding = boot_runs::initrd_run_finding(initrd_setup)?;
    #[cfg(not(feature = "systemd"))]
    let initrd_run_finding = match initrd_setup {
        InitrdSetup::Prepared | InitrdSetup::Unprepared => HealthFinding::advice(
            "this bootvine was built without systemd support, so it cannot read the journal"
                .to_owned(),
            "build it with the systemd feature".to_owned(),
        ),
    };
    let console_findings = match tool_availability("lsinitrd") {
        ToolAvailability::OnPath => vec![initramfs_console_finding()?],
        ToolAvailability::Missing => Vec::new(),
    };
    Ok(unit_findings
        .into_iter()
        .flatten()
        .chain(once(config_finding()))
        .chain(hook_checks.into_iter().map(|hook_check| hook_check.finding))
        .chain(once(initrd_run_finding))
        .chain(console_findings)
        .collect())
}

fn openrc_findings() -> Vec<HealthFinding> {
    let script_finding = if Path::new(OPENRC_SCRIPT.name).is_file() {
        HealthFinding::healthy(format!("{} is installed", OPENRC_SCRIPT.name))
    } else {
        HealthFinding::unhealthy(
            format!("{} is not installed", OPENRC_SCRIPT.name),
            format!("install bootvine as described in {INSTALL_GUIDE}"),
        )
    };
    let runlevel_link: PathBuf = Path::new("/etc/runlevels")
        .join(OPENRC_RUNLEVEL)
        .join(OPENRC_SERVICE);
    let runlevel_finding = if runlevel_link.exists() {
        HealthFinding::healthy(format!(
            "{OPENRC_SERVICE} is in the {OPENRC_RUNLEVEL} runlevel"
        ))
    } else {
        HealthFinding::unhealthy(
            format!("{OPENRC_SERVICE} is not in the {OPENRC_RUNLEVEL} runlevel"),
            format!("rc-update add {OPENRC_SERVICE} {OPENRC_RUNLEVEL}"),
        )
    };
    vec![script_finding, runlevel_finding, config_finding()]
}

fn rerun_as_root() -> HealthError {
    HealthError::Elevation(match env::current_exe() {
        Ok(running_binary) => Command::new("sudo")
            .arg(running_binary)
            .args(env::args_os().skip(1))
            .exec(),
        Err(error) => error,
    })
}

pub fn report_health() -> Result<OverallHealth, HealthError> {
    if !geteuid().is_root() {
        return Err(rerun_as_root());
    }
    let findings = match running_manager() {
        InstalledManager::Systemd => systemd_findings()?,
        InstalledManager::Openrc => openrc_findings(),
    };
    findings.iter().for_each(|finding| println!("{finding}"));
    Ok(OverallHealth::from(findings.as_slice()))
}
