use std::os::fd::OwnedFd;

use rustix::{
    fs::{Mode, OFlags, open},
    io::Errno,
    process::{chdir, chroot, fchdir},
};

const SYSROOT: &str = "/sysroot";
pub(crate) const INITRD_RELEASE: &str = "/etc/initrd-release";

pub(crate) struct Sysroot {
    directory: OwnedFd,
}

impl Sysroot {
    pub(crate) fn hold() -> Result<Self, Errno> {
        let directory = open(
            SYSROOT,
            OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self { directory })
    }

    pub(crate) fn enter(&self) -> Result<(), Errno> {
        fchdir(&self.directory)?;
        chroot(".")?;
        chdir("/")
    }
}
