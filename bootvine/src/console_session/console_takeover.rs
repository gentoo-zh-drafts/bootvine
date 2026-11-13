use std::{collections::HashSet, fs, io, mem, os::fd::BorrowedFd, ptr};

use rustix::{
    io::Errno,
    ioctl::{Getter, IntegerSetter, Opcode, Updater, ioctl},
    stdio::stdin,
    termios::ttyname,
};
use tracing::{info, warn};

use crate::{
    console_session::{
        ask_password,
        console_palette::{ConsoleLook, ConsolePalette},
    },
    screen_components::console_claim::KernelLevel,
    screen_settings::font_glyphs::Glyphs,
};

const GIO_UNIMAP: Opcode = 0x4B66;
const VT_GETSTATE: Opcode = 0x5603;
const VT_ACTIVATE: Opcode = 0x5606;
const VIRTUAL_TERMINAL_PREFIX: &str = "/dev/tty";
pub(crate) const KERNEL_PRINTK: &str = "/proc/sys/kernel/printk";
const ALERTS_ONLY: &str = "1";
const HIGHEST_LOW_GLYPH: u16 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StdinTerminal {
    VirtualConsole(u16),
    OtherTerminal,
    NotATerminal,
}

impl TryFrom<BorrowedFd<'_>> for StdinTerminal {
    type Error = Errno;

    fn try_from(terminal: BorrowedFd<'_>) -> Result<Self, Errno> {
        let device = match ttyname(terminal, Vec::new()) {
            Err(Errno::NOTTY) => return Ok(Self::NotATerminal),
            named => named?,
        };
        let device = device.to_str().expect("terminal device names are ASCII");
        let Some(digits) = device
            .strip_prefix(VIRTUAL_TERMINAL_PREFIX)
            .filter(|digits| {
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
        else {
            return Ok(Self::OtherTerminal);
        };
        Ok(
            match digits
                .parse()
                .expect("virtual console numbers fit in sixteen bits")
            {
                0 => Self::OtherTerminal,
                number => Self::VirtualConsole(number),
            },
        )
    }
}

pub(super) struct QuietKernelConsole {
    previous_level: String,
}

pub(super) enum KernelConsole {
    Quieted(QuietKernelConsole),
    LeftAsIs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KernelLoudness {
    Unchanged,
    QuietedAgain,
}

pub(crate) fn console_level() -> io::Result<String> {
    let levels = fs::read_to_string(KERNEL_PRINTK)?;
    Ok(levels
        .split_whitespace()
        .next()
        .expect("printk lists the console log levels")
        .to_owned())
}

impl QuietKernelConsole {
    fn take() -> io::Result<Self> {
        let previous_level = console_level()?;
        fs::write(KERNEL_PRINTK, ALERTS_ONLY)?;
        Ok(Self { previous_level })
    }

    fn keep_quiet(&mut self) -> KernelLoudness {
        let level = console_level().expect("printk stays readable once bootvine quieted it");
        if level == ALERTS_ONLY {
            return KernelLoudness::Unchanged;
        }
        fs::write(KERNEL_PRINTK, ALERTS_ONLY)
            .expect("printk stays writable once bootvine quieted it");
        self.previous_level = level;
        KernelLoudness::QuietedAgain
    }
}

impl KernelConsole {
    pub(super) fn quiet() -> Self {
        match QuietKernelConsole::take() {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Self::LeftAsIs,
            taken => {
                Self::Quieted(taken.expect("the kernel console level is readable and writable"))
            }
        }
    }

    pub(super) fn keep_quiet(&mut self) -> KernelLoudness {
        match self {
            Self::Quieted(quiet) => quiet.keep_quiet(),
            Self::LeftAsIs => KernelLoudness::Unchanged,
        }
    }

    pub(super) fn level(&self) -> KernelLevel {
        match self {
            Self::Quieted(_)
                if console_level().expect("printk stays readable once bootvine quieted it")
                    == ALERTS_ONLY =>
            {
                KernelLevel::Quiet
            }
            Self::Quieted(_) => KernelLevel::Raised,
            Self::LeftAsIs => KernelLevel::Untouched,
        }
    }
}

impl Drop for QuietKernelConsole {
    fn drop(&mut self) {
        if let Err(error) = fs::write(KERNEL_PRINTK, &self.previous_level) {
            warn!(error = %error, level = %self.previous_level, "could not restore the kernel console level");
        }
    }
}

struct LoadedFont {
    characters: HashSet<char>,
    uses_high_glyphs: bool,
}

enum ConsoleFont {
    Mapped(LoadedFont),
    Unmapped,
}

pub(super) fn console_look(terminal: StdinTerminal) -> ConsoleLook {
    let StdinTerminal::VirtualConsole(_) = terminal else {
        return ConsoleLook {
            glyphs: Glyphs::default(),
            palette: ConsolePalette::Terminal,
        };
    };
    match loaded_font().expect("the font of bootvine's own virtual console is readable") {
        ConsoleFont::Mapped(font) => ConsoleLook {
            glyphs: Glyphs::fitting(|symbol| {
                symbol.is_ascii() || font.characters.contains(&symbol)
            }),
            palette: if font.uses_high_glyphs {
                ConsolePalette::EightColorVt
            } else {
                ConsolePalette::SixteenColorVt
            },
        },
        ConsoleFont::Unmapped => ConsoleLook {
            glyphs: Glyphs::default(),
            palette: ConsolePalette::SixteenColorVt,
        },
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UnicodePair {
    unicode: u16,
    font_position: u16,
}

#[repr(C)]
struct UnicodeMap {
    count: u16,
    pairs: *mut UnicodePair,
}

fn read_unicode_map(map: &mut UnicodeMap) -> Result<(), Errno> {
    unsafe { ioctl(stdin(), Updater::<GIO_UNIMAP, UnicodeMap>::new(map)) }
}

fn loaded_font() -> Result<ConsoleFont, Errno> {
    let mut probe = UnicodeMap {
        count: 0,
        pairs: ptr::null_mut(),
    };
    match read_unicode_map(&mut probe) {
        Ok(()) | Err(Errno::NOMEM) => {}
        Err(error) => return Err(error),
    }
    if probe.count == 0 {
        return Ok(ConsoleFont::Unmapped);
    }
    let mut pairs = vec![UnicodePair::default(); usize::from(probe.count)];
    let mut map = UnicodeMap {
        count: probe.count,
        pairs: pairs.as_mut_ptr(),
    };
    read_unicode_map(&mut map)?;
    let mapped = &pairs[..usize::from(map.count).min(pairs.len())];
    Ok(ConsoleFont::Mapped(LoadedFont {
        characters: mapped
            .iter()
            .filter_map(|pair| char::from_u32(u32::from(pair.unicode)))
            .collect(),
        uses_high_glyphs: mapped
            .iter()
            .any(|pair| pair.font_position > HIGHEST_LOW_GLYPH),
    }))
}

#[repr(C)]
struct VtState {
    active: u16,
    _signal: u16,
    _state: u16,
}

pub(super) struct ForegroundVt {
    own: u16,
    previous: u16,
}

pub(super) enum HeldTerminal {
    Holding(ForegroundVt),
    StayingBehind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VtOwnership {
    StillOurs,
    TakenAway,
}

fn active_vt() -> Result<u16, Errno> {
    let state: VtState = unsafe { ioctl(stdin(), Getter::<VT_GETSTATE, VtState>::new()) }?;
    Ok(state.active)
}

impl ForegroundVt {
    fn take(own: u16) -> Result<Self, Errno> {
        let previous = active_vt()?;
        if previous != own {
            activate(own)?;
        }
        Ok(Self { own, previous })
    }

    pub(super) fn ownership(&self) -> VtOwnership {
        if active_vt().expect("bootvine's own virtual console answers VT_GETSTATE") == self.own {
            VtOwnership::StillOurs
        } else {
            VtOwnership::TakenAway
        }
    }

    pub(super) fn insist(&self) -> VtOwnership {
        let ownership = self.ownership();
        if ownership == VtOwnership::TakenAway {
            activate(self.own).expect("bootvine switched to its own virtual console before");
        }
        ownership
    }

    fn stay_switched(self) {
        mem::forget(self);
    }
}

impl HeldTerminal {
    pub(super) fn take(terminal: StdinTerminal) -> Self {
        let StdinTerminal::VirtualConsole(own) = terminal else {
            return Self::StayingBehind;
        };
        match ForegroundVt::take(own) {
            Err(Errno::PERM | Errno::ACCESS) => Self::StayingBehind,
            taken => Self::Holding(taken.expect("bootvine can switch to its own virtual console")),
        }
    }

    pub(super) fn stay_switched(&mut self) {
        if let Self::Holding(foreground) = mem::replace(self, Self::StayingBehind) {
            foreground.stay_switched();
        }
    }
}

impl Drop for ForegroundVt {
    fn drop(&mut self) {
        if ask_password::requested() {
            info!("leaving a password prompt to the console agent on this terminal");
            return;
        }
        if let Err(error) = activate(self.previous) {
            warn!(error = %error, vt = self.previous, "could not switch back to the previous virtual console");
        }
    }
}

fn activate(number: u16) -> Result<(), Errno> {
    let number = usize::from(number);
    unsafe { ioctl(stdin(), IntegerSetter::<VT_ACTIVATE>::new_usize(number)) }
}
