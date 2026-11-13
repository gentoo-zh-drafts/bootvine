use std::{
    env,
    ffi::OsString,
    fs::{self, Permissions},
    io::ErrorKind,
    iter::once,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use clap::{Args, ValueEnum};
use rustix::process::geteuid;

use crate::system_install::install_error::InstallError;

const DRACUT_MODULE_DIR: &str = "/usr/lib/dracut/modules.d/90bootvine";
const DRACUT_CONF: &str = "/etc/dracut.conf.d/bootvine.conf";
const MKINITCPIO_HOOK: &str = "/usr/lib/initcpio/install/sd-bootvine";
const PACKAGED_BINARY_PATH: &str = "/usr/bin/bootvine";

struct PackagedUnit {
    name: &'static str,
    contents: &'static str,
}

const PACKAGED_UNITS: [PackagedUnit; 2] = [
    PackagedUnit {
        name: "bootvine.service",
        contents: include_str!("../../system-files/systemd/bootvine.service"),
    },
    PackagedUnit {
        name: "bootvine-shutdown.service",
        contents: include_str!("../../system-files/systemd/bootvine-shutdown.service"),
    },
];
const RESUME_DROP_IN_DIR: &str = "systemd-hibernate-resume.service.d";
const RESUME_DROP_IN: PackagedUnit = PackagedUnit {
    name: "systemd-hibernate-resume.service.d/bootvine.conf",
    contents: include_str!(
        "../../system-files/systemd/systemd-hibernate-resume.service.d/bootvine.conf"
    ),
};
const DRACUT_MODULE_SETUP: &str =
    include_str!("../../system-files/dracut/90bootvine/module-setup.sh");
const MKINITCPIO_INSTALL_HOOK: &str =
    include_str!("../../system-files/mkinitcpio/install/sd-bootvine");
const INITRD_GENERATORS: [InitrdGenerator; 2] =
    [InitrdGenerator::Dracut, InitrdGenerator::Mkinitcpio];
const OPENRC_SCRIPT: PackagedUnit = PackagedUnit {
    name: "/etc/init.d/bootvine",
    contents: include_str!("../../system-files/openrc/bootvine"),
};
const OPENRC_SERVICE: &str = "bootvine";
const OPENRC_RUNLEVEL: &str = "sysinit";
const OPENRC_RUNLEVEL_FILE: &str = "/run/openrc/softlevel";

#[derive(Args)]
pub struct InstallTarget {
    #[arg(
        long,
        default_value = "/usr",
        help = "Prefix for bin/bootvine and lib/systemd/system"
    )]
    prefix: String,
    #[arg(
        long,
        help = "Stage the files under this directory instead of /; skips sudo and systemctl"
    )]
    destdir: Option<PathBuf>,
    #[arg(
        long,
        value_enum,
        default_value_t = ManagerChoice::Detect,
        help = "Service manager to install for"
    )]
    service_manager: ManagerChoice,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ManagerChoice {
    Detect,
    Systemd,
    Openrc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InstalledManager {
    Systemd,
    Openrc,
}

#[derive(Args)]
pub struct InstallOptions {
    #[command(flatten)]
    target: InstallTarget,
    #[arg(
        long,
        value_enum,
        default_value_t = InitrdGenerator::None,
        help = "Also install the hook that adds bootvine to this initramfs generator"
    )]
    initrd: InitrdGenerator,
    #[arg(long, help = "Leave the units disabled")]
    no_enable: bool,
}

#[derive(Args)]
pub struct UninstallOptions {
    #[command(flatten)]
    target: InstallTarget,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InitrdGenerator {
    None,
    Dracut,
    Mkinitcpio,
}

struct InstalledFile {
    destination: PathBuf,
    contents: Vec<u8>,
    mode: u32,
}

impl InstallTarget {
    fn staged(&self, path: &Path) -> PathBuf {
        match &self.destdir {
            Some(root) => root.join(path.strip_prefix("/").unwrap_or(path)),
            None => path.to_owned(),
        }
    }

    fn changes_live_system(&self) -> bool {
        self.destdir.is_none()
    }

    fn needs_sudo(&self) -> bool {
        self.changes_live_system() && !geteuid().is_root()
    }

    fn installed_manager(&self) -> InstalledManager {
        match self.service_manager {
            ManagerChoice::Openrc => InstalledManager::Openrc,
            ManagerChoice::Detect if Path::new(OPENRC_RUNLEVEL_FILE).is_file() => {
                InstalledManager::Openrc
            }
            ManagerChoice::Systemd | ManagerChoice::Detect => InstalledManager::Systemd,
        }
    }

