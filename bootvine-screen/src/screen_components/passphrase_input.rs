use std::mem::take;

use zeroize::Zeroizing;

const ROOM_FOR_TYPING: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPress {
    Character(char),
    Enter,
    Backspace,
    Escape,
    ClearLine,
    Interrupt,
    Other,
}

#[must_use]
pub fn empty() -> Zeroizing<String> {
    Zeroizing::new(String::with_capacity(ROOM_FOR_TYPING))
}

#[must_use]
pub fn masked(typed: &str) -> String {
    "*".repeat(typed.chars().count())
}

#[must_use]
pub fn asks_to_quit(key: KeyPress, typing: bool) -> bool {
    match key {
        KeyPress::Escape => true,
        KeyPress::Character('q') | KeyPress::Interrupt => !typing,
        KeyPress::Character(_)
        | KeyPress::Enter
        | KeyPress::Backspace
        | KeyPress::ClearLine
        | KeyPress::Other => false,
    }
}

pub fn type_key(typed: &mut Zeroizing<String>, key: KeyPress) -> Option<Zeroizing<String>> {
    match key {
        KeyPress::Enter => return Some(Zeroizing::new(take(&mut **typed))),
        KeyPress::Backspace => {
            typed.pop();
        }
        KeyPress::ClearLine => typed.clear(),
        KeyPress::Character(character) if typed.len() + character.len_utf8() <= ROOM_FOR_TYPING => {
            typed.push(character);
        }
        KeyPress::Character(_) | KeyPress::Escape | KeyPress::Interrupt | KeyPress::Other => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Zeroizing<String> {
        let mut typed = empty();
        text.chars().for_each(|character| {
            assert_eq!(type_key(&mut typed, KeyPress::Character(character)), None);
        });
        typed
    }

    #[test]
    fn hides_what_is_typed() {
        let mut typed = typed("secret");
        type_key(&mut typed, KeyPress::Backspace);
        assert_eq!(masked(&typed), "*****");
    }

    #[test]
    fn clearing_the_line_empties_it() {
        let mut typed = typed("secret");
        type_key(&mut typed, KeyPress::ClearLine);
        assert_eq!(typed.as_str(), "");
    }

    #[test]
    fn enter_hands_over_what_was_typed() {
        let mut typed = typed("secret");
        let submitted = type_key(&mut typed, KeyPress::Enter);
        assert_eq!(submitted.as_deref().map(String::as_str), Some("secret"));
        assert_eq!(typed.as_str(), "");
    }

    #[test]
    fn quit_keys_count_only_outside_a_prompt_except_escape() {
        assert!(asks_to_quit(KeyPress::Escape, true));
        assert!(asks_to_quit(KeyPress::Character('q'), false));
        assert!(!asks_to_quit(KeyPress::Character('q'), true));
        assert!(asks_to_quit(KeyPress::Interrupt, false));
        assert!(!asks_to_quit(KeyPress::Interrupt, true));
    }
}
