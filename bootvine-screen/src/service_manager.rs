use std::{error::Error, future::Future, io};

use futures_core::Stream;

use crate::{
    boot_model::{
        BootChange, BootProgress, ConsolePrompt, ManagerVocabulary, Phase, RootRequest, RunRequest,
        Scope,
    },
    screen_components::{
        handover::{DisplayManager, ShellStart, StatusRelease, WatchFailure},
        password_request::{PasswordAnswer, PasswordRequest, PromptAsker, PromptDeadline},
    },
};

pub trait ServiceManager: Clone + 'static {
    type Failure: Error + 'static;

    fn vocabulary(&self) -> ManagerVocabulary;

    fn starting_phase(&self, scope: Scope, request: RunRequest) -> Phase;

    fn boot_progress(&self) -> impl Future<Output = BootProgress>;

    fn boot_changes(
        &self,
        phase: Phase,
        scope: Scope,
        root: RootRequest,
    ) -> impl Stream<Item = Result<Vec<BootChange>, WatchFailure>> + 'static;

    fn password_requests(&self) -> impl Stream<Item = Vec<PasswordRequest>> + 'static;

    fn password_abandoned(
        &self,
        deadline: PromptDeadline,
        asker: PromptAsker,
    ) -> impl Future<Output = ()> + 'static;

    fn console_prompt(&self) -> ConsolePrompt;

    fn console_prompts(&self) -> impl Stream<Item = ConsolePrompt> + 'static;

    fn answer_password(&self, answer: PasswordAnswer) -> impl Future<Output = io::Result<()>>;

    fn shell_start(&self) -> impl Future<Output = ShellStart>;

    fn display_manager(&self) -> impl Future<Output = DisplayManager>;

    fn restore_status_output(&self) -> Result<(), Self::Failure>;

    fn quiet_status_output_during<Outer: From<Self::Failure>>(
        &self,
        watch: impl Future<Output = Result<StatusRelease, Outer>>,
    ) -> impl Future<Output = Result<(), Outer>>;
}
