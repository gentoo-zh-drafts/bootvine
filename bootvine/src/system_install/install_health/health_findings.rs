use std::fmt;

pub(super) enum HealthVerdict {
    Healthy,
    Advice(String),
    Unhealthy(String),
}

pub(super) struct HealthFinding {
    pub(super) verdict: HealthVerdict,
    pub(super) statement: String,
}

pub enum OverallHealth {
    Healthy,
    Unhealthy,
}

impl HealthFinding {
    pub(super) fn healthy(statement: String) -> Self {
        Self {
            verdict: HealthVerdict::Healthy,
            statement,
        }
    }

    pub(super) fn advice(statement: String, remedy: String) -> Self {
        Self {
            verdict: HealthVerdict::Advice(remedy),
            statement,
        }
    }

    pub(super) fn unhealthy(statement: String, remedy: String) -> Self {
        Self {
            verdict: HealthVerdict::Unhealthy(remedy),
            statement,
        }
    }
}

impl fmt::Display for HealthFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.verdict {
            HealthVerdict::Healthy => write!(formatter, "✓ {}", self.statement),
            HealthVerdict::Advice(remedy) => {
                write!(formatter, "• {}\n    → {remedy}", self.statement)
            }
            HealthVerdict::Unhealthy(remedy) => {
                write!(formatter, "✗ {}\n    → {remedy}", self.statement)
            }
        }
    }
}

impl From<&[HealthFinding]> for OverallHealth {
    fn from(findings: &[HealthFinding]) -> Self {
        if findings
            .iter()
            .any(|finding| matches!(finding.verdict, HealthVerdict::Unhealthy(_)))
        {
            Self::Unhealthy
        } else {
            Self::Healthy
        }
    }
}
