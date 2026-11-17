use std::str;

use bootvine_screen::boot_model::Phase;
use sdjournal::{EntryRef, Journal};

use crate::journal_tail::{JournalReadError, current_boot_id};

pub const STARTING_MESSAGE: &str = "starting bootvine";
pub(crate) const WATCHING_MESSAGE: &str = "watching the boot";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoggedVersion {
    Logged(String),
    Unlogged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootvineRun {
    pub phase: Phase,
    pub version: LoggedVersion,
}

enum RunRecord {
    Started { process_id: String, version: String },
    Watching { process_id: String, phase: Phase },
}

fn field_text(entry: &EntryRef, field: &str) -> String {
    str::from_utf8(
        entry
            .get(field)
            .unwrap_or_else(|| panic!("bootvine logs {field} with every run record")),
    )
    .expect("bootvine logs its run fields as ASCII")
    .to_owned()
}

impl From<&EntryRef> for RunRecord {
    fn from(entry: &EntryRef) -> Self {
        let process_id = field_text(entry, "_PID");
        if entry.get("MESSAGE") == Some(STARTING_MESSAGE.as_bytes()) {
            Self::Started {
                process_id,
                version: field_text(entry, "VERSION"),
            }
        } else {
            Self::Watching {
                process_id,
                phase: field_text(entry, "PHASE")
                    .parse()
                    .expect("bootvine logs one of its phase labels"),
            }
        }
    }
}

fn version_started_by(run_records: &[RunRecord], watching_process: &str) -> LoggedVersion {
    run_records
        .iter()
        .find_map(|run_record| match run_record {
            RunRecord::Started {
                process_id,
                version,
            } if process_id == watching_process => Some(LoggedVersion::Logged(version.clone())),
            RunRecord::Started { .. } | RunRecord::Watching { .. } => None,
        })
        .unwrap_or(LoggedVersion::Unlogged)
}

fn runs_from_records(run_records: &[RunRecord]) -> Vec<BootvineRun> {
    run_records
        .iter()
        .filter_map(|run_record| match run_record {
            RunRecord::Watching { process_id, phase } => Some(BootvineRun {
                phase: *phase,
                version: version_started_by(run_records, process_id),
            }),
            RunRecord::Started { .. } => None,
        })
        .collect()
}

pub fn this_boot_bootvine_runs() -> Result<Vec<BootvineRun>, JournalReadError> {
    let boot_id = current_boot_id()?;
    let journal = Journal::open_default()?;
    let mut query = journal.query();
    [STARTING_MESSAGE, WATCHING_MESSAGE]
        .iter()
        .for_each(|message| {
            query.or_group(|group| {
                group.match_exact("MESSAGE", message.as_bytes());
                group.match_exact("_BOOT_ID", boot_id.as_bytes());
            });
        });
    let run_records = query
        .iter()?
        .map(|entry| entry.map(|entry| RunRecord::from(&entry)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(runs_from_records(&run_records))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(process_id: &str, version: &str) -> RunRecord {
        RunRecord::Started {
            process_id: process_id.to_owned(),
            version: version.to_owned(),
        }
    }

    fn watching(process_id: &str, phase: Phase) -> RunRecord {
        RunRecord::Watching {
            process_id: process_id.to_owned(),
            phase,
        }
    }

    #[test]
    fn runs_take_the_version_logged_by_the_same_process() {
        let runs = runs_from_records(&[
            started("70", "0.2.0"),
            watching("70", Phase::Initrd),
            watching("70", Phase::Host),
            watching("412", Phase::Shutdown),
        ]);
        assert_eq!(
            runs,
            vec![
                BootvineRun {
                    phase: Phase::Initrd,
                    version: LoggedVersion::Logged("0.2.0".to_owned()),
                },
                BootvineRun {
                    phase: Phase::Host,
                    version: LoggedVersion::Logged("0.2.0".to_owned()),
                },
                BootvineRun {
                    phase: Phase::Shutdown,
                    version: LoggedVersion::Unlogged,
                },
            ]
        );
    }
}
