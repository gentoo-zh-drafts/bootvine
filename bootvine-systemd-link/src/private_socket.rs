use std::{
    io,
    os::fd::{BorrowedFd, OwnedFd},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
};
use zbus::{
    connection::socket::{ReadHalf, Socket, Split, WriteHalf},
    export::async_trait::async_trait,
};

const PRIMARY_HEADER_LENGTH: usize = 16;
const LARGEST_MESSAGE_LENGTH: usize = 128 * 1024 * 1024;
const SENDER_FIELD_CODE: u8 = 7;
const UNIQUE_NAME_MARKER: u8 = b':';
const READ_CHUNK_LENGTH: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct SystemdPrivateSocket(pub(crate) UnixStream);

impl Socket for SystemdPrivateSocket {
    type ReadHalf = SenderRewritingReader;
    type WriteHalf = PlainSocketWriter;

    fn split(self) -> Split<SenderRewritingReader, PlainSocketWriter> {
        let (read_half, write_half) = self.0.into_split();
        Split::new(
            SenderRewritingReader {
                stream: read_half,
                unread_bytes: Vec::new(),
                rewritten_bytes: Vec::new(),
                stage: ReaderStage::Handshake,
            },
            PlainSocketWriter(write_half),
        )
    }
}

#[derive(Debug)]
enum ReaderStage {
    Handshake,
    Messages,
}

enum ByteOrder {
    Little,
    Big,
}

enum FramedBytes {
    Complete(usize),
    Incomplete,
    Unframeable,
}

#[derive(Debug)]
pub(crate) struct SenderRewritingReader {
    stream: OwnedReadHalf,
    unread_bytes: Vec<u8>,
    rewritten_bytes: Vec<u8>,
    stage: ReaderStage,
}

impl SenderRewritingReader {
    fn move_unread_to_rewritten(&mut self, length: usize) {
        self.rewritten_bytes
            .extend(self.unread_bytes.drain(..length));
    }

    fn rewrite_what_is_complete(&mut self) {
        loop {
            match self.stage {
                ReaderStage::Handshake => {
                    let Some(line_end) = self
                        .unread_bytes
                        .windows(2)
                        .position(|pair| pair == b"\r\n")
                    else {
                        return;
                    };
                    if self.unread_bytes.starts_with(b"OK ") {
                        self.stage = ReaderStage::Messages;
                    }
                    self.move_unread_to_rewritten(line_end + 2);
                }
                ReaderStage::Messages => match framed_message_length(&self.unread_bytes) {
                    FramedBytes::Complete(length) => {
                        relabel_well_known_sender(&mut self.unread_bytes[..length]);
                        self.move_unread_to_rewritten(length);
                    }
                    FramedBytes::Incomplete => return,
                    FramedBytes::Unframeable => {
                        self.move_unread_to_rewritten(self.unread_bytes.len());
                        return;
                    }
                },
            }
        }
    }
}

#[async_trait]
impl ReadHalf for SenderRewritingReader {
    async fn recvmsg(&mut self, buffer: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
        while self.rewritten_bytes.is_empty() {
            let mut received_chunk = vec![0; READ_CHUNK_LENGTH];
            let received_length = self.stream.read(&mut received_chunk).await?;
            if received_length == 0 {
                return Ok((0, Vec::new()));
            }
            self.unread_bytes
                .extend_from_slice(&received_chunk[..received_length]);
            self.rewrite_what_is_complete();
        }
        let handed_length = buffer.len().min(self.rewritten_bytes.len());
        buffer[..handed_length].copy_from_slice(&self.rewritten_bytes[..handed_length]);
        self.rewritten_bytes.drain(..handed_length);
        Ok((handed_length, Vec::new()))
    }
}

#[derive(Debug)]
pub(crate) struct PlainSocketWriter(OwnedWriteHalf);

#[async_trait]
impl WriteHalf for PlainSocketWriter {
    async fn sendmsg(&mut self, buffer: &[u8], _fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
        self.0.write(buffer).await
    }

    async fn close(&mut self) -> io::Result<()> {
        self.0.shutdown().await
    }

    fn can_resume_writes(&self) -> bool {
        true
    }
}

fn read_u32(bytes: &[u8], offset: usize, order: &ByteOrder) -> Option<usize> {
    let length_bytes: [u8; 4] = bytes
        .get(offset..offset + 4)?
        .try_into()
        .expect("a range of four bytes is four bytes long");
    let decoded_length = match order {
        ByteOrder::Little => u32::from_le_bytes(length_bytes),
        ByteOrder::Big => u32::from_be_bytes(length_bytes),
    };
    Some(usize::try_from(decoded_length).expect("a u32 fits in a usize on Linux"))
}

