use std::{
    fs::{read_dir, read_link, read_to_string},
    io::ErrorKind,
    path::Path,
};

use bootvine_screen::boot_model::boot_origin::{
    BootOrigin, SystemLink, generation_of, init_system, nixos_version, pretty_name,
};
use rustix::system::uname;

const KERNEL_COMMAND_LINE: &str = "/proc/cmdline";
const SYSTEM_PROFILES: &str = "/nix/var/nix/profiles";
const OS_RELEASES: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

#[must_use]
pub fn read_boot_origin() -> BootOrigin {
    let names = uname();
    let host = names.nodename().to_string_lossy().into_owned();
    let command_line =
        read_to_string(KERNEL_COMMAND_LINE).expect("the kernel command line is readable");
    let system_path = init_system(&command_line);
    let system = system_path
        .and_then(|path| nixos_version(path, &host))
        .map(|version| format!("NixOS {version}"))
        .or_else(|| {
            OS_RELEASES
                .iter()
                .find_map(|path| match read_to_string(path) {
                    Err(error) if error.kind() == ErrorKind::NotFound => None,
                    contents => Some(contents.expect("os-release is readable")),
                })
                .and_then(|release| pretty_name(&release))
        });
    let generation = system_path.and_then(|path| generation_of(Path::new(path), &system_links()));
    BootOrigin {
        kernel: format!("Linux {}", names.release().to_string_lossy()),
        host,
        system,
        generation,
    }
}

fn system_links() -> Vec<SystemLink> {
    let entries = match read_dir(SYSTEM_PROFILES) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        listed => listed.expect("the nix system profiles are listable"),
    };
    entries
        .map(|entry| entry.expect("entries of the nix system profiles are readable"))
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .into_string()
                .expect("nix profile names are UTF-8");
            let generation = name.strip_prefix("system-")?.strip_suffix("-link")?;
            Some(SystemLink {
                generation: generation.to_owned(),
                target: read_link(entry.path()).expect("system generation links are symlinks"),
            })
        })
        .collect()
}
