use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveState {
    Activating,
    Active,
    Deactivating,
    Reloading,
    Failed,
    Inactive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobResult {
    Done,
    Skipped,
    Canceled,
    Collected,
    Timeout,
    Dependency,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitTimeouts {
    pub start: Option<Duration>,
    pub stop: Option<Duration>,
    pub job: Option<Duration>,
    pub job_running: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitFacts {
    pub name: String,
    pub short_name: String,
    pub kind: UnitKind,
    pub description: Option<String>,
    pub after: Vec<String>,
    pub before: Vec<String>,
    pub timeouts: UnitTimeouts,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub state: Option<ActiveState>,
    pub inactive_exit: Option<Duration>,
    pub active_enter: Option<Duration>,
    pub active_exit: Option<Duration>,
}

impl Activity {
    pub(super) fn merge(self, newer: Self) -> Self {
        Self {
            state: newer.state.or(self.state),
            inactive_exit: newer.inactive_exit.or(self.inactive_exit),
            active_enter: newer.active_enter.or(self.active_enter),
            active_exit: newer.active_exit.or(self.active_exit),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    TakesDown,
    Unknown,
    BringsUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Service,
    Target,
    Socket,
    Mount,
    Automount,
    Device,
    Swap,
    Path,
    Timer,
    Slice,
    Scope,
    Other,
}

impl UnitKind {
    #[must_use]
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Self::Service | Self::Target | Self::Other => None,
            Self::Socket => Some("Socket"),
            Self::Mount => Some("Mount"),
            Self::Automount => Some("Automount"),
            Self::Device => Some("Device"),
            Self::Swap => Some("Swap"),
            Self::Path => Some("Path"),
            Self::Timer => Some("Timer"),
            Self::Slice => Some("Slice"),
            Self::Scope => Some("Scope"),
        }
    }
}

fn description_label(facts: &UnitFacts) -> Option<&String> {
    facts.description.as_ref().filter(|_| {
        matches!(
            facts.kind,
            UnitKind::Device | UnitKind::Mount | UnitKind::Swap | UnitKind::Automount
        )
    })
}

pub(super) fn display_label(facts: &UnitFacts) -> String {
    description_label(facts).map_or_else(|| facts.short_name.clone(), Clone::clone)
}

pub(super) fn summary(facts: &UnitFacts) -> Option<String> {
    let repeats_the_name = facts.description.as_deref().is_some_and(|description| {
        description == facts.name || description.eq_ignore_ascii_case(&display_label(facts))
    });
    if description_label(facts).is_some() || repeats_the_name {
        return None;
    }
    facts.description.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot_test_fixtures::{facts, waiting};

    #[test]
    fn labels_use_the_short_name_unless_a_device_is_described() {
        assert_eq!(display_label(&facts("docker.socket", &[])), "docker");
        assert_eq!(display_label(&facts("basic.target", &[])), "basic.target");
        assert_eq!(
            display_label(
                &waiting("dev-sda.device", &[])
                    .described("Samsung SSD")
                    .facts
            ),
            "Samsung SSD"
        );
        assert_eq!(UnitKind::Socket.tag(), Some("Socket"));
        assert_eq!(UnitKind::Target.tag(), None);
    }

    #[test]
    fn descriptions_that_repeat_the_label_are_dropped() {
        let described = |name: &str, description: &str| {
            summary(&waiting(name, &[]).described(description).facts)
        };
        assert_eq!(
            described("multi-user.target", "Multi-User System"),
            Some("Multi-User System".to_owned())
        );
        assert_eq!(described("greetd.service", "greetd.service"), None);
        assert_eq!(described("greetd.service", "Greetd"), None);
        assert_eq!(described("dev-sda.device", "Samsung SSD"), None);
        assert_eq!(described("demo.service", ""), None);
    }
}
