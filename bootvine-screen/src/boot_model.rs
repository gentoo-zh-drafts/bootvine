mod boot_change;
pub mod boot_origin;
mod job_ingestion;
mod reached_milestones;
mod unit_facts;
mod waiting_tree;

use std::{collections::HashMap, error::Error, fmt, str::FromStr, time::Duration};

pub use self::{
    boot_change::BootChange,
    reached_milestones::MilestoneRow,
    unit_facts::{ActiveState, Activity, JobKind, JobResult, UnitFacts, UnitKind, UnitTimeouts},
    waiting_tree::{RowStatus, TreeNode, TreeRow, flatten_forest},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootRequest {
    Named(String),
    ManagerDefault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    System,
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Initrd,
    Host,
    User,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunRequest {
    Boot,
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootProgress {
    InProgress,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsolePrompt {
    Showing,
    Absent,
}

impl Phase {
    const ALL: [Self; 4] = [Self::Initrd, Self::Host, Self::User, Self::Shutdown];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Initrd => "Initrd",
            Self::Host => "Stage 2",
            Self::User => "User",
            Self::Shutdown => "Shutdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPhaseLabel(pub String);

impl fmt::Display for UnknownPhaseLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no phase is labelled {:?}", self.0)
    }
}

impl Error for UnknownPhaseLabel {}

impl FromStr for Phase {
    type Err = UnknownPhaseLabel;

    fn from_str(label: &str) -> Result<Self, UnknownPhaseLabel> {
        Self::ALL
            .into_iter()
            .find(|phase| phase.label() == label)
            .ok_or_else(|| UnknownPhaseLabel(label.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingJob {
    id: u32,
    queued_at: Duration,
    kind: JobKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Broke,
    TimedOut,
    DependencyFailed,
}

impl FailureKind {
    #[must_use]
    pub fn has_own_cause(self) -> bool {
        self != Self::DependencyFailed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureCause {
    pub reason: String,
    pub log_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailureExplanation {
    Unexplained,
    Reasoned(String),
    Explained(FailureCause),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedUnit {
    pub unit: String,
    pub label: String,
    pub kind: FailureKind,
    pub explanation: FailureExplanation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Succeeded { at: Duration },
    Failed(FailureKind),
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    Untouched,
    Pending(PendingJob),
    Finished(Outcome),
}

#[derive(Clone, Debug)]
struct UnitRecord {
    facts: UnitFacts,
    activity: Activity,
    progress: Progress,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub done: usize,
    pub running: usize,
    pub waiting: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishedRun {
    pub label: String,
    pub started_at: Duration,
    pub finished_at: Duration,
    pub failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishedStage {
    pub label: &'static str,
    pub finished_units: usize,
    pub failed_units: usize,
    pub started_at: Duration,
    pub reached_at: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Root {
    Unresolved,
    Awaiting(String),
    Queued(String),
}

impl Root {
    #[must_use]
    pub fn unit(&self) -> Option<&str> {
        match self {
            Self::Unresolved => None,
            Self::Awaiting(unit) | Self::Queued(unit) => Some(unit),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BootState {
    pub phase: Phase,
    pub origin: Duration,
    pub root: Root,
    pub failed: Vec<FailedUnit>,
    pub finished_stages: Vec<FinishedStage>,
    pub finished_runs: Vec<FinishedRun>,
    pub handoff_unit: Option<String>,
    pub manager_exited_at: Option<Duration>,
    pub milestone_targets: Vec<String>,
    pub shutdown_successor: String,
    shell_units: Vec<String>,
    boot_finished: bool,
    units: HashMap<String, UnitRecord>,
    jobs: HashMap<u32, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerVocabulary {
    pub host_milestones: Vec<String>,
    pub shutdown_milestones: Vec<String>,
    pub handoff_unit: String,
    pub shell_units: Vec<String>,
    pub shutdown_successor: String,
}

impl ManagerVocabulary {
    #[must_use]
    pub fn milestones_for(&self, phase: Phase) -> Vec<String> {
        match phase {
            Phase::Initrd | Phase::Host => self.host_milestones.clone(),
            Phase::Shutdown => self.shutdown_milestones.clone(),
            Phase::User => Vec::new(),
        }
    }
}
