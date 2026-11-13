use tracing::info;

use crate::console_session::console_palette::ConsoleLook;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refresh {
    Steady,
    Repaint,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnConsole {
    Virtual(u16),
    Elsewhere,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foreground {
    Holding,
    StayingBehind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lending {
    Lent,
    Kept,
    Returned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerPresence {
    Present,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelLevel {
    Quiet,
    Raised,
    Untouched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForegroundOutcome {
    Taken,
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalSwitch {
    Ignored,
    TakenAway,
}

pub struct ConsoleClaim {
    pub own_console: OwnConsole,
    pub foreground: Foreground,
    pub lending: Lending,
    pub look: ConsoleLook,
    pub refresh: Refresh,
}

impl ConsoleClaim {
    #[must_use]
    pub fn new(
        own_console: OwnConsole,
        foreground: Foreground,
        marker: MarkerPresence,
        look: ConsoleLook,
    ) -> Self {
        Self {
            own_console,
            foreground,
            lending: match marker {
                MarkerPresence::Present => Lending::Lent,
                MarkerPresence::Absent => Lending::Kept,
            },
            look,
            refresh: Refresh::Steady,
        }
    }

    pub fn request(&mut self, refresh: Refresh) {
        self.refresh = self.refresh.max(refresh);
    }

    #[must_use]
    pub fn is_lent(&self) -> bool {
        self.lending == Lending::Lent
    }

    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.foreground == Foreground::Holding
    }

    #[must_use]
    pub fn terminal_switched(&self, active: u16) -> TerminalSwitch {
        match (self.own_console, self.foreground, self.lending) {
            (OwnConsole::Virtual(own), Foreground::Holding, Lending::Kept | Lending::Returned)
                if own != active =>
            {
                TerminalSwitch::TakenAway
            }
            _ => TerminalSwitch::Ignored,
        }
    }

    pub fn reclaimed(&mut self) {
        self.request(Refresh::Repaint);
    }

    pub fn marker_seen(&mut self, marker: MarkerPresence) {
        self.lending = match (marker, self.lending) {
            (MarkerPresence::Present, _) => Lending::Lent,
            (MarkerPresence::Absent, Lending::Lent) => {
                self.request(Refresh::Repaint);
                Lending::Returned
            }
            (MarkerPresence::Absent, kept) => kept,
        };
    }

    #[must_use]
    pub fn inspected(&mut self, kernel: KernelLevel, look: ConsoleLook) -> KernelLevel {
        if self.is_lent() {
            return KernelLevel::Untouched;
        }
        if look != self.look {
            self.look = look;
            self.request(Refresh::Repaint);
        }
        if kernel == KernelLevel::Raised {
            self.request(Refresh::Repaint);
        }
        kernel
    }

    #[must_use]
    pub fn comes_forward(&self, prompts_elsewhere: usize) -> bool {
        self.foreground == Foreground::StayingBehind
            && matches!(self.own_console, OwnConsole::Virtual(_))
            && prompts_elsewhere == 0
    }

    pub fn foreground_taken(&mut self, outcome: ForegroundOutcome) {
        if outcome == ForegroundOutcome::Taken {
            info!("password prompt answered, taking the console back");
            self.foreground = Foreground::Holding;
            self.request(Refresh::Clear);
        }
    }

    pub fn release(&mut self) {
        self.foreground = Foreground::StayingBehind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holding_console() -> ConsoleClaim {
        ConsoleClaim::new(
            OwnConsole::Virtual(12),
            Foreground::Holding,
            MarkerPresence::Absent,
            ConsoleLook::default(),
        )
    }

    #[test]
    fn another_terminal_in_front_counts_only_while_holding_and_kept() {
        let mut console = holding_console();
        assert_eq!(console.terminal_switched(12), TerminalSwitch::Ignored);
        assert_eq!(console.terminal_switched(1), TerminalSwitch::TakenAway);
        console.marker_seen(MarkerPresence::Present);
        assert_eq!(console.terminal_switched(1), TerminalSwitch::Ignored);
        console.marker_seen(MarkerPresence::Absent);
        console.release();
        assert_eq!(console.terminal_switched(1), TerminalSwitch::Ignored);
    }

    #[test]
    fn the_console_is_repainted_only_after_it_was_lent_for_resume() {
        let mut console = holding_console();
        console.marker_seen(MarkerPresence::Present);
        assert!(console.is_lent());
        assert_eq!(console.refresh, Refresh::Steady);
        console.marker_seen(MarkerPresence::Absent);
        assert!(!console.is_lent());
        assert_eq!(console.refresh, Refresh::Repaint);
    }

    #[test]
    fn a_raised_kernel_level_is_left_alone_while_the_console_is_lent() {
        let mut console = holding_console();
        console.marker_seen(MarkerPresence::Present);
        assert_eq!(
            console.inspected(KernelLevel::Raised, ConsoleLook::default()),
            KernelLevel::Untouched
        );
        console.marker_seen(MarkerPresence::Absent);
        console.refresh = Refresh::Steady;
        assert_eq!(
            console.inspected(KernelLevel::Raised, ConsoleLook::default()),
            KernelLevel::Raised
        );
        assert_eq!(console.refresh, Refresh::Repaint);
    }

    #[test]
    fn coming_forward_after_another_agents_prompt_clears_the_screen() {
        let mut console = holding_console();
        console.release();
        assert!(!console.comes_forward(1));
        assert!(console.comes_forward(0));
        console.foreground_taken(ForegroundOutcome::Taken);
        assert!(console.is_holding());
        assert_eq!(console.refresh, Refresh::Clear);
    }
}
