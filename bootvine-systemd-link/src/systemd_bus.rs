use std::{
    collections::HashMap,
    future::{Future, pending, ready},
    io, mem,
    num::NonZeroUsize,
    pin::pin,
    thread,
    time::Duration,
};

use bootvine_screen::{
    boot_model::{
        Activity, BootProgress, JobKind, JobResult, Scope, UnitFacts, UnitKind, UnitTimeouts,
    },
    screen_components::handover::StatusRelease,
};
use futures::{
    FutureExt, Stream, StreamExt,
    future::{Either, select},
    stream,
};
use serde::Serialize;
use tokio::{
    net::UnixStream,
    runtime::Builder as RuntimeBuilder,
    signal::unix::{Signal as OsSignal, SignalKind, signal},
    time::timeout,
};
use tracing::{error, warn};
use zbus::{
    Connection, DBusError, DynamicType, MatchRule, Message, MessageStream, OwnedObjectPath,
    OwnedVariant, Variant, connection::Builder as ConnectionBuilder, message::Type as MessageType,
};

use crate::{
    descriptor_streams::{CarriedItem, unfold_carrying},
    link_error::{BusRefusal, LinkError, LinkFailure},
    private_socket::SystemdPrivateSocket,
    systemd_names::{
        active_state_from, job_kind_from, job_result_from, systemd_unit_facts, unit_kind_of,
    },
};

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER_INTERFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_INTERFACE: &str = "org.freedesktop.systemd1.Unit";
const SERVICE_INTERFACE: &str = "org.freedesktop.systemd1.Service";
const MOUNT_INTERFACE: &str = "org.freedesktop.systemd1.Mount";
const SWAP_INTERFACE: &str = "org.freedesktop.systemd1.Swap";
const SOCKET_INTERFACE: &str = "org.freedesktop.systemd1.Socket";
const JOB_INTERFACE: &str = "org.freedesktop.systemd1.Job";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const PRIVATE_SOCKET: &str = "/run/systemd/private";
const SIGNAL_QUEUE_CAPACITY: NonZeroUsize = NonZeroUsize::new(4096).expect("4096 is not zero");
const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const UNLIMITED: Duration = Duration::from_micros(u64::MAX);
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";
const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";

type PropertyMap = HashMap<String, OwnedVariant>;

pub(crate) enum SignalBatch {
    Open(Vec<Signal>),
    Closed(Vec<Signal>),
}

pub(crate) enum Signal {
    JobNew {
        id: u32,
        job: OwnedObjectPath,
        unit: String,
    },
    JobRemoved {
        id: u32,
        unit: String,
        result: JobResult,
    },
    UnitChanged {
        path: OwnedObjectPath,
        activity: Activity,
    },
    BootFinished,
}

pub(crate) struct FetchedUnit {
    pub(crate) path: OwnedObjectPath,
    pub(crate) facts: UnitFacts,
    pub(crate) activity: Activity,
}

pub(crate) enum UnitLookup {
    Loaded(Box<FetchedUnit>),
    NotLoaded,
}

enum BusMessage {
    Signal(Signal),
    Unrelated,
}

pub(crate) struct JobListing {
    pub(crate) id: u32,
    pub(crate) unit: String,
    pub(crate) kind: JobKind,
}

type ListedJob = (
    u32,
    String,
    String,
    String,
    OwnedObjectPath,
    OwnedObjectPath,
);

