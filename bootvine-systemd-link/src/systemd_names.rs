use bootvine_screen::boot_model::{
    ActiveState, JobKind, JobResult, UnitFacts, UnitKind, UnitTimeouts,
};

#[must_use]
pub fn active_state_from(text: &str) -> ActiveState {
    match text {
        "activating" => ActiveState::Activating,
        "active" => ActiveState::Active,
        "deactivating" => ActiveState::Deactivating,
        "reloading" => ActiveState::Reloading,
        "failed" => ActiveState::Failed,
        _ => ActiveState::Inactive,
    }
}

#[must_use]
pub fn job_result_from(text: &str) -> JobResult {
    match text {
        "done" => JobResult::Done,
        "skipped" => JobResult::Skipped,
        "canceled" => JobResult::Canceled,
        "collected" => JobResult::Collected,
        "timeout" => JobResult::Timeout,
        "dependency" => JobResult::Dependency,
        _ => JobResult::Failed,
    }
}

#[must_use]
pub fn job_kind_from(text: &str) -> JobKind {
    match text {
        "stop" => JobKind::TakesDown,
        _ => JobKind::BringsUp,
    }
}

#[must_use]
pub fn unit_kind_of(name: &str) -> UnitKind {
    match name.rsplit_once('.').map(|(_, suffix)| suffix) {
        Some("service") => UnitKind::Service,
        Some("target") => UnitKind::Target,
        Some("socket") => UnitKind::Socket,
        Some("mount") => UnitKind::Mount,
        Some("automount") => UnitKind::Automount,
        Some("device") => UnitKind::Device,
        Some("swap") => UnitKind::Swap,
        Some("path") => UnitKind::Path,
        Some("timer") => UnitKind::Timer,
        Some("slice") => UnitKind::Slice,
        Some("scope") => UnitKind::Scope,
        _ => UnitKind::Other,
    }
}

#[must_use]
pub fn short_unit_name(name: &str) -> String {
    match unit_kind_of(name) {
        UnitKind::Target | UnitKind::Other => name.to_owned(),
        _ => name
            .rsplit_once('.')
            .map_or_else(|| name.to_owned(), |(stem, _)| stem.to_owned()),
    }
}

#[must_use]
pub fn systemd_unit_facts(
    name: String,
    description: Option<String>,
    after: Vec<String>,
    before: Vec<String>,
    timeouts: UnitTimeouts,
) -> UnitFacts {
    UnitFacts {
        short_name: short_unit_name(&name),
        kind: unit_kind_of(&name),
        name,
        description,
        after,
        before,
        timeouts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_systemd_strings_fall_back_instead_of_failing() {
        assert_eq!(active_state_from("deactivating"), ActiveState::Deactivating);
        assert_eq!(active_state_from("maintenance"), ActiveState::Inactive);
        assert_eq!(job_result_from("dependency"), JobResult::Dependency);
        assert_eq!(job_result_from("collected"), JobResult::Collected);
        assert_eq!(job_result_from("frozen"), JobResult::Failed);
        assert_eq!(job_kind_from("stop"), JobKind::TakesDown);
        assert_eq!(job_kind_from("restart"), JobKind::BringsUp);
        assert_eq!(unit_kind_of("dev-sda.swap"), UnitKind::Swap);
        assert_eq!(unit_kind_of("init.scope.d"), UnitKind::Other);
        assert_eq!(unit_kind_of("no-suffix"), UnitKind::Other);
    }

    #[test]
    fn kinds_with_a_tag_drop_their_suffix() {
        assert_eq!(unit_kind_of("docker.socket"), UnitKind::Socket);
        assert_eq!(short_unit_name("docker.socket"), "docker");
        assert_eq!(short_unit_name("sshd.service"), "sshd");
        assert_eq!(short_unit_name("basic.target"), "basic.target");
    }
}
