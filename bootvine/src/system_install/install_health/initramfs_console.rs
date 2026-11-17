use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    process::{self, Command},
};

use super::{ISSUE_TRACKER, REBUILD_INITRAMFS, health_findings::HealthFinding};
use crate::system_install::install_error::InstallError;

const UNIT_DIRECTORIES_IN_INITRAMFS: [&str; 2] = ["usr/lib/systemd/system", "etc/systemd/system"];
const BOOT_UNIT_IN_INITRAMFS: &str = "usr/lib/systemd/system/bootvine.service";

#[derive(Debug, PartialEq, Eq)]
enum ConsoleUse {
    ReadsConsole,
    LeavesConsole,
}

fn unit_setting<'unit>(unit_text: &'unit str, key: &str) -> Option<&'unit str> {
    unit_text
        .lines()
        .filter_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
        .next_back()
}

fn console_use(unit_text: &str) -> ConsoleUse {
    let reads_terminal = unit_setting(unit_text, "StandardInput")
        .is_some_and(|standard_input| standard_input.starts_with("tty"));
    let on_console = unit_setting(unit_text, "TTYPath")
        .is_none_or(|terminal_path| terminal_path == "/dev/console");
    if reads_terminal && on_console {
        ConsoleUse::ReadsConsole
    } else {
        ConsoleUse::LeavesConsole
    }
}

fn units_ordered_around(unit_text: &str) -> BTreeSet<String> {
    unit_text
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("After=")
                .or_else(|| line.strip_prefix("Conflicts="))
        })
        .flat_map(str::split_whitespace)
        .map(str::to_owned)
        .collect()
}

fn console_unit_name(unit_path: &Path) -> io::Result<Option<String>> {
    if !fs::symlink_metadata(unit_path)?.is_file()
        || unit_path
            .extension()
            .is_none_or(|extension| extension != "service")
    {
        return Ok(None);
    }
    Ok(match console_use(&fs::read_to_string(unit_path)?) {
        ConsoleUse::ReadsConsole => Some(
            unit_path
                .file_name()
                .expect("listed unit files have names")
                .to_str()
                .expect("unit names are UTF-8")
                .to_owned(),
        ),
        ConsoleUse::LeavesConsole => None,
    })
}

fn console_units_in(unit_directory: &Path) -> io::Result<BTreeSet<String>> {
    let entries = match fs::read_dir(unit_directory) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeSet::new()),
        listed => listed?,
    };
    entries
        .map(|entry| console_unit_name(&entry?.path()))
        .filter_map(Result::transpose)
        .collect()
}

fn console_finding_for(unpacked_initramfs: &Path) -> io::Result<HealthFinding> {
    let packed_unit = match fs::read_to_string(unpacked_initramfs.join(BOOT_UNIT_IN_INITRAMFS)) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(HealthFinding::advice(
                "the initramfs has no bootvine".to_owned(),
                REBUILD_INITRAMFS.to_owned(),
            ));
        }
        read => read?,
    };
    let ordered_units = units_ordered_around(&packed_unit);
    let blocked_units = UNIT_DIRECTORIES_IN_INITRAMFS
        .iter()
        .map(|unit_directory| console_units_in(&unpacked_initramfs.join(unit_directory)))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|console_unit| !ordered_units.contains(console_unit))
        .collect::<BTreeSet<_>>();
    if blocked_units.is_empty() {
        return Ok(HealthFinding::healthy(
            "every unit in the initramfs that reads the console runs before bootvine".to_owned(),
        ));
    }
    Ok(HealthFinding::unhealthy(
        format!(
            "the initramfs has units that would wait for the console bootvine holds: {}",
            blocked_units.into_iter().collect::<Vec<_>>().join(", ")
        ),
        format!("{REBUILD_INITRAMFS}; if they remain, report them at {ISSUE_TRACKER}"),
    ))
}

fn unpack_current_initramfs(unpacked_initramfs: &Path) -> Result<(), InstallError> {
    let program = "lsinitrd".to_owned();
    let status = Command::new(&program)
        .arg("--unpack")
        .current_dir(unpacked_initramfs)
        .status()
        .map_err(|source| InstallError::Spawn {
            program: program.clone(),
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(InstallError::Failed { program, status })
    }
}

fn unpacked_console_finding(unpacked_initramfs: &Path) -> Result<HealthFinding, InstallError> {
    unpack_current_initramfs(unpacked_initramfs)?;
    console_finding_for(unpacked_initramfs).map_err(|source| InstallError::File {
        path: unpacked_initramfs.to_owned(),
        source,
    })
}

pub(super) fn initramfs_console_finding() -> Result<HealthFinding, InstallError> {
    let unpacked_initramfs: PathBuf =
        env::temp_dir().join(format!("bootvine-health-{}", process::id()));
    let directory_error = |source| InstallError::File {
        path: unpacked_initramfs.clone(),
        source,
    };
    fs::create_dir_all(&unpacked_initramfs).map_err(directory_error)?;
    let console_finding = unpacked_console_finding(&unpacked_initramfs);
    fs::remove_dir_all(&unpacked_initramfs).map_err(directory_error)?;
    console_finding
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_install::install_files::PACKAGED_UNITS;

    #[test]
    fn only_units_reading_the_console_count() {
        assert_eq!(
            console_use("[Service]\nStandardInput=tty\nStandardOutput=inherit\n"),
            ConsoleUse::ReadsConsole
        );
        assert_eq!(
            console_use("[Service]\nStandardInput=tty-force\nTTYPath=/dev/console\n"),
            ConsoleUse::ReadsConsole
        );
        assert_eq!(
            console_use("[Service]\nStandardInput=tty\nTTYPath=/dev/tty9\n"),
            ConsoleUse::LeavesConsole
        );
        assert_eq!(
            console_use("[Service]\nStandardOutput=tty\n"),
            ConsoleUse::LeavesConsole
        );
    }

    #[test]
    fn the_shipped_unit_starts_after_the_known_console_units() {
        let ordered_units = units_ordered_around(PACKAGED_UNITS[0].contents);
        [
            "dracut-cmdline-ask.service",
            "breakpoint-pre-udev.service",
            "emergency.service",
            "rescue.service",
            "dracut-emergency.service",
        ]
        .iter()
        .for_each(|console_unit| assert!(ordered_units.contains(*console_unit), "{console_unit}"));
    }
}