    fn binary_path(&self) -> String {
        format!("{}/bin/bootvine", self.prefix.trim_end_matches('/'))
    }

    fn unit_path(&self, unit: &str) -> PathBuf {
        Path::new(&self.prefix)
            .join("lib/systemd/system")
            .join(unit)
    }

    fn every_installed_path(&self) -> Vec<PathBuf> {
        once(PathBuf::from(self.binary_path()))
            .chain(unit_files(self).map(|file| file.destination))
            .chain(once(openrc_script(self).destination))
            .chain(
                INITRD_GENERATORS
                    .into_iter()
                    .flat_map(initrd_hook_files)
                    .map(|file| file.destination),
            )
            .collect()
    }
}

impl InitrdGenerator {
    fn install_hint(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Dracut => Some("rebuild the initramfs: sudo dracut --force"),
            Self::Mkinitcpio => Some(
                "add sd-bootvine after systemd in HOOKS in /etc/mkinitcpio.conf, then run: sudo \
                 mkinitcpio -P",
            ),
        }
    }

    fn removal_hint(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Dracut => Some("rebuild the initramfs without bootvine: sudo dracut --force"),
            Self::Mkinitcpio => Some(
                "remove sd-bootvine from HOOKS in /etc/mkinitcpio.conf, then run: sudo mkinitcpio \
                 -P",
            ),
        }
    }
}

fn running_binary() -> Result<PathBuf, InstallError> {
    env::current_exe().map_err(|source| InstallError::File {
        path: PathBuf::from("/proc/self/exe"),
        source,
    })
}

fn binary_file(target: &InstallTarget) -> Result<Option<InstalledFile>, InstallError> {
    let running = running_binary()?;
    let destination = PathBuf::from(target.binary_path());
    if fs::canonicalize(target.staged(&destination)).is_ok_and(|resolved| resolved == running) {
        return Ok(None);
    }
    let contents = fs::read(&running).map_err(|source| InstallError::File {
        path: running,
        source,
    })?;
    Ok(Some(InstalledFile {
        destination,
        contents,
        mode: 0o755,
    }))
}

fn unit_files(target: &InstallTarget) -> impl Iterator<Item = InstalledFile> {
    let binary_path = target.binary_path();
    PACKAGED_UNITS
        .iter()
        .chain(once(&RESUME_DROP_IN))
        .map(move |unit| InstalledFile {
            destination: target.unit_path(unit.name),
            contents: unit
                .contents
                .replace(PACKAGED_BINARY_PATH, &binary_path)
                .into_bytes(),
            mode: 0o644,
        })
}

fn openrc_script(target: &InstallTarget) -> InstalledFile {
    InstalledFile {
        destination: PathBuf::from(OPENRC_SCRIPT.name),
        contents: OPENRC_SCRIPT
            .contents
            .replace(PACKAGED_BINARY_PATH, &target.binary_path())
            .into_bytes(),
        mode: 0o755,
    }
}

fn service_files(target: &InstallTarget) -> Vec<InstalledFile> {
    match target.installed_manager() {
        InstalledManager::Systemd => unit_files(target).collect(),
        InstalledManager::Openrc => vec![openrc_script(target)],
    }
}

fn initrd_hook_files(initrd: InitrdGenerator) -> Vec<InstalledFile> {
    match initrd {
        InitrdGenerator::None => Vec::new(),
        InitrdGenerator::Dracut => vec![
            InstalledFile {
                destination: Path::new(DRACUT_MODULE_DIR).join("module-setup.sh"),
                contents: DRACUT_MODULE_SETUP.as_bytes().to_vec(),
                mode: 0o755,
            },
            InstalledFile {
                destination: PathBuf::from(DRACUT_CONF),
                contents: b"add_dracutmodules+=\" bootvine \"\n".to_vec(),
                mode: 0o644,
            },
        ],
        InitrdGenerator::Mkinitcpio => vec![InstalledFile {
            destination: PathBuf::from(MKINITCPIO_HOOK),
            contents: MKINITCPIO_INSTALL_HOOK.as_bytes().to_vec(),
            mode: 0o644,
        }],
    }
}

fn sibling_for_replacement(path: &Path) -> PathBuf {
    let mut file_name = OsString::from(".");
    file_name.push(path.file_name().unwrap_or_default());
    file_name.push(".bootvine-new");
    path.with_file_name(file_name)
}

