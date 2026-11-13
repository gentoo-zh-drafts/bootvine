use std::{
    iter::once,
    path::{Path, PathBuf},
};

const NIXOS_SYSTEM: &str = "nixos-system-";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BootOrigin {
    pub host: String,
    pub system: Option<String>,
    pub generation: Option<String>,
    pub kernel: String,
}

impl BootOrigin {
    #[must_use]
    pub fn parts(&self) -> Vec<String> {
        once(self.host.clone())
            .chain(self.system.clone())
            .chain(
                self.generation
                    .as_ref()
                    .map(|generation| format!("generation {generation}")),
            )
            .chain(once(self.kernel.clone()))
            .collect()
    }
}

#[must_use]
pub fn init_system(command_line: &str) -> Option<&str> {
    command_line
        .split_whitespace()
        .find_map(|argument| argument.strip_prefix("init="))
        .and_then(|init| init.strip_suffix("/init"))
}

#[must_use]
pub fn nixos_version<'a>(system_path: &'a str, host: &str) -> Option<&'a str> {
    let (_, name) = system_path.rsplit_once('/')?;
    let (_, rest) = name.split_once('-')?;
    rest.strip_prefix(NIXOS_SYSTEM)?
        .strip_prefix(host)?
        .strip_prefix('-')
}

#[must_use]
pub fn pretty_name(os_release: &str) -> Option<String> {
    os_release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_owned())
}

pub struct SystemLink {
    pub generation: String,
    pub target: PathBuf,
}

#[must_use]
pub fn generation_of(system_path: &Path, links: &[SystemLink]) -> Option<String> {
    links
        .iter()
        .find(|link| link.target == system_path)
        .map(|link| link.generation.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND_LINE: &str = "initrd=\\EFI\\nixos\\x-initrd.efi \
                                init=/nix/store/\
                                av1146s7ayap8zgrpindi641v1fmdxsf-nixos-system-ryzen-26.11.\
                                20261007.39ad350/init panic=10 root=fstab";

    #[test]
    fn the_nixos_version_comes_from_the_init_on_the_command_line() {
        let system = init_system(COMMAND_LINE).expect("init system");
        assert_eq!(
            nixos_version(system, "ryzen"),
            Some("26.11.20261007.39ad350")
        );
        assert_eq!(nixos_version(system, "other-host"), None);
    }

    #[test]
    fn the_generation_is_the_profile_link_to_the_booted_system() {
        let system = init_system(COMMAND_LINE).expect("init system");
        let links = [
            SystemLink {
                generation: "2770".to_owned(),
                target: "/nix/store/x43q-nixos-system-ryzen-26.11".into(),
            },
            SystemLink {
                generation: "2771".to_owned(),
                target: system.into(),
            },
        ];
        assert_eq!(
            generation_of(Path::new(system), &links),
            Some("2771".to_owned())
        );
        assert_eq!(generation_of(Path::new(system), &links[..1]), None);
    }

    #[test]
    fn other_distributions_use_their_pretty_name() {
        assert_eq!(
            pretty_name("NAME=Arch\nPRETTY_NAME=\"Arch Linux\"\n"),
            Some("Arch Linux".to_owned())
        );
    }

    #[test]
    fn unknown_parts_are_left_out() {
        let origin = BootOrigin {
            host: "ryzen".to_owned(),
            system: Some("NixOS 26.11".to_owned()),
            generation: None,
            kernel: "Linux 7.2.9".to_owned(),
        };
        assert_eq!(origin.parts(), ["ryzen", "NixOS 26.11", "Linux 7.2.9"]);
    }
}
