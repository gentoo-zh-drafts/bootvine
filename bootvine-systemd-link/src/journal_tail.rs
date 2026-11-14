use std::{
    fs, io,
    time::{Duration, Instant},
};

use sdjournal::{EntryRef, Journal, JournalConfig, LiveJournal, LiveSubscription, SdJournalError};
use tokio::task::spawn_blocking;
use tracing::warn;

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const UNIT_FIELDS: [&str; 4] = ["_SYSTEMD_UNIT", "UNIT", "_SYSTEMD_USER_UNIT", "USER_UNIT"];
const MANAGER_SUMMARY_MESSAGE_IDS: [&str; 10] = [
    "7d4958e842da4a758f6c1cdc7b36dcc5",
    "39f53479d3a045ac8e11786248231fbf",
    "7ad2d189f7e94e70a38c781354912448",
    "5eb03494b6584870a536b337290809b3",
    "be02cf6855d2428ba40df7e9d022f03d",
    "de5b426a63be47a7b6ac3eaac82e2f6f",
    "9d1aaa27d60140bd96365438aad20286",
    "ae8f7b866b0347b9af31fe1c80b127c0",
    "d9b373ed55a64feb8242e02dbe79a49c",
    "98e322203f7a4ed290d09fe03c09fe15",
];
const ENTRIES_SCANNED: usize = 200;
const JOURNAL_WAKE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
enum JournalReadError {
    #[error("reading the boot id: {0}")]
    BootId(#[from] io::Error),
    #[error("reading the journal: {0}")]
    Journal(#[from] SdJournalError),
}

struct UnitMessages {
    oldest_first: Vec<String>,
    manager_reported: bool,
}

enum JournalGrowth {
    Grew,
    DeadlinePassed,
}

pub(crate) async fn settled_unit_messages(
    unit: String,
    wanted: usize,
    patience: Duration,
) -> Vec<String> {
    spawn_blocking(move || {
        read_settled_messages(&unit, wanted, Instant::now() + patience).unwrap_or_else(|error| {
            warn!(unit = %unit, error = %error, "could not read the journal of the unit");
            Vec::new()
        })
    })
    .await
    .expect("reading the journal does not panic")
}

fn read_settled_messages(
    unit: &str,
    wanted: usize,
    deadline: Instant,
) -> Result<Vec<String>, JournalReadError> {
    let boot_id = fs::read_to_string(BOOT_ID_PATH)?.trim().replace('-', "");
    let mut live_journal = woken_journal()?;
    let mut unit_filter = live_journal.filter();
    UNIT_FIELDS.iter().for_each(|field| {
        unit_filter.or_group(|group| {
            group.match_exact(field, unit.as_bytes());
            group.match_exact("_BOOT_ID", boot_id.as_bytes());
        });
    });
    let new_unit_entries = live_journal.subscribe(unit_filter)?;
    loop {
        let messages = scan_newest_messages(&Journal::open_default()?, unit, &boot_id, wanted)?;
        if messages.manager_reported {
            return Ok(messages.oldest_first);
        }
        match wait_for_new_entries(&mut live_journal, &new_unit_entries, deadline)? {
            JournalGrowth::Grew => {}
            JournalGrowth::DeadlinePassed => return Ok(messages.oldest_first),
        }
    }
}

fn woken_journal() -> Result<LiveJournal, SdJournalError> {
    Journal::open_default_with_config(JournalConfig {
        poll_interval: JOURNAL_WAKE_INTERVAL,
        ..JournalConfig::default()
    })?
    .live()
}

fn wait_for_new_entries(
    live_journal: &mut LiveJournal,
    new_entries: &LiveSubscription,
    deadline: Instant,
) -> Result<JournalGrowth, SdJournalError> {
    while Instant::now() < deadline {
        if live_journal.poll_once()? > 0 {
            while new_entries.try_recv().is_ok() {}
            return Ok(JournalGrowth::Grew);
        }
    }
    Ok(JournalGrowth::DeadlinePassed)
}

fn scan_newest_messages(
    journal: &Journal,
    unit: &str,
    boot_id: &str,
    wanted: usize,
) -> Result<UnitMessages, SdJournalError> {
    let mut query = journal.query();
    UNIT_FIELDS.iter().for_each(|field| {
        query.or_group(|group| {
            group.match_exact(field, unit.as_bytes());
            group.match_exact("_BOOT_ID", boot_id.as_bytes());
        });
    });
    query.seek_tail().limit(ENTRIES_SCANNED);
    let mut newest_first = Vec::new();
    let mut manager_reported = false;
    for entry in query.iter()? {
        if newest_first.len() >= wanted {
            break;
        }
        let entry = entry?;
        if is_manager_summary(&entry) {
            manager_reported = true;
        } else if let Some(message) = entry_message(&entry) {
            newest_first.push(message);
        }
    }
    newest_first.reverse();
    Ok(UnitMessages {
        oldest_first: newest_first,
        manager_reported,
    })
}

fn is_manager_summary(entry: &EntryRef) -> bool {
    entry.get("MESSAGE_ID").is_some_and(|message_id| {
        MANAGER_SUMMARY_MESSAGE_IDS
            .iter()
            .any(|summary_id| summary_id.as_bytes() == message_id)
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "journal fields may hold arbitrary bytes and are only shown on screen, so a \
              replacement character beats dropping the message"
)]
fn entry_message(entry: &EntryRef) -> Option<String> {
    entry
        .get("MESSAGE")
        .map(|message| printable_line(&String::from_utf8_lossy(message)))
        .filter(|line| !line.is_empty())
}

fn printable_line(message: &str) -> String {
    strip_ansi_escapes::strip_str(message.lines().next().unwrap_or_default())
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::{env, process::Command};

    use super::*;

    #[test]
    #[ignore = "reads this machine's journal: cargo test journal_of_this_boot -- --ignored \
                --nocapture"]
    fn journal_of_this_boot_has_lines_for_journald() {
        let unit = env::var("BOOTVINE_JOURNAL_UNIT")
            .unwrap_or_else(|_| "systemd-journald.service".to_owned());
        let lines = read_settled_messages(&unit, 6, Instant::now()).expect("readable journal");
        lines.iter().for_each(|line| println!("{line}"));
        assert!(!lines.is_empty());
    }

    #[test]
    #[ignore = "writes to this machine's journal: cargo test journal_write_wakes -- --ignored"]
    fn journal_write_wakes_the_wait() {
        let mut live_journal = woken_journal().expect("the journal opens");
        let mut test_filter = live_journal.filter();
        test_filter.match_exact("SYSLOG_IDENTIFIER", b"bootvine-test");
        let test_entries = live_journal
            .subscribe(test_filter)
            .expect("the subscription registers");
        Command::new("systemd-cat")
            .args(["-t", "bootvine-test", "echo", "wake the wait"])
            .status()
            .expect("systemd-cat runs");
        let growth = wait_for_new_entries(
            &mut live_journal,
            &test_entries,
            Instant::now() + Duration::from_secs(5),
        )
        .expect("the wait works");
        assert!(matches!(growth, JournalGrowth::Grew));
    }

    #[test]
    fn printable_line_drops_hyperlinks_but_keeps_their_text() {
        assert_eq!(
            printable_line(
                "see \u{1b}]8;;https://systemd.io\u{7}the docs\u{1b}]8;;\u{7}\tfor more"
            ),
            "see the docsfor more"
        );
    }

    #[test]
    fn printable_line_keeps_the_first_line_without_colours() {
        assert_eq!(
            printable_line("\u{1b}[31mno such file\u{1b}[0m\nsecond line"),
            "no such file"
        );
    }
}
