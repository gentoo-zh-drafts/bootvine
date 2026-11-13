use std::{future::Future, io, path::Path};

use bootvine_screen::{
    boot_model::{ManagerVocabulary, Phase, RootRequest, Scope},
    screen_components::{
        handover::{DisplayManager, ShellStart, StatusRelease},
        password_request::{PromptAsker, PromptDeadline},
        prompt::Passphrase,
        screen_application::Message,
    },
};
use bootvine_systemd_link::{
    boot_feed::bus_inputs,
    link_error::Error,
    password_agent,
    password_agent::{password_request_inputs, send_answer, until_abandoned},
    switch_root::INITRD_RELEASE,
    systemd_bus::{Session, restore_status_after_watch},
    systemd_vocabulary::systemd_vocabulary,
};
use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::LocalBoxStream};

pub trait ServiceManager {
    fn vocabulary(&self) -> ManagerVocabulary;
    fn boot_in_progress(&self) -> bool;
    fn initrd_running(&self) -> bool;
    fn boot_changes(
        &self,
        phase: Phase,
        scope: Scope,
        root: RootRequest,
    ) -> LocalBoxStream<'static, Message>;
    fn shell_is_starting(&self) -> ShellStart;
    fn display_manager_running(&self) -> DisplayManager;
    fn passwords_requested(&self) -> bool;
    fn password_requests(&self) -> LocalBoxStream<'static, Message>;
    fn answer_password(&self, socket: &Path, passphrase: &Passphrase) -> io::Result<()>;
    fn until_password_abandoned(
        &self,
        deadline: PromptDeadline,
        asker: PromptAsker,
    ) -> LocalBoxFuture<'static, ()>;
    fn restore_configured_status(&self) -> Result<(), Error>;
    fn release_status_output_after(
        &self,
        watch: impl Future<Output = Result<StatusRelease, Error>>,
    ) -> impl Future<Output = Result<(), Error>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemdManager;

impl ServiceManager for SystemdManager {
    fn vocabulary(&self) -> ManagerVocabulary {
        systemd_vocabulary()
    }

    fn boot_in_progress(&self) -> bool {
        Session::open(Scope::System)
            .and_then(|session| session.boot_in_progress())
            .is_ok_and(|booting| booting)
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the systemd initrd announces itself with /etc/initrd-release"
    )]
    fn initrd_running(&self) -> bool {
        Path::new(INITRD_RELEASE).exists()
    }

    fn boot_changes(
        &self,
        phase: Phase,
        scope: Scope,
        root: RootRequest,
    ) -> LocalBoxStream<'static, Message> {
        bus_inputs(phase, scope, root).boxed_local()
    }

    fn shell_is_starting(&self) -> ShellStart {
        let starting = Session::open(Scope::System)
            .and_then(|session| session.shell_is_starting())
            .expect("the private socket of systemd answers while a shell is about to start");
        if starting {
            ShellStart::Starting
        } else {
            ShellStart::NotStarting
        }
    }

    fn display_manager_running(&self) -> DisplayManager {
        let running = Session::open(Scope::System)
            .and_then(|session| session.display_manager_running())
            .expect("the private socket of systemd answers while the boot runs");
        if running {
            DisplayManager::Running
        } else {
            DisplayManager::NotRunning
        }
    }

    fn passwords_requested(&self) -> bool {
        password_agent::requested()
    }

    fn password_requests(&self) -> LocalBoxStream<'static, Message> {
        password_request_inputs()
    }

    fn answer_password(&self, socket: &Path, passphrase: &Passphrase) -> io::Result<()> {
        send_answer(socket, passphrase)
    }

    fn until_password_abandoned(
        &self,
        deadline: PromptDeadline,
        asker: PromptAsker,
    ) -> LocalBoxFuture<'static, ()> {
        until_abandoned(deadline, asker).boxed_local()
    }

    fn restore_configured_status(&self) -> Result<(), Error> {
        Session::open(Scope::System)?.restore_configured_status()
    }

    fn release_status_output_after(
        &self,
        watch: impl Future<Output = Result<StatusRelease, Error>>,
    ) -> impl Future<Output = Result<(), Error>> {
        restore_status_after_watch(watch)
    }
}
