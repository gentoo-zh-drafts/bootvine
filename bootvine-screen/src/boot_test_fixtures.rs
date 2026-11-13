use std::{iter::once, time::Duration};

use crate::boot_model::{
    ActiveState, Activity, BootState, JobKind, ManagerVocabulary, Phase, TreeRow, UnitFacts,
    UnitKind, UnitTimeouts, flatten_forest,
};

pub const MS: Duration = Duration::from_millis(1);

pub struct QueuedUnit {
    pub facts: UnitFacts,
    pub activity: Activity,
    pub kind: JobKind,
}

fn fixture_unit_kind(name: &str) -> UnitKind {
    match name.rsplit_once('.').map(|(_, suffix)| suffix) {
        Some("service") => UnitKind::Service,
        Some("target") => UnitKind::Target,
        Some("socket") => UnitKind::Socket,
        Some("mount") => UnitKind::Mount,
        Some("device") => UnitKind::Device,
        Some("swap") => UnitKind::Swap,
        Some("timer") => UnitKind::Timer,
        _ => UnitKind::Other,
    }
}

pub fn facts(name: &str, after: &[&str]) -> UnitFacts {
    let kind = fixture_unit_kind(name);
    let short_name = match kind {
        UnitKind::Target | UnitKind::Other => name.to_owned(),
        _ => name
            .rsplit_once('.')
            .map_or_else(|| name.to_owned(), |(stem, _)| stem.to_owned()),
    };
    UnitFacts {
        name: name.to_owned(),
        short_name,
        kind,
        description: None,
        after: after.iter().map(ToString::to_string).collect(),
        before: Vec::new(),
        timeouts: UnitTimeouts::default(),
    }
}

pub fn activating(since: Duration) -> Activity {
    Activity {
        state: Some(ActiveState::Activating),
        inactive_exit: Some(since),
        ..Activity::default()
    }
}

pub fn waiting(name: &str, after: &[&str]) -> QueuedUnit {
    QueuedUnit {
        facts: facts(name, after),
        activity: Activity::default(),
        kind: JobKind::BringsUp,
    }
}

pub fn running(name: &str, after: &[&str], since: Duration) -> QueuedUnit {
    QueuedUnit {
        activity: activating(since),
        ..waiting(name, after)
    }
}

impl QueuedUnit {
    pub fn stopping(self) -> Self {
        Self {
            kind: JobKind::TakesDown,
            ..self
        }
    }

    pub fn ordered_before(mut self, before: &[&str]) -> Self {
        self.facts.before = before.iter().map(ToString::to_string).collect();
        self
    }

    pub fn timing_out(mut self, timeouts: UnitTimeouts) -> Self {
        self.facts.timeouts = timeouts;
        self
    }

    pub fn described(mut self, description: &str) -> Self {
        self.facts.description = (!description.is_empty()).then(|| description.to_owned());
        self
    }
}

pub const HANDOFF_UNIT: &str = "display-manager.service";

pub fn test_vocabulary() -> ManagerVocabulary {
    ManagerVocabulary {
        host_milestones: [
            "sysinit.target",
            "basic.target",
            "multi-user.target",
            "graphical.target",
        ]
        .map(str::to_owned)
        .to_vec(),
        shutdown_milestones: ["shutdown.target", "umount.target", "final.target"]
            .map(str::to_owned)
            .to_vec(),
        handoff_unit: HANDOFF_UNIT.to_owned(),
        shell_units: ["emergency.service", "rescue.service"]
            .map(str::to_owned)
            .to_vec(),
        shutdown_successor: "systemd-shutdown".to_owned(),
    }
}

pub fn queued_boot(
    phase: Phase,
    queued_at: Duration,
    units: impl IntoIterator<Item = QueuedUnit>,
) -> BootState {
    let mut state = BootState::new(phase, &test_vocabulary());
    units.into_iter().zip(1..).for_each(|(unit, id)| {
        if id == 1 {
            state.set_root(unit.facts.name.clone());
        }
        queue_unit(&mut state, id, unit, queued_at);
    });
    state
}

pub fn nested_boot_sample() -> BootState {
    queued_boot(
        Phase::Host,
        500 * MS,
        [
            waiting("graphical.target", &["multi-user.target"]),
            waiting(
                "multi-user.target",
                &["docker.service", "nm-wait.service", "basic.target"],
            ),
            waiting("nm-wait.service", &["nm.service"]),
            running("nm.service", &[], 1000 * MS),
            running("docker.service", &["containerd.service"], 3000 * MS),
            waiting("containerd.service", &[]),
            waiting("basic.target", &[]),
        ],
    )
}

pub fn queue_unit(state: &mut BootState, id: u32, unit: QueuedUnit, queued_at: Duration) {
    let name = unit.facts.name.clone();
    state.track_unit(unit.facts, unit.activity);
    state.queue_job(id, &name, unit.kind, queued_at);
}

pub fn waiting_rows(state: &BootState) -> Vec<TreeRow> {
    flatten_forest(state.waiting_forest(Duration::MAX))
}

pub fn tree_outline(rows: &[TreeRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let chain = once(row.label.as_str())
                .chain(row.chain.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" > ");
            format!("{}{chain}", "  ".repeat(row.depth))
        })
        .collect()
}