impl From<ListedJob> for JobListing {
    fn from((id, unit, kind, ..): ListedJob) -> Self {
        Self {
            id,
            unit,
            kind: job_kind_from(&kind),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum StatusOutput {
    Silent,
    Configured,
}

impl StatusOutput {
    fn show_status_mode(self) -> &'static str {
        match self {
            Self::Silent => "no",
            Self::Configured => "",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BusTransport {
    PrivateSocket,
    MessageBus,
}

impl BusTransport {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::PrivateSocket => "the private socket",
            Self::MessageBus => "the bus",
        }
    }
}

enum SignalStreamState {
    Open,
    Ended,
}

enum CallProgress<Answer> {
    Answered(Answer),
    SignalArrived(Option<zbus::Result<Message>>),
}

async fn boot_finish_or_lost_connection() -> Result<(), LinkFailure> {
    let mut connection = BusConnection::open(Scope::System).await?;
    connection.subscribe().await?;
    if connection.boot_progress().await? == BootProgress::InProgress {
        connection
            .into_signals()
            .any(|signal| ready(matches!(signal, Signal::BootFinished)))
            .await;
    }
    Ok(())
}

async fn termination_request(listener: io::Result<OsSignal>) {
    match listener {
        Ok(mut listener) => {
            listener.recv().await;
        }
        Err(_) => pending().await,
    }
}

pub(crate) async fn restore_status_after_watch<Failure: From<LinkError>>(
    watch: impl Future<Output = Result<StatusRelease, Failure>>,
) -> Result<(), Failure> {
    let silenced_status = SilencedStatus;
    let terminated = termination_request(signal(SignalKind::terminate()));
    let watched = watch.await;
    if !matches!(watched, Ok(StatusRelease::Immediately)) {
        select(pin!(terminated), pin!(boot_finish_or_lost_connection())).await;
    }
    silenced_status.restore()?;
    watched.map(drop)
}

struct SilencedStatus;

impl SilencedStatus {
    fn restore(self) -> Result<(), LinkError> {
        mem::forget(self);
        Ok(change_status_output_now(StatusOutput::Configured)?)
    }
}

impl Drop for SilencedStatus {
    fn drop(&mut self) {
        if let Err(error) = change_status_output_now(StatusOutput::Configured) {
            error!(error = %error, "could not restore the status output of systemd");
        }
    }
}

pub(crate) fn change_status_output_now(output: StatusOutput) -> Result<(), LinkFailure> {
    thread::spawn(move || {
        RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(LinkFailure::CallRuntime)?
            .block_on(async {
                BusConnection::open(Scope::System)
                    .await?
                    .change_status_output(output)
                    .await
            })
    })
    .join()
    .expect("changing the status output of systemd does not panic")
}

async fn private_socket_connection() -> Result<Connection, LinkFailure> {
    let stream = UnixStream::connect(PRIVATE_SOCKET).await?;
    Ok(ConnectionBuilder::socket(SystemdPrivateSocket(stream))
        .p2p()
        .build()
        .await?)
}

pub(crate) struct BusConnection {
    connection: Connection,
    signal_stream: MessageStream,
    signal_stream_state: SignalStreamState,
    transport: BusTransport,
    early_signals: Vec<Signal>,
}

impl BusConnection {
    pub(crate) async fn open(scope: Scope) -> Result<Self, LinkFailure> {
        let (connection, transport) = match scope {
            Scope::System => match private_socket_connection().await {
                Ok(connection) => (connection, BusTransport::PrivateSocket),
                Err(_) => (
                    ConnectionBuilder::system().build().await?,
                    BusTransport::MessageBus,
                ),
            },
            Scope::User => (
                ConnectionBuilder::session().build().await?,
                BusTransport::MessageBus,
            ),
        };
        let signal_rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path_namespace(MANAGER_PATH)
            .build()?;
        let signal_stream =
            MessageStream::for_match_rule(signal_rule, &connection, Some(SIGNAL_QUEUE_CAPACITY))
                .await?;
        Ok(Self {
            connection,
            signal_stream,
            signal_stream_state: SignalStreamState::Open,
            transport,
            early_signals: Vec::new(),
        })
    }

    #[must_use]
    pub(crate) fn transport(&self) -> BusTransport {
        self.transport
    }

    fn keep_arrived_signal(&mut self, arrived: Option<zbus::Result<Message>>) {
        match arrived {
            Some(Ok(message)) => {
                if let BusMessage::Signal(signal) = parse_signal(&message) {
                    self.early_signals.push(signal);
                }
            }
            Some(Err(error)) => {
                warn!(error = %error, "skipped an unreadable message from systemd");
            }
            None => self.signal_stream_state = SignalStreamState::Ended,
        }
    }

    async fn keeping_signals_until<Answer>(
        &mut self,
        answer: impl Future<Output = Answer>,
    ) -> Answer {
        let mut answer = pin!(answer);
        loop {
            if let SignalStreamState::Ended = self.signal_stream_state {
                return answer.await;
            }
            let progress = match select(answer.as_mut(), self.signal_stream.next()).await {
                Either::Left((answered, _)) => CallProgress::Answered(answered),
                Either::Right((arrived, _)) => CallProgress::SignalArrived(arrived),
            };
            match progress {
                CallProgress::Answered(answered) => return answered,
                CallProgress::SignalArrived(arrived) => self.keep_arrived_signal(arrived),
            }
        }
    }

    async fn call<Arguments: Serialize + DynamicType>(
        &mut self,
        path: &str,
        interface: &str,
        member: &str,
        arguments: &Arguments,
    ) -> Result<Message, LinkFailure> {
        let connection = self.connection.clone();
        let reply =
            connection.call_method(Some(DESTINATION), path, Some(interface), member, arguments);
        let Ok(answered) = timeout(CALL_TIMEOUT, self.keeping_signals_until(reply)).await else {
            return Err(LinkFailure::CallTimedOut);
        };
        answered.map_err(|call_error| match call_error {
            zbus::Error::MethodError(name, description, _) => LinkFailure::Refused(BusRefusal {
                name: name.to_string(),
                description: description.unwrap_or_default(),
            }),
            zbus::Error::FDO(standard_error) => LinkFailure::Refused(BusRefusal {
                name: standard_error.name().to_string(),
                description: standard_error.description().unwrap_or_default().to_owned(),
            }),
            other => LinkFailure::Bus(other),
        })
    }

    async fn manager_call<Arguments: Serialize + DynamicType>(
        &mut self,
        member: &str,
        arguments: &Arguments,
    ) -> Result<Message, LinkFailure> {
        self.call(MANAGER_PATH, MANAGER_INTERFACE, member, arguments)
            .await
    }

    async fn property(
        &mut self,
        path: &str,
        interface: &str,
        property: &str,
    ) -> Result<OwnedVariant, LinkFailure> {
        let reply = self
            .call(path, PROPERTIES_INTERFACE, "Get", &(interface, property))
            .await?;
        Ok(reply.body().deserialize()?)
    }

    pub(crate) async fn subscribe(&mut self) -> Result<(), LinkFailure> {
        self.manager_call("Subscribe", &()).await?;
        Ok(())
    }

    pub(crate) async fn change_status_output(
        &mut self,
        output: StatusOutput,
    ) -> Result<(), LinkFailure> {
        self.manager_call("SetShowStatus", &(output.show_status_mode(),))
            .await?;
        Ok(())
    }

    pub(crate) async fn default_target(&mut self) -> Result<String, LinkFailure> {
        let reply = self.manager_call("GetDefaultTarget", &()).await?;
        Ok(reply.body().deserialize()?)
    }

    pub(crate) async fn boot_progress(&mut self) -> Result<BootProgress, LinkFailure> {
        let finished = self
            .property(MANAGER_PATH, MANAGER_INTERFACE, "FinishTimestampMonotonic")
            .await?;
        Ok(match u64::try_from(finished)? {
            0 => BootProgress::InProgress,
            _ => BootProgress::Finished,
        })
    }

    pub(crate) async fn list_jobs(&mut self) -> Result<Vec<JobListing>, LinkFailure> {
        let reply = self.manager_call("ListJobs", &()).await?;
        let listed: Vec<ListedJob> = reply.body().deserialize()?;
        Ok(listed.into_iter().map(JobListing::from).collect())
    }

    pub(crate) async fn job_kind(&mut self, job: &OwnedObjectPath) -> Result<JobKind, LinkFailure> {
        let kind = match self.property(job.as_str(), JOB_INTERFACE, "JobType").await {
            Err(LinkFailure::Refused(refusal)) if refusal.name == UNKNOWN_OBJECT => {
                return Ok(JobKind::Unknown);
            }
            answered => answered?,
        };
        Ok(job_kind_from(&String::try_from(kind)?))
    }

    pub(crate) async fn fetch_unit(&mut self, name: &str) -> Result<UnitLookup, LinkFailure> {
        let path: OwnedObjectPath = match self.manager_call("GetUnit", &(name,)).await {
            Err(LinkFailure::Refused(refusal)) if refusal.name == NO_SUCH_UNIT => {
                return Ok(UnitLookup::NotLoaded);
            }
            answered => answered?.body().deserialize()?,
        };
        self.lookup_at(name, path).await
    }

    pub(crate) async fn load_unit(&mut self, name: &str) -> Result<UnitLookup, LinkFailure> {
        let path: OwnedObjectPath = self
            .manager_call("LoadUnit", &(name,))
            .await?
            .body()
            .deserialize()?;
        self.lookup_at(name, path).await
    }

    async fn lookup_at(
        &mut self,
        name: &str,
        path: OwnedObjectPath,
    ) -> Result<UnitLookup, LinkFailure> {
        match self.unit_at(name, path).await {
            Err(LinkFailure::Refused(refusal)) if refusal.name == UNKNOWN_OBJECT => {
                Ok(UnitLookup::NotLoaded)
            }
            fetched => Ok(UnitLookup::Loaded(Box::new(fetched?))),
        }
    }

    async fn unit_at(
        &mut self,
        name: &str,
        path: OwnedObjectPath,
    ) -> Result<FetchedUnit, LinkFailure> {
        let properties = self.all_properties(&path, UNIT_INTERFACE).await?;
        let kind_properties = match kind_interface(unit_kind_of(&unit_id(name, &properties))) {
            Some(interface) => self.all_properties(&path, interface).await?,
            None => PropertyMap::new(),
        };
        Ok(FetchedUnit {
            facts: unit_facts_from(name, &properties, &kind_properties),
            activity: activity_from(&properties),
            path,
        })
    }

    pub(crate) async fn failure_reason(
        &mut self,
        name: &str,
    ) -> Result<FailureReason, LinkFailure> {
        let Some(interface) = kind_interface(unit_kind_of(name)) else {
            return Ok(FailureReason::Unexplained);
        };
        let path: OwnedObjectPath = match self.manager_call("GetUnit", &(name,)).await {
            Err(LinkFailure::Refused(refusal)) if refusal.name == NO_SUCH_UNIT => {
                return Ok(FailureReason::Unexplained);
            }
            answered => answered?.body().deserialize()?,
        };
        match self.all_properties(&path, interface).await {
            Err(LinkFailure::Refused(refusal)) if refusal.name == UNKNOWN_OBJECT => {
                Ok(FailureReason::Unexplained)
            }
            properties => Ok(FailureReason::from(&properties?)),
        }
    }

    async fn all_properties(
        &mut self,
        path: &OwnedObjectPath,
        interface: &str,
    ) -> Result<PropertyMap, LinkFailure> {
        Ok(self
            .call(path.as_str(), PROPERTIES_INTERFACE, "GetAll", &(interface,))
            .await?
            .body()
            .deserialize()?)
    }

    pub(crate) async fn wait_for_signals(&mut self) {
        if self.early_signals.is_empty()
            && let SignalStreamState::Open = self.signal_stream_state
        {
            let arrived = self.signal_stream.next().await;
            self.keep_arrived_signal(arrived);
        }
    }

    pub(crate) fn take_signals(&mut self) -> SignalBatch {
        while let SignalStreamState::Open = self.signal_stream_state
            && let Some(arrived) = self.signal_stream.next().now_or_never()
        {
            self.keep_arrived_signal(arrived);
        }
        let signals = mem::take(&mut self.early_signals);
        match self.signal_stream_state {
            SignalStreamState::Open => SignalBatch::Open(signals),
            SignalStreamState::Ended => SignalBatch::Closed(signals),
        }
    }

    async fn next_signals(&mut self) -> SignalBatch {
        self.wait_for_signals().await;
        self.take_signals()
    }

    fn into_signals(self) -> impl Stream<Item = Signal> {
        unfold_carrying(self, |mut connection| async move {
            match connection.next_signals().await {
                SignalBatch::Open(signals) => Some(CarriedItem {
                    item: signals,
                    state: connection,
                }),
                SignalBatch::Closed(_) => None,
            }
        })
        .flat_map(stream::iter)
    }
}

fn text_property(properties: &PropertyMap, property: &str) -> Option<String> {
    match properties.get(property).map(|value| &**value) {
        Some(Variant::Str(text)) => Some(text.as_str().to_owned()),
        _ => None,
    }
}

fn unsigned_property(properties: &PropertyMap, property: &str) -> Option<u64> {
    match properties.get(property).map(|value| &**value) {
        Some(Variant::U64(number)) => Some(*number),
        _ => None,
    }
}

fn signal_name(number: i32) -> String {
    let name = match number {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        other => return format!("signal {other}"),
    };
    name.to_owned()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FailureReason {
    Explained(String),
    Unexplained,
}

impl From<&PropertyMap> for FailureReason {
    fn from(properties: &PropertyMap) -> Self {
        let Some(result) = text_property(properties, "Result").filter(|result| result != "success")
        else {
            return Self::Unexplained;
        };
        let status = match properties.get("ExecMainStatus").map(|value| &**value) {
            Some(Variant::I32(status)) => Some(*status),
            _ => None,
        };
        let reason = match (result.as_str(), status) {
            ("exit-code", Some(status)) if status != 0 => format!("exited with status {status}"),
            ("exit-code", _) => "exited with an error".to_owned(),
            ("signal", Some(signal)) => format!("killed by {}", signal_name(signal)),
            ("core-dump", Some(signal)) => format!("dumped core on {}", signal_name(signal)),
            ("timeout", _) => "timed out".to_owned(),
            ("watchdog", _) => "stopped answering the watchdog".to_owned(),
            ("start-limit-hit", _) => "restarted too often".to_owned(),
            ("oom-kill", _) => "killed for running out of memory".to_owned(),
            ("resources", _) => "could not set up its resources".to_owned(),
            ("protocol", _) => "broke the start-up protocol".to_owned(),
            (other, _) => other.to_owned(),
        };
        Self::Explained(
            match text_property(properties, "StatusText").filter(|text| !text.is_empty()) {
                Some(status_text) => format!("{reason}: {status_text}"),
                None => reason,
            },
        )
    }
}

fn unit_id(name: &str, properties: &PropertyMap) -> String {
    text_property(properties, "Id").unwrap_or_else(|| name.to_owned())
}

fn kind_interface(kind: UnitKind) -> Option<&'static str> {
    match kind {
        UnitKind::Service => Some(SERVICE_INTERFACE),
        UnitKind::Mount => Some(MOUNT_INTERFACE),
        UnitKind::Swap => Some(SWAP_INTERFACE),
        UnitKind::Socket => Some(SOCKET_INTERFACE),
        _ => None,
    }
}

fn time_limit(properties: &PropertyMap, property: &str) -> Option<Duration> {
    unsigned_property(properties, property)
        .map(Duration::from_micros)
        .filter(|limit| !limit.is_zero() && *limit != UNLIMITED)
}

fn timeouts_from(kind: UnitKind, unit: &PropertyMap, kind_specific: &PropertyMap) -> UnitTimeouts {
    let (start, stop) = if kind == UnitKind::Service {
        ("TimeoutStartUSec", "TimeoutStopUSec")
    } else {
        ("TimeoutUSec", "TimeoutUSec")
    };
    UnitTimeouts {
        start: time_limit(kind_specific, start),
        stop: time_limit(kind_specific, stop),
        job: time_limit(unit, "JobTimeoutUSec"),
        job_running: time_limit(unit, "JobRunningTimeoutUSec"),
    }
}

fn unit_facts_from(
    name: &str,
    properties: &PropertyMap,
    kind_properties: &PropertyMap,
) -> UnitFacts {
    let text = |property: &str| text_property(properties, property);
    let names = |property: &str| match properties.get(property).map(|value| &**value) {
        Some(Variant::Array(unit_names)) => unit_names
            .inner()
            .iter()
            .filter_map(|unit_name| match unit_name {
                Variant::Str(text) => Some(text.as_str().to_owned()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let id = unit_id(name, properties);
    let timeouts = timeouts_from(unit_kind_of(&id), properties, kind_properties);
    systemd_unit_facts(
        id,
        text("Description").filter(|description| !description.is_empty()),
        names("After"),
        names("Before"),
        timeouts,
    )
}

fn parse_signal(message: &Message) -> BusMessage {
    let header = message.header();
    let (MessageType::Signal, Some(interface), Some(member), Some(path)) = (
        header.message_type(),
        header.interface(),
        header.member(),
        header.path(),
    ) else {
        return BusMessage::Unrelated;
    };
    match (interface.as_str(), member.as_str()) {
        (MANAGER_INTERFACE, "JobNew") => {
            let (id, job, unit): (u32, OwnedObjectPath, String) = message
                .body()
                .deserialize()
                .expect("systemd sends JobNew with an id, a job path and a unit name");
            BusMessage::Signal(Signal::JobNew { id, job, unit })
        }
        (MANAGER_INTERFACE, "JobRemoved") => {
            let (id, _job, unit, result): (u32, OwnedObjectPath, String, String) =
                message.body().deserialize().expect(
                    "systemd sends JobRemoved with an id, a job path, a unit name and a result",
                );
            BusMessage::Signal(Signal::JobRemoved {
                id,
                unit,
                result: job_result_from(&result),
            })
        }
        (MANAGER_INTERFACE, "StartupFinished") => BusMessage::Signal(Signal::BootFinished),
        (PROPERTIES_INTERFACE, "PropertiesChanged") => {
            let (changed_interface, changed, _invalidated): (String, PropertyMap, Vec<String>) =
                message.body().deserialize().expect(
                    "PropertiesChanged carries an interface, changed and invalidated properties",
                );
            if changed_interface == UNIT_INTERFACE {
                BusMessage::Signal(Signal::UnitChanged {
                    path: OwnedObjectPath::from(path.to_owned()),
                    activity: activity_from(&changed),
                })
            } else {
                BusMessage::Unrelated
            }
        }
        _ => BusMessage::Unrelated,
    }
}

fn activity_from(properties: &PropertyMap) -> Activity {
    let monotonic = |property: &str| {
        unsigned_property(properties, property)
            .filter(|micros| *micros != 0)
            .map(Duration::from_micros)
    };
    Activity {
        state: text_property(properties, "ActiveState")
            .as_deref()
            .map(active_state_from),
        inactive_exit: monotonic("InactiveExitTimestampMonotonic"),
        active_enter: monotonic("ActiveEnterTimestampMonotonic"),
        active_exit: monotonic("ActiveExitTimestampMonotonic"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_finished_signal_marks_the_boot_finished() {
        let startup_finished = Message::signal(MANAGER_PATH, MANAGER_INTERFACE, "StartupFinished")
            .build(&(0_u64, 0_u64, 0_u64, 0_u64, 0_u64, 0_u64))
            .expect("a StartupFinished signal builds");
        assert!(matches!(
            parse_signal(&startup_finished),
            BusMessage::Signal(Signal::BootFinished)
        ));
    }

    fn limit_properties(entries: &[(&str, Duration)]) -> PropertyMap {
        entries
            .iter()
            .map(|(name, limit)| {
                let micros = u64::try_from(limit.as_micros()).unwrap_or(u64::MAX);
                ((*name).to_owned(), OwnedVariant::from(micros))
            })
            .collect()
    }

    #[test]
    fn unlimited_timeouts_are_absent() {
        let unit = limit_properties(&[
            ("JobTimeoutUSec", UNLIMITED),
            ("JobRunningTimeoutUSec", Duration::ZERO),
        ]);
        let service = limit_properties(&[
            ("TimeoutStartUSec", UNLIMITED),
            ("TimeoutStopUSec", Duration::from_secs(90)),
        ]);
        assert_eq!(
            timeouts_from(UnitKind::Service, &unit, &service),
            UnitTimeouts {
                stop: Some(Duration::from_secs(90)),
                ..UnitTimeouts::default()
            }
        );
    }

    fn reason_properties(result: &str, status: i32, status_text: &str) -> PropertyMap {
        [
            (
                "Result",
                OwnedVariant::try_from(Variant::from(result)).expect("a string has no fds"),
            ),
            ("ExecMainStatus", OwnedVariant::from(status)),
            (
                "StatusText",
                OwnedVariant::try_from(Variant::from(status_text)).expect("a string has no fds"),
            ),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
    }

    #[test]
    fn exit_codes_and_signals_become_a_reason() {
        assert_eq!(
            FailureReason::from(&reason_properties("exit-code", 1, "")),
            FailureReason::Explained("exited with status 1".to_owned())
        );
        assert_eq!(
            FailureReason::from(&reason_properties("signal", 11, "")),
            FailureReason::Explained("killed by SIGSEGV".to_owned())
        );
    }

    #[test]
    fn status_text_follows_the_reason() {
        assert_eq!(
            FailureReason::from(&reason_properties("timeout", 0, "waiting for the disk")),
            FailureReason::Explained("timed out: waiting for the disk".to_owned())
        );
    }

    #[test]
    fn success_has_no_reason() {
        assert_eq!(
            FailureReason::from(&reason_properties("success", 0, "")),
            FailureReason::Unexplained
        );
    }

    #[test]
    fn mounts_use_one_timeout_for_both_directions() {
        let mount = limit_properties(&[("TimeoutUSec", Duration::from_secs(20))]);
        let timeouts = timeouts_from(UnitKind::Mount, &PropertyMap::new(), &mount);
        assert_eq!(timeouts.start, Some(Duration::from_secs(20)));
        assert_eq!(timeouts.stop, Some(Duration::from_secs(20)));
    }
}
