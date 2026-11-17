use std::{
    collections::BTreeSet,
    fs,
    io::{self, ErrorKind},
    iter::once,
    path::PathBuf,
};

use super::{InitrdHookCheck, InitrdSetup, health_findings::HealthFinding, read_file};
use crate::system_install::install_error::InstallError;

const MKINITCPIO_CONFIG: &str = "/etc/mkinitcpio.conf";
const MKINITCPIO_CONFIG_DIRECTORY: &str = "/etc/mkinitcpio.conf.d";
const SYSTEMD_HOOK: &str = "systemd";
const BOOTVINE_HOOK: &str = "sd-bootvine";

#[derive(Debug, PartialEq, Eq)]
enum HookPlacement {
    AfterSystemd,
    BeforeSystemd,
    WithoutSystemd,
    Absent,
}

impl From<&[String]> for HookPlacement {
    fn from(hooks: &[String]) -> Self {
        let position_of = |wanted: &str| hooks.iter().position(|hook| hook == wanted);
        match (position_of(SYSTEMD_HOOK), position_of(BOOTVINE_HOOK)) {
            (_, None) => Self::Absent,
            (None, Some(_)) => Self::WithoutSystemd,
            (Some(systemd_position), Some(bootvine_position))
                if bootvine_position > systemd_position =>
            {
                Self::AfterSystemd
            }
            (Some(_), Some(_)) => Self::BeforeSystemd,
        }
    }
}

fn hook_assignments(config_text: &str) -> Vec<Vec<String>> {
    let uncommented = config_text
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n{uncommented}")
        .split("\nHOOKS=(")
        .skip(1)
        .map(|assignment| {
            assignment
                .split(')')
                .next()
                .expect("split yields at least one piece")
                .split_whitespace()
                .map(|hook| hook.trim_matches(['"', '\'']).to_owned())
                .collect()
        })
        .collect()
}

fn configured_placement(config_texts: &[String]) -> HookPlacement {
    match config_texts
        .iter()
        .flat_map(|config_text| hook_assignments(config_text))
        .last()
    {
        Some(hooks) => HookPlacement::from(hooks.as_slice()),
        None => HookPlacement::Absent,
    }
}

fn drop_in_paths() -> Result<BTreeSet<PathBuf>, InstallError> {
    let directory_error = |source| InstallError::File {
        path: PathBuf::from(MKINITCPIO_CONFIG_DIRECTORY),
        source,
    };
    let entries = match fs::read_dir(MKINITCPIO_CONFIG_DIRECTORY) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeSet::new()),
        listed => listed.map_err(directory_error)?,
    };
    entries
        .map(|entry| -> io::Result<Option<PathBuf>> {
            let path = entry?.path();
            Ok(path
                .extension()
                .is_some_and(|extension| extension == "conf")
                .then_some(path))
        })
        .filter_map(Result::transpose)
        .collect::<io::Result<_>>()
        .map_err(directory_error)
}

fn config_texts() -> Result<Vec<String>, InstallError> {
    once(PathBuf::from(MKINITCPIO_CONFIG))
        .chain(drop_in_paths()?)
        .map(|path| read_file(&path))
        .collect()
}

pub(super) fn mkinitcpio_hook_check() -> Result<InitrdHookCheck, InstallError> {
    Ok(match configured_placement(&config_texts()?) {
        HookPlacement::AfterSystemd => InitrdHookCheck {
            finding: HealthFinding::healthy(format!(
                "HOOKS has {BOOTVINE_HOOK} after {SYSTEMD_HOOK}"
            )),
            initrd_setup: InitrdSetup::Prepared,
        },
        HookPlacement::Absent => InitrdHookCheck {
            finding: HealthFinding::advice(
                format!("HOOKS has no {BOOTVINE_HOOK}, so the initrd is not shown"),
                format!(
                    "add {BOOTVINE_HOOK} after {SYSTEMD_HOOK} in HOOKS in {MKINITCPIO_CONFIG}, \
                     then run mkinitcpio -P"
                ),
            ),
            initrd_setup: InitrdSetup::Unprepared,
        },
        HookPlacement::BeforeSystemd => InitrdHookCheck {
            finding: HealthFinding::unhealthy(
                format!("HOOKS has {BOOTVINE_HOOK} before {SYSTEMD_HOOK}"),
                format!(
                    "move {BOOTVINE_HOOK} after {SYSTEMD_HOOK} in HOOKS, then run mkinitcpio -P"
                ),
            ),
            initrd_setup: InitrdSetup::Prepared,
        },
        HookPlacement::WithoutSystemd => InitrdHookCheck {
            finding: HealthFinding::unhealthy(
                format!("HOOKS has {BOOTVINE_HOOK} but no {SYSTEMD_HOOK} hook"),
                format!(
                    "bootvine needs a systemd initramfs: use the {SYSTEMD_HOOK} hook, or remove \
                     {BOOTVINE_HOOK}"
                ),
            ),
            initrd_setup: InitrdSetup::Unprepared,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_last_uncommented_hooks_assignment_counts() {
        let config_text = "\
#HOOKS=(base udev sd-bootvine)
HOOKS=(base systemd autodetect
       'sd-bootvine' block filesystems)
MODULES=()
";
        assert_eq!(
            hook_assignments(config_text),
            vec![hooks(&[
                "base",
                "systemd",
                "autodetect",
                "sd-bootvine",
                "block",
                "filesystems"
            ])]
        );
        assert!(hook_assignments("MODULES=()\n").is_empty());
    }

    #[test]
    fn a_drop_in_overrides_the_main_config() {
        let config_texts = [
            "HOOKS=(base systemd sd-bootvine)\n".to_owned(),
            "HOOKS=(base udev)\n".to_owned(),
        ];
        assert_eq!(configured_placement(&config_texts), HookPlacement::Absent);
        assert_eq!(configured_placement(&[]), HookPlacement::Absent);
    }

    #[test]
    fn hook_placement_needs_the_systemd_hook_first() {
        assert_eq!(
            HookPlacement::from(hooks(&["base", "systemd", "sd-bootvine"]).as_slice()),
            HookPlacement::AfterSystemd
        );
        assert_eq!(
            HookPlacement::from(hooks(&["base", "sd-bootvine", "systemd"]).as_slice()),
            HookPlacement::BeforeSystemd
        );
        assert_eq!(
            HookPlacement::from(hooks(&["base", "udev", "sd-bootvine"]).as_slice()),
            HookPlacement::WithoutSystemd
        );
        assert_eq!(
            HookPlacement::from(hooks(&["base", "systemd"]).as_slice()),
            HookPlacement::Absent
        );
    }
}
