use std::{future::Future, net::Shutdown, sync::Arc};

use russh::{
    Channel, ChannelId,
    keys::{Algorithm, PrivateKey, PublicKey, ssh_key::LineEnding},
    server,
};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::oneshot, time::timeout};

use super::*;
use crate::{Exit, HostKeyPin, Limits};

fn key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}

struct Fixture {
    endpoint: std::net::SocketAddr,
    host: PublicKey,
    user: PrivateKey,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.take();
    }
}

impl Fixture {
    async fn new(mode: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let host = key();
        let host_public = host.public_key().clone();
        let user = key();
        let handler = Peer {
            user: user.public_key().clone(),
            mode,
            received: Vec::new(),
        };
        let config = Arc::new(server::Config {
            keys: vec![host],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            window_size: if mode == "zero-window" { 0 } else { 128 * 1024 },
            ..server::Config::default()
        });
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let accepted = tokio::select! {
                result = listener.accept() => result.unwrap().0,
                _ = &mut stopped => return,
            };
            let socket = accepted.into_std().unwrap();
            let control = socket.try_clone().unwrap();
            let socket = tokio::net::TcpStream::from_std(socket).unwrap();
            let running = async {
                if let Ok(session) = server::run_stream(config, socket, handler).await {
                    let _ = session.await;
                }
            };
            tokio::pin!(running);
            tokio::select! {
                () = &mut running => {},
                _ = &mut stopped => {
                    let _ = control.shutdown(Shutdown::Both);
                    timeout(Duration::from_secs(3), running).await.unwrap();
                }
            }
        });
        Self {
            endpoint,
            host: host_public,
            user,
            stop: Some(stop),
            task: Some(task),
        }
    }

    fn request(&self, duration: Duration) -> ExecRequest {
        ExecRequest::new(
            self.endpoint,
            "test",
            "fixture-command",
            HostKeyPin::from_openssh(&self.host.to_openssh().unwrap()).unwrap(),
            Instant::now() + duration,
        )
        .unwrap()
    }

    fn credentials(&self) -> Credentials {
        Credentials::from_openssh(&self.user.to_openssh(LineEnding::LF).unwrap(), None).unwrap()
    }

    async fn close(mut self) {
        self.stop.take();
        timeout(Duration::from_secs(4), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}

struct Peer {
    user: PublicKey,
    mode: &'static str,
    received: Vec<u8>,
}

impl server::Handler for Peer {
    type Error = russh::Error;

    fn auth_publickey(
        &mut self,
        user: &str,
        public: &PublicKey,
    ) -> impl Future<Output = Result<server::Auth, Self::Error>> {
        std::future::ready(Ok(if user == "test" && public.key_data() == self.user.key_data() {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        }))
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    fn exec_request(
        &mut self,
        channel: ChannelId,
        _: &[u8],
        session: &mut server::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> {
        std::future::ready(self.exec(channel, session))
    }

    fn data(
        &mut self,
        _: ChannelId,
        data: &[u8],
        _: &mut server::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> {
        assert!(self.received.len() + data.len() <= 1024 * 1024);
        self.received.extend_from_slice(data);
        std::future::ready(Ok(()))
    }

    fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> {
        std::future::ready(self.eof(channel, session))
    }
}

impl Peer {
    fn exec(&self, channel: ChannelId, session: &mut server::Session) -> Result<(), russh::Error> {
        if self.mode == "reject" {
            session.channel_failure(channel)?;
            return Ok(());
        }
        session.channel_success(channel)?;
        match self.mode {
            "split" => {
                // Exit status must not truncate later output.
                session.exit_status_request(channel, 37)?;
                session.data(channel, b"out\0\xff".to_vec())?;
                session.extended_data(channel, 1, b"err\0\xfe".to_vec())?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            "missing-exit" => {
                session.eof(channel)?;
                session.close(channel)?;
            }
            "flood" => {
                session.data(channel, vec![b'x'; 512 * 1024])?;
            }
            "signal-custom" | "signal-standard" | "signal-limit" | "signal-oversized" => {
                let signal = match self.mode {
                    "signal-standard" => russh::Sig::TERM,
                    "signal-limit" => russh::Sig::Custom("x".repeat(128)),
                    "signal-oversized" => russh::Sig::Custom("x".repeat(129)),
                    _ => russh::Sig::Custom("USR2".to_owned()),
                };
                session.exit_signal_request(channel, signal, false, "", "")?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn eof(&self, channel: ChannelId, session: &mut server::Session) -> Result<(), russh::Error> {
        if self.mode == "echo-eof" {
            session.data(channel, self.received.clone())?;
            session.extended_data(channel, 1, b"after-stdin-eof".to_vec())?;
            session.eof(channel)?;
            session.exit_status_request(channel, 0)?;
            session.close(channel)?;
        }
        Ok(())
    }
}

async fn terminal(command: &mut Command) -> Result<Completion, Error> {
    timeout(Duration::from_secs(5), async {
        while command.next_event().await.is_some() {}
        command.completion().await
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn binary_streams_exit_and_eof_stay_separate_and_output_after_status_drains() {
    let fixture = Fixture::new("split").await;
    let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut order = Vec::new();
    while let Some(event) = command.next_event().await {
        match event {
            Event::ExecAccepted => order.push("accepted"),
            Event::Stdout(bytes) => {
                out.extend(bytes);
                order.push("stdout");
            }
            Event::Stderr(bytes) => {
                err.extend(bytes);
                order.push("stderr");
            }
            Event::Exit(Exit::Status(37)) => order.push("status"),
            Event::RemoteEof => order.push("eof"),
            Event::RemoteClose => order.push("close"),
            Event::Exit(_) => panic!("unexpected event"),
        }
    }
    assert_eq!(out, b"out\0\xff");
    assert_eq!(err, b"err\0\xfe");
    assert_eq!(order, ["accepted", "status", "stdout", "stderr", "eof", "close"]);
    assert_eq!(
        command.completion().await.unwrap(),
        Completion {
            exit: Exit::Status(37),
            remote_eof: true
        }
    );
    fixture.close().await;
}

#[tokio::test]
async fn signal_event_and_completion_preserve_names_but_debug_redacts_them() {
    for (mode, name) in [
        ("signal-custom", "USR2".to_owned()),
        ("signal-standard", "TERM".to_owned()),
        ("signal-limit", "x".repeat(128)),
    ] {
        let fixture = Fixture::new(mode).await;
        let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
        let expected = Exit::Signal {
            name,
            core_dumped: false,
        };
        let mut exits = Vec::new();
        while let Some(event) = command.next_event().await {
            if let Event::Exit(exit) = event {
                exits.push(exit);
            }
        }
        assert_eq!(exits.as_slice(), std::slice::from_ref(&expected));
        let completion = command.completion().await.unwrap();
        assert_eq!(
            completion,
            Completion {
                exit: expected,
                remote_eof: true
            }
        );
        let Exit::Signal { name, .. } = &completion.exit else {
            panic!("expected signal");
        };
        for debug in [format!("{:?}", completion.exit), format!("{completion:?}")] {
            assert!(!debug.contains(name.as_str()), "untrusted signal name must be redacted");
            assert!(debug.contains("<redacted>"));
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn oversized_signal_name_is_refused_before_exit_event() {
    let fixture = Fixture::new("signal-oversized").await;
    let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
    while let Some(event) = command.next_event().await {
        assert!(!matches!(event, Event::Exit(_)));
    }
    assert_eq!(command.completion().await, Err(Error::Protocol));
    fixture.close().await;
}

#[tokio::test]
async fn stdin_eof_keeps_return_path_alive() {
    let fixture = Fixture::new("echo-eof").await;
    let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
    let mut input = command.take_input().unwrap();
    input.write(b"binary\0\xff").await.unwrap();
    input.eof().await.unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    while let Some(event) = command.next_event().await {
        match event {
            Event::Stdout(bytes) => out.extend(bytes),
            Event::Stderr(bytes) => err.extend(bytes),
            _ => {}
        }
    }
    assert_eq!(out, b"binary\0\xff");
    assert_eq!(err, b"after-stdin-eof");
    assert_eq!(command.completion().await.unwrap().exit, Exit::Status(0));
    fixture.close().await;
}

#[tokio::test]
async fn wrong_host_pin_and_wrong_auth_key_are_distinct_failures() {
    let fixture = Fixture::new("split").await;
    let mut request = fixture.request(Duration::from_secs(3));
    request.pin = HostKeyPin(key().public_key().clone());
    let mut command = execute(request, fixture.credentials()).unwrap();
    assert_eq!(terminal(&mut command).await, Err(Error::HostKeyMismatch));
    fixture.close().await;
    let fixture = Fixture::new("split").await;
    let wrong = Credentials(Arc::new(key()));
    let mut command = execute(fixture.request(Duration::from_secs(3)), wrong).unwrap();
    assert_eq!(terminal(&mut command).await, Err(Error::Authentication));
    fixture.close().await;
}

#[tokio::test]
async fn command_refusal_and_missing_exit_are_not_success() {
    for (mode, expected) in [("reject", Error::ExecRejected), ("missing-exit", Error::MissingExit)] {
        let fixture = Fixture::new(mode).await;
        let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
        assert_eq!(terminal(&mut command).await, Err(expected));
        fixture.close().await;
    }
}

#[tokio::test]
async fn cumulative_output_limit_refuses_remote_flood() {
    let fixture = Fixture::new("flood").await;
    let request = fixture
        .request(Duration::from_secs(3))
        .with_limits(Limits {
            stdout: 4096,
            ..Limits::default()
        })
        .unwrap();
    let mut command = execute(request, fixture.credentials()).unwrap();
    assert_eq!(terminal(&mut command).await, Err(Error::DataLimit));
    fixture.close().await;
}

#[tokio::test]
async fn full_event_queue_can_cancel_and_does_not_block_another_connection() {
    let fixture = Fixture::new("flood").await;
    let command = execute(fixture.request(Duration::from_secs(4)), fixture.credentials()).unwrap();
    timeout(Duration::from_secs(2), async {
        while command.events.len() < QUEUE {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let other = Fixture::new("split").await;
    let mut independent = execute(other.request(Duration::from_secs(2)), other.credentials()).unwrap();
    assert_eq!(terminal(&mut independent).await.unwrap().exit, Exit::Status(37));
    other.close().await;
    assert_eq!(
        timeout(Duration::from_secs(3), command.shutdown()).await.unwrap(),
        Err(Error::Cancelled)
    );
    fixture.close().await;
}

#[tokio::test]
async fn zero_remote_window_cancellation_wakes_full_input_queue() {
    let fixture = Fixture::new("zero-window").await;
    let mut command = execute(fixture.request(Duration::from_secs(4)), fixture.credentials()).unwrap();
    assert!(matches!(command.next_event().await, Some(Event::ExecAccepted)));
    let mut input = command.take_input().unwrap();
    let queue = input.sender.clone();
    let writer = tokio::spawn(async move {
        for _ in 0..16 {
            input.write(&vec![b'x'; CHUNK]).await?;
        }
        Ok::<(), Error>(())
    });
    timeout(Duration::from_secs(2), async {
        while queue.capacity() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    command.cancellation().cancel();
    assert_eq!(
        timeout(Duration::from_secs(2), writer).await.unwrap().unwrap(),
        Err(Error::Cancelled)
    );
    assert_eq!(command.shutdown().await, Err(Error::Cancelled));
    fixture.close().await;
}

#[tokio::test]
async fn active_command_deadline_retires_transport() {
    let fixture = Fixture::new("stall").await;
    let mut command = execute(fixture.request(Duration::from_secs(3)), fixture.credentials()).unwrap();
    assert!(matches!(command.next_event().await, Some(Event::ExecAccepted)));
    assert_eq!(terminal(&mut command).await, Err(Error::Deadline));
    fixture.close().await;
}

#[tokio::test]
async fn stalled_handshake_deadline_closes_original_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let host = key();
    let request = ExecRequest::new(
        endpoint,
        "test",
        "fixture-command",
        HostKeyPin(host.public_key().clone()),
        Instant::now() + Duration::from_millis(200),
    )
    .unwrap();
    let user = Credentials(Arc::new(key()));
    let mut command = execute(request, user).unwrap();
    let mut socket = listener.accept().await.unwrap().0;
    assert_eq!(terminal(&mut command).await, Err(Error::Deadline));
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.starts_with(b"SSH-2.0-"));
}