fn write_installed_file(target: &InstallTarget, file: &InstalledFile) -> Result<(), InstallError> {
    let path = target.staged(&file.destination);
    let replacement = sibling_for_replacement(&path);
    path.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| fs::write(&replacement, &file.contents))
        .and_then(|()| fs::set_permissions(&replacement, Permissions::from_mode(file.mode)))
        .and_then(|()| fs::rename(&replacement, &path))
        .map_err(|source| InstallError::File { path, source })
}

fn remove_if_present(path: PathBuf) -> Result<(), InstallError> {
    match fs::remove_file(&path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(InstallError::File {
            path,
            source: error,
        }),
        _ => Ok(()),
    }
}

fn run_command(command: &mut Command) -> Result<(), InstallError> {
    let program = command.get_program().display().to_string();
    let status = command.status().map_err(|source| InstallError::Spawn {
        program: program.clone(),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(InstallError::Failed { program, status })
    }
}

fn systemctl<'a>(arguments: impl IntoIterator<Item = &'a str>) -> Result<(), InstallError> {
    run_command(Command::new("systemctl").args(arguments))
}

fn rc_update<'a>(arguments: impl IntoIterator<Item = &'a str>) -> Result<(), InstallError> {
    run_command(Command::new("rc-update").args(arguments))
}

fn unit_names() -> impl Iterator<Item = &'static str> {
    PACKAGED_UNITS.iter().map(|unit| unit.name)
}

fn rerun_with_sudo() -> Result<(), InstallError> {
    run_command(
        Command::new("sudo")
            .arg(running_binary()?)
            .args(env::args_os().skip(1)),
    )
}

pub fn install_files(options: &InstallOptions) -> Result<(), InstallError> {
    let target = &options.target;
    let installed_manager = target.installed_manager();
    if installed_manager == InstalledManager::Openrc && options.initrd != InitrdGenerator::None {
        return Err(InstallError::InitrdNeedsSystemd);
    }
    if target.needs_sudo() {
        return rerun_with_sudo();
    }
    binary_file(target)?
        .into_iter()
        .chain(service_files(target))
        .chain(initrd_hook_files(options.initrd))
        .try_for_each(|file| write_installed_file(target, &file))?;
    if !target.changes_live_system() {
        return Ok(());
    }
    match installed_manager {
        InstalledManager::Systemd => {
            systemctl(["daemon-reload"])?;
            if !options.no_enable {
                systemctl(once("enable").chain(unit_names()))?;
            }
        }
        InstalledManager::Openrc if !options.no_enable => {
            rc_update(["add", OPENRC_SERVICE, OPENRC_RUNLEVEL])?;
        }
        InstalledManager::Openrc => {}
    }
    if let Some(hint) = options.initrd.install_hint() {
        println!("{hint}");
    }
    Ok(())
}

fn installed_initrd_hooks(target: &InstallTarget) -> Vec<InitrdGenerator> {
    INITRD_GENERATORS
        .into_iter()
        .filter(|generator| {
            initrd_hook_files(*generator)
                .iter()
                .any(|file| target.staged(&file.destination).exists())
        })
        .collect()
}

pub fn uninstall_files(options: &UninstallOptions) -> Result<(), InstallError> {
    let target = &options.target;
    if target.needs_sudo() {
        return rerun_with_sudo();
    }
    let removed_initrd_hooks = installed_initrd_hooks(target);
    if target.changes_live_system() {
        let installed_units: Vec<&str> = unit_names()
            .filter(|unit| target.unit_path(unit).exists())
            .collect();
        if !installed_units.is_empty() {
            systemctl(once("disable").chain(installed_units))?;
        }
        if Path::new("/etc/runlevels")
            .join(OPENRC_RUNLEVEL)
            .join(OPENRC_SERVICE)
            .exists()
        {
            rc_update(["del", OPENRC_SERVICE, OPENRC_RUNLEVEL])?;
        }
    }
    target
        .every_installed_path()
        .into_iter()
        .try_for_each(|path| remove_if_present(target.staged(&path)))?;
    let _ = fs::remove_dir(target.staged(&target.unit_path(RESUME_DROP_IN_DIR)));
    let dracut_module_dir = target.staged(Path::new(DRACUT_MODULE_DIR));
    match fs::remove_dir(&dracut_module_dir) {
        Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => {
            return Err(InstallError::File {
                path: dracut_module_dir,
                source: error,
            });
        }
        _ => {}
    }
    if target.changes_live_system() {
        if target.installed_manager() == InstalledManager::Systemd {
            systemctl(["daemon-reload"])?;
        }
        removed_initrd_hooks
            .into_iter()
            .filter_map(InitrdGenerator::removal_hint)
            .for_each(|hint| println!("{hint}"));
    }
    Ok(())
}
