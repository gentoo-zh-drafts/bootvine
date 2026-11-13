use std::{
    io,
    str::Chars,
    time::{Duration, Instant},
};

use systemd::{
    id128::Id128,
    journal::{Journal, OpenOptions},
};
use tokio::task::spawn_blocking;
use tracing::warn;

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
const ESCAPE: char = '\u{1b}';

struct UnitMessages {
    oldest_first: Vec<String>,
    manager_reported: bool,
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

fn read_settled_messages(unit: &str, wanted: usize, deadline: Instant) -> io::Result<Vec<String>> {
    let mut journal = OpenOptions::default().open()?;
    match_unit_in_this_boot(&mut journal, unit)?;
    journal.fd()?;
    loop {
        let messages = scan_newest_messages(&mut journal, wanted)?;
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero());
        match remaining {
            Some(left) if !messages.manager_reported => {
                journal.wait(Some(left))?;
            }
            _ => return Ok(messages.oldest_first),
        }
    }
}

fn scan_newest_messages(journal: &mut Journal, wanted: usize) -> io::Result<UnitMessages> {
    journal.seek_tail()?;
    let mut newest_first = Vec::new();
    let mut manager_reported = false;
    let mut scanned = 0;
    while newest_first.len() < wanted
        && scanned < ENTRIES_SCANNED
        && journal.previous().is_ok_and(|moved| moved > 0)
    {
        scanned += 1;
        if is_manager_summary(journal).unwrap_or(true) {
            manager_reported = true;
            continue;
        }
        if let Ok(Some(message)) = entry_message(journal) {
            newest_first.push(message);
        }
    }
    newest_first.reverse();
    Ok(UnitMessages {
        oldest_first: newest_first,
        manager_reported,
    })
}

fn match_unit_in_this_boot(journal: &mut Journal, unit: &str) -> io::Result<()> {
    UNIT_FIELDS.iter().try_for_each(|field| {
        journal.match_add(field, unit)?.match_or()?;
        Ok::<_, io::Error>(())
    })?;
    journal
        .match_and()?
        .match_add("_BOOT_ID", Id128::from_boot()?.to_string())?;
    Ok(())
}

#[expect(
    clippy::disallowed_methods,
    reason = "journal fields may hold arbitrary bytes and are only shown on screen, so a \
              replacement character beats dropping the message"
)]
fn field_text(journal: &mut Journal, field: &str) -> io::Result<Option<String>> {
    Ok(journal.get_data(field)?.and_then(|entry_field| {
        entry_field
            .value()
            .map(|value| String::from_utf8_lossy(value).into_owned())
    }))
}

fn is_manager_summary(journal: &mut Journal) -> io::Result<bool> {
    Ok(field_text(journal, "MESSAGE_ID")?
        .is_some_and(|message_id| MANAGER_SUMMARY_MESSAGE_IDS.contains(&message_id.as_str())))
}

fn entry_message(journal: &mut Journal) -> io::Result<Option<String>> {
    Ok(field_text(journal, "MESSAGE")?
        .map(|message| printable_line(&message))
        .filter(|line| !line.is_empty()))
}

fn printable_line(message: &str) -> String {
    let mut characters = message.lines().next().unwrap_or_default().chars();
    let mut printable = String::new();
    while let Some(character) = characters.next() {
        if character == ESCAPE {
            skip_escape_sequence(&mut characters);
        } else if !character.is_control() {
            printable.push(character);
        }
    }
    printable.trim_end().to_owned()
}

fn skip_escape_sequence(characters: &mut Chars<'_>) {
    if characters.clone().next() == Some('[') {
        characters.next();
        characters.find(|character| ('@'..='~').contains(character));
    }
}

#[cfg(test)]
mod tests {
    use std::env;

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
    fn printable_line_keeps_the_first_line_without_colours() {
        assert_eq!(
            printable_line("\u{1b}[31mno such file\u{1b}[0m\nsecond line"),
            "no such file"
        );
    }
}
