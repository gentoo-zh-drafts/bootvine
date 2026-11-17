use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    INSTALL_GUIDE, RUNNING_VERSION, command_stdout, health_findings::HealthFinding, read_file,
};
use crate::system_install::{
    install_error::InstallError,
    install_files::{PACKAGED_BINARY_PATH, PackagedUnit},
};

enum UnitFragment {
    Installed(PathBuf),
    NotInstalled,
}

#[derive(Debug, PartialEq, Eq)]
enum UnitEnablement {
    Enabled,
    NotEnabled,
}

impl From<&str> for UnitEnablement {
    fn from(enablement_state: &str) -> Self {
        match enablement_state {
            "enabled" | "enabled-runtime" | "linked" | "linked-runtime" => Self::Enabled,
            _ => Self::NotEnabled,
        }
    }
}

fn unit_fragment(unit_name: &str) -> Result<UnitFragment, InstallError> {
    let fragment = command_stdout(
        "systemctl",
        &["show", "--property=FragmentPath", "--value", unit_name],
    )?;
    if fragment.is_empty() {
        return Ok(UnitFragment::NotInstalled);
    }
    let fragment_path = fs::canonicalize(&fragment).map_err(|source| InstallError::File {
        path: PathBuf::from(&fragment),
        source,
    })?;
    Ok(UnitFragment::Installed(fragment_path))
}

fn expected_unit_contents(unit: &PackagedUnit, fragment_path: &Path) -> String {
    let prefix = fragment_path
        .ancestors()
        .nth(4)
        .expect("unit files live in <prefix>/lib/systemd/system");
    unit.contents.replace(
        PACKAGED_BINARY_PATH,
        prefix
            .join("bin/bootvine")
            .to_str()
            .expect("unit file paths are UTF-8"),
    )
}

pub(super) fn installed_unit_finding(unit: &PackagedUnit) -> Result<HealthFinding, InstallError> {
    let UnitFragment::Installed(fragment_path) = unit_fragment(unit.name)? else {
        return Ok(HealthFinding::unhealthy(
            format!("{} is not installed", unit.name),
            format!("install bootvine as described in {INSTALL_GUIDE}"),
        ));
    };
    if read_file(&fragment_path)? == expected_unit_contents(unit, &fragment_path) {
        return Ok(HealthFinding::healthy(format!(
            "{} matches bootvine {RUNNING_VERSION}",
            unit.name
        )));
    }
    Ok(HealthFinding::unhealthy(
        format!(
            "{} differs from the {} of bootvine {RUNNING_VERSION}",
            fragment_path.display(),
            unit.name
        ),
        "reinstall the units with `bootvine install` or by updating the package, then run \
         `systemctl daemon-reload`"
            .to_owned(),
    ))
}

pub(super) fn enabled_unit_finding(unit: &PackagedUnit) -> Result<HealthFinding, InstallError> {
    let enablement_state = command_stdout("systemctl", &["is-enabled", unit.name])?;
    let statement = format!("{} is {enablement_state}", unit.name);
    Ok(match UnitEnablement::from(enablement_state.as_str()) {
        UnitEnablement::Enabled => HealthFinding::healthy(statement),
        UnitEnablement::NotEnabled => {
            HealthFinding::unhealthy(statement, format!("systemctl enable {}", unit.name))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_install::install_files::PACKAGED_UNITS;

    #[test]
    fn enabled_and_linked_units_count_as_enabled() {
        ["enabled", "enabled-runtime", "linked", "linked-runtime"]
            .into_iter()
            .for_each(|state| assert_eq!(UnitEnablement::from(state), UnitEnablement::Enabled));
        ["disabled", "masked", "static", "not-found"]
            .into_iter()
            .for_each(|state| assert_eq!(UnitEnablement::from(state), UnitEnablement::NotEnabled));
    }

    #[test]
    fn the_expected_unit_runs_the_binary_of_its_prefix() {
        let expected = expected_unit_contents(
            &PACKAGED_UNITS[0],
            Path::new("/nix/store/hash-bootvine/lib/systemd/system/bootvine.service"),
        );
        assert!(expected.contains("/nix/store/hash-bootvine/bin/bootvine"));
        assert!(!expected.contains(PACKAGED_BINARY_PATH));
    }
}