fn byte_order_of(message: &[u8]) -> Option<ByteOrder> {
    match message.first()? {
        b'l' => Some(ByteOrder::Little),
        b'B' => Some(ByteOrder::Big),
        _ => None,
    }
}

fn framed_message_length(bytes: &[u8]) -> FramedBytes {
    if bytes.len() < PRIMARY_HEADER_LENGTH {
        return FramedBytes::Incomplete;
    }
    let Some(order) = byte_order_of(bytes) else {
        return FramedBytes::Unframeable;
    };
    let (Some(body_length), Some(fields_length)) =
        (read_u32(bytes, 4, &order), read_u32(bytes, 12, &order))
    else {
        return FramedBytes::Unframeable;
    };
    let length = (PRIMARY_HEADER_LENGTH + fields_length).next_multiple_of(8) + body_length;
    if length > LARGEST_MESSAGE_LENGTH {
        FramedBytes::Unframeable
    } else if bytes.len() < length {
        FramedBytes::Incomplete
    } else {
        FramedBytes::Complete(length)
    }
}

fn relabel_well_known_sender(message: &mut [u8]) {
    let Some(order) = byte_order_of(message) else {
        return;
    };
    let Some(fields_length) = read_u32(message, 12, &order) else {
        return;
    };
    let fields_end = PRIMARY_HEADER_LENGTH + fields_length;
    let mut field_start = PRIMARY_HEADER_LENGTH;
    while field_start + 4 <= fields_end {
        let Some(&[code, 1, value_type, 0]) = message.get(field_start..field_start + 4) else {
            return;
        };
        let value_start = field_start + 4;
        let value_end = match value_type {
            b's' | b'o' => {
                let Some(text_length) = read_u32(message, value_start, &order) else {
                    return;
                };
                if code == SENDER_FIELD_CODE
                    && text_length >= 2
                    && let Some(name) =
                        message.get_mut(value_start + 4..value_start + 4 + text_length)
                    && name[0] != UNIQUE_NAME_MARKER
                    && name[1] != b'.'
                {
                    name[0] = UNIQUE_NAME_MARKER;
                }
                value_start + 4 + text_length + 1
            }
            b'g' => {
                let Some(&signature_length) = message.get(value_start) else {
                    return;
                };
                value_start + 1 + usize::from(signature_length) + 1
            }
            b'u' => value_start + 4,
            _ => return,
        };
        field_start = value_end.next_multiple_of(8);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{StreamExt, future::try_join};
    use rustix::process::getuid;
    use tokio::{runtime::Builder as RuntimeBuilder, time::timeout};
    use zbus::{
        Connection, Guid, Message, MessageStream, connection::Builder as ConnectionBuilder,
        fdo::ConnectionCredentials, message::Type as MessageType, names::MemberName,
    };

    use super::*;

    const DISGUISED_SENDER: &[u8] = b":rg.freedesktop.systemd1";
    const WELL_KNOWN_SENDER: &[u8] = b"org.freedesktop.systemd1";

    #[derive(Debug)]
    struct WellKnownSenderSocket(UnixStream);

    #[derive(Debug)]
    struct WellKnownSenderWriter(OwnedWriteHalf);

    #[derive(Debug)]
    struct PlainSocketReader(OwnedReadHalf);

    impl Socket for WellKnownSenderSocket {
        type ReadHalf = PlainSocketReader;
        type WriteHalf = WellKnownSenderWriter;

        fn split(self) -> Split<PlainSocketReader, WellKnownSenderWriter> {
            let (read_half, write_half) = self.0.into_split();
            Split::new(
                PlainSocketReader(read_half),
                WellKnownSenderWriter(write_half),
            )
        }
    }

    #[async_trait]
    impl ReadHalf for PlainSocketReader {
        async fn recvmsg(&mut self, buffer: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
            Ok((self.0.read(buffer).await?, Vec::new()))
        }

        async fn peer_credentials(&mut self) -> io::Result<ConnectionCredentials> {
            Ok(ConnectionCredentials::default().set_unix_user_id(getuid().as_raw()))
        }
    }

    #[async_trait]
    impl WriteHalf for WellKnownSenderWriter {
        async fn sendmsg(&mut self, buffer: &[u8], _fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
            let mut on_the_wire = buffer.to_vec();
            if let Some(start) = on_the_wire
                .windows(DISGUISED_SENDER.len())
                .position(|window| window == DISGUISED_SENDER)
            {
                on_the_wire[start..start + WELL_KNOWN_SENDER.len()]
                    .copy_from_slice(WELL_KNOWN_SENDER);
            }
            self.0.write_all(&on_the_wire).await?;
            Ok(buffer.len())
        }

        async fn close(&mut self) -> io::Result<()> {
            self.0.shutdown().await
        }
    }

    async fn answer_like_systemd(manager: Connection) -> zbus::Result<()> {
        let mut incoming = MessageStream::from(&manager);
        while let Some(call) = incoming.next().await {
            let call = call?;
            if call.message_type() != MessageType::MethodCall {
                continue;
            }
            let job_new = Message::signal(
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "JobNew",
            )
            .sender(":rg.freedesktop.systemd1")
            .build(&(
                7_u32,
                zbus::ObjectPath::try_from("/org/freedesktop/systemd1/job/7")?,
                "slow.service",
            ))?;
            manager.send(&job_new).await?;
            let default_target_return = Message::method_return(&call.header())
                .sender(":rg.freedesktop.systemd1")
                .build(&("graphical.target",))?;
            manager.send(&default_target_return).await?;
        }
        Ok(())
    }

    struct ConnectedPair {
        client: Connection,
        manager: Connection,
    }

    async fn connected_pair(
        client_socket: impl FnOnce(UnixStream) -> ConnectionBuilder<'static>,
    ) -> zbus::Result<ConnectedPair> {
        let (client_end, manager_end) = UnixStream::pair()?;
        let client = client_socket(client_end).p2p().build();
        let manager = ConnectionBuilder::socket(WellKnownSenderSocket(manager_end))
            .server(Guid::generate())
            .p2p()
            .build();
        let (client, manager) = try_join(client, manager).await?;
        Ok(ConnectedPair { client, manager })
    }

    async fn default_target_call(client: &Connection) -> zbus::Result<String> {
        let default_target_reply = client
            .call_method(
                Some("org.freedesktop.systemd1"),
                "/org/freedesktop/systemd1",
                Some("org.freedesktop.systemd1.Manager"),
                "GetDefaultTarget",
                &(),
            )
            .await?;
        default_target_reply.body().deserialize()
    }

    fn run_on_tokio<Output>(future: impl Future<Output = Output>) -> Output {
        RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .expect("a tokio runtime starts")
            .block_on(future)
    }

    #[test]
    fn replies_under_the_well_known_name_reach_the_caller() {
        run_on_tokio(async {
            let ConnectedPair { client, manager } =
                connected_pair(|stream| ConnectionBuilder::socket(SystemdPrivateSocket(stream)))
                    .await
                    .expect("the client and the fake manager connect");
            let mut signals = MessageStream::from(&client);
            tokio::spawn(answer_like_systemd(manager));
            let target = timeout(Duration::from_secs(5), default_target_call(&client))
                .await
                .expect("the reply arrives in time")
                .expect("the reply is accepted");
            assert_eq!(target, "graphical.target");
            let job_new = signals
                .next()
                .await
                .expect("the signal stream is open")
                .expect("the signal is accepted");
            assert_eq!(
                job_new.header().member().map(MemberName::as_str),
                Some("JobNew")
            );
        });
    }

    #[test]
    fn without_the_rewrite_zbus_drops_the_reply() {
        run_on_tokio(async {
            let ConnectedPair { client, manager } = connected_pair(|stream| {
                ConnectionBuilder::unix_stream(stream.into_std().expect("a tokio socket converts"))
            })
            .await
            .expect("the client and the fake manager connect");
            tokio::spawn(answer_like_systemd(manager));
            let outcome = timeout(Duration::from_secs(2), default_target_call(&client)).await;
            assert!(
                !matches!(outcome, Ok(Ok(_))),
                "zbus accepted a well-known sender: {outcome:?}"
            );
        });
    }

    #[test]
    fn unique_senders_are_left_alone() {
        let mut header_only = vec![b'l', 2, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 13, 0, 0, 0];
        header_only.extend_from_slice(&[7, 1, b's', 0, 4, 0, 0, 0]);
        header_only.extend_from_slice(b":1.5\0");
        header_only.extend_from_slice(&[0, 0, 0]);
        let before = header_only.clone();
        relabel_well_known_sender(&mut header_only);
        assert_eq!(header_only, before);
    }
}
