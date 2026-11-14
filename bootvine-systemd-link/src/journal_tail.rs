use std::{
    fs, io,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use journal::{DirectoryReader, SdkError};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::inotify::{self, CreateFlags, WatchFlags},
};
use tokio::task::spawn_blocking;
use tracing::warn;

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const JOURNAL_ROOTS: [&str; 2] = ["/run/log/journal", "/var/log/journal"];
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

#[derive(Debug, thiserror::Error)]
enum JournalReadError {
    #[error("{0}")]
    Filesystem(#[from] io::Error),
    #[error("reading the journal files: {0}")]
    JournalFiles(#[from] SdkError),
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
    loop {
        let directories = journal_directories()?;
        let journal_changes = watch_journal_directories(&directories)?;
        let messages = scan_newest_messages(&directories, unit, &boot_id, wanted)?;
        if messages.manager_reported {
            return Ok(messages.oldest_first);
        }
        match wait_for_journal_change(&journal_changes, deadline)? {
            JournalGrowth::Grew => {}
            JournalGrowth::DeadlinePassed => return Ok(messages.oldest_first),
        }
    }
}

fn directory_entries(directory: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(directory) {
        Ok(entries) => entries.map(|entry| Ok(entry?.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn journal_directories() -> io::Result<Vec<PathBuf>> {
    let machine_directories = JOURNAL_ROOTS
        .iter()
        .map(|root| directory_entries(Path::new(root)))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(JOURNAL_ROOTS
        .iter()
        .map(PathBuf::from)
        .chain(machine_directories.into_iter().flatten())
        .filter(|directory| directory.is_dir())
        .collect())
}

fn is_journal_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".journal") || name.ends_with(".journal~"))
}

fn watch_journal_directories(directories: &[PathBuf]) -> io::Result<OwnedFd> {
    let journal_changes = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    directories.iter().try_for_each(|directory| {
        inotify::add_watch(
            &journal_changes,
            directory.as_path(),
            WatchFlags::MODIFY | WatchFlags::CREATE | WatchFlags::MOVED_TO,
        )
        .map(drop)
    })?;
    Ok(journal_changes)
}

fn wait_for_journal_change(
    journal_changes: &OwnedFd,
    deadline: Instant,
) -> io::Result<JournalGrowth> {
    let Some(remaining) = deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
    else {
        return Ok(JournalGrowth::DeadlinePassed);
    };
    let timeout = Timespec::try_from(remaining).map_err(io::Error::other)?;
    let mut watched = [PollFd::new(journal_changes, PollFlags::IN)];
    Ok(match poll(&mut watched, Some(&timeout))? {
        0 => JournalGrowth::DeadlinePassed,
        _ => JournalGrowth::Grew,
    })
}

fn scan_newest_messages(
    directories: &[PathBuf],
    unit: &str,
    boot_id: &str,
    wanted: usize,
) -> Result<UnitMessages, JournalReadError> {
    let journal_files = directories
        .iter()
        .map(|directory| directory_entries(directory))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|path| is_journal_file(path))
        .collect::<Vec<_>>();
    let mut reader = DirectoryReader::open_files(&journal_files)?;
    UNIT_FIELDS.iter().try_for_each(|field| {
        reader.add_match(format!("{field}={unit}").as_bytes());
        reader.add_disjunction()
    })?;
    reader.add_conjunction()?;
    reader.add_match(format!("_BOOT_ID={boot_id}").as_bytes());
    reader.seek_tail();
    let mut newest_first = Vec::new();
    let mut manager_reported = false;
    let mut scanned = 0;
    while newest_first.len() < wanted && scanned < ENTRIES_SCANNED && reader.previous()? {
        scanned += 1;
        if is_manager_summary(&mut reader)? {
            manager_reported = true;
        } else if let Some(message) = entry_message(&mut reader)? {
            newest_first.push(message);
        }
    }
    newest_first.reverse();
    Ok(UnitMessages {
        oldest_first: newest_first,
        manager_reported,
    })
}

fn field_value(reader: &mut DirectoryReader, field: &str) -> Result<Option<Vec<u8>>, SdkError> {
    Ok(reader
        .get_entry_payload(field.as_bytes())?
        .map(|payload| payload.get(field.len() + 1..).unwrap_or_default().to_vec()))
}

fn is_manager_summary(reader: &mut DirectoryReader) -> Result<bool, SdkError> {
    Ok(
        field_value(reader, "MESSAGE_ID")?.is_some_and(|message_id| {
            MANAGER_SUMMARY_MESSAGE_IDS
                .iter()
                .any(|summary_id| summary_id.as_bytes() == message_id)
        }),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "journal fields may hold arbitrary bytes and are only shown on screen, so a \
              replacement character beats dropping the message"
)]
fn entry_message(reader: &mut DirectoryReader) -> Result<Option<String>, SdkError> {
    Ok(field_value(reader, "MESSAGE")?
        .map(|message| printable_line(&String::from_utf8_lossy(&message)))
        .filter(|line| !line.is_empty()))
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
        let journal_changes =
            watch_journal_directories(&journal_directories().expect("journal directories list"))
                .expect("journal directories are watched");
        let started = Instant::now();
        Command::new("systemd-cat")
            .args(["-t", "bootvine-test", "echo", "wake the wait"])
            .status()
            .expect("systemd-cat runs");
        let growth = wait_for_journal_change(&journal_changes, started + Duration::from_secs(5))
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
