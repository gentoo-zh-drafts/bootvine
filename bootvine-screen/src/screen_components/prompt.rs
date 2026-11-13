use std::{
    fmt,
    path::{Path, PathBuf},
};

use tracing::info;
use zeroize::Zeroizing;

use crate::{
    boot_view::PasswordPrompt,
    screen_components::{
        passphrase_input,
        passphrase_input::KeyPress,
        password_request::{PasswordRequest, RequestHint},
    },
};

#[derive(Clone, PartialEq, Eq)]
pub struct Passphrase(Zeroizing<String>);

impl Passphrase {
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Passphrase {
    fn from(text: &str) -> Self {
        Self(Zeroizing::new(text.to_owned()))
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Passphrase(<hidden>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerState {
    Typing,
    Sent,
    Rejected,
}

pub struct AskedRequest {
    pub request: PasswordRequest,
    typed: Zeroizing<String>,
    answer: AnswerState,
}

pub enum ShownRequest {
    Asking(AskedRequest),
    Idle,
}

pub enum TypedKey {
    Submitted {
        request: PasswordRequest,
        passphrase: Passphrase,
    },
    Absorbed,
}

pub struct Prompt {
    pub shown: ShownRequest,
    waiting: Vec<PasswordRequest>,
}

impl Default for Prompt {
    fn default() -> Self {
        Self {
            shown: ShownRequest::Idle,
            waiting: Vec::new(),
        }
    }
}

impl AskedRequest {
    fn open(request: PasswordRequest) -> Self {
        info!(path = %request.file.display(), "asking for a password");
        Self {
            request,
            typed: passphrase_input::empty(),
            answer: AnswerState::Typing,
        }
    }
}

impl Prompt {
    #[must_use]
    pub fn asked(&self) -> Option<&AskedRequest> {
        match &self.shown {
            ShownRequest::Asking(asked) => Some(asked),
            ShownRequest::Idle => None,
        }
    }

    #[must_use]
    pub fn is_asking(&self) -> bool {
        matches!(self.shown, ShownRequest::Asking(_))
    }

    pub fn follow(&mut self, listed: Vec<PasswordRequest>) {
        self.waiting = listed;
        let still_listed = |file: &Path| self.waiting.iter().any(|request| request.file == file);
        if let ShownRequest::Asking(asked) = &self.shown
            && still_listed(&asked.request.file)
        {
            return;
        }
        self.open_next();
    }

    pub fn hide(&mut self, listed: Vec<PasswordRequest>) {
        self.waiting = listed;
        self.shown = ShownRequest::Idle;
    }

    pub fn abandon(&mut self, file: &Path) {
        self.waiting.retain(|request| request.file != file);
        if self.asked().is_some_and(|asked| asked.request.file == file) {
            self.open_next();
        }
    }

    fn open_next(&mut self) {
        self.shown = match self.waiting.first() {
            Some(request) => ShownRequest::Asking(AskedRequest::open(request.clone())),
            None => ShownRequest::Idle,
        };
    }

    pub fn type_key(&mut self, key: KeyPress) -> TypedKey {
        let ShownRequest::Asking(asked) = &mut self.shown else {
            return TypedKey::Absorbed;
        };
        if asked.answer == AnswerState::Sent {
            return TypedKey::Absorbed;
        }
        match passphrase_input::type_key(&mut asked.typed, key) {
            Some(submitted) => TypedKey::Submitted {
                request: asked.request.clone(),
                passphrase: Passphrase(submitted),
            },
            None => TypedKey::Absorbed,
        }
    }

    pub fn mark_answer(&mut self, file: &Path, answer: AnswerState) {
        if let ShownRequest::Asking(asked) = &mut self.shown
            && asked.request.file == file
        {
            asked.answer = answer;
        }
    }

    #[must_use]
    pub fn view(&self) -> Option<PasswordPrompt<'_>> {
        let asked = self.asked()?;
        let request = &asked.request;
        let input = if request.silent {
            String::new()
        } else if request.echo {
            asked.typed.to_string()
        } else {
            passphrase_input::masked(&asked.typed)
        };
        Some(PasswordPrompt {
            message: &request.message,
            hint: match &request.hint {
                RequestHint::Shown(hint) => Some(hint.as_str()),
                RequestHint::Hidden => None,
            },
            input,
            sent: asked.answer == AnswerState::Sent,
            rejected: asked.answer == AnswerState::Rejected,
        })
    }

    #[must_use]
    pub fn shown_file(&self) -> Option<PathBuf> {
        self.asked().map(|asked| asked.request.file.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_components::password_request::tests::cryptsetup_request;

    fn type_text(prompt: &mut Prompt, text: &str) {
        text.chars().for_each(|character| {
            let _ = prompt.type_key(KeyPress::Character(character));
        });
    }

    #[test]
    fn follows_the_oldest_request_and_keeps_it_while_listed() {
        let mut prompt = Prompt::default();
        prompt.follow(vec![
            cryptsetup_request("ask.a"),
            cryptsetup_request("ask.b"),
        ]);
        type_text(&mut prompt, "sec");
        prompt.follow(vec![
            cryptsetup_request("ask.a"),
            cryptsetup_request("ask.b"),
        ]);
        assert_eq!(prompt.shown_file(), Some(PathBuf::from("ask.a")));
        assert_eq!(
            prompt.view().map(|shown| shown.input),
            Some("***".to_owned())
        );
        prompt.follow(vec![cryptsetup_request("ask.b")]);
        assert_eq!(prompt.shown_file(), Some(PathBuf::from("ask.b")));
        prompt.follow(Vec::new());
        assert!(!prompt.is_asking());
    }

    #[test]
    fn hides_what_is_typed_and_submits_on_enter() {
        let mut prompt = Prompt::default();
        prompt.follow(vec![cryptsetup_request("ask.x")]);
        type_text(&mut prompt, "secret");
        let _ = prompt.type_key(KeyPress::Backspace);
        assert_eq!(
            prompt.view().map(|shown| shown.input),
            Some("*****".to_owned())
        );
        let TypedKey::Submitted {
            request,
            passphrase,
        } = prompt.type_key(KeyPress::Enter)
        else {
            panic!("enter submits the typed passphrase");
        };
        assert_eq!(request.file, PathBuf::from("ask.x"));
        assert_eq!(passphrase.expose(), "secre");
    }

    #[test]
    fn a_sent_answer_blocks_typing_and_a_rejected_one_allows_it_again() {
        let mut prompt = Prompt::default();
        prompt.follow(vec![cryptsetup_request("ask.x")]);
        prompt.mark_answer(Path::new("ask.x"), AnswerState::Sent);
        type_text(&mut prompt, "more");
        assert_eq!(
            prompt.view().map(|shown| (shown.input, shown.sent)),
            Some((String::new(), true))
        );
        prompt.mark_answer(Path::new("ask.x"), AnswerState::Rejected);
        type_text(&mut prompt, "again");
        assert_eq!(
            prompt.view().map(|shown| (shown.input, shown.rejected)),
            Some(("*****".to_owned(), true))
        );
    }

    #[test]
    fn an_abandoned_request_gives_way_to_the_next() {
        let mut prompt = Prompt::default();
        prompt.follow(vec![
            cryptsetup_request("ask.a"),
            cryptsetup_request("ask.b"),
        ]);
        prompt.abandon(Path::new("ask.a"));
        assert_eq!(prompt.shown_file(), Some(PathBuf::from("ask.b")));
        prompt.abandon(Path::new("ask.b"));
        assert!(!prompt.is_asking());
    }

    #[test]
    fn the_passphrase_stays_out_of_debug_output() {
        assert_eq!(
            format!("{:?}", Passphrase::from("secret")),
            "Passphrase(<hidden>)"
        );
    }
}
