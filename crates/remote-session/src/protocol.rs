use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use russh::{
    ChannelMsg, client,
    keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::Instant,
};

use crate::{
    Cancellation, Completion, Credentials, Error, Event, ExecRequest, Exit,
    command::{CHUNK, Input, QUEUE, SHUTDOWN},
    control::Transport,
};

struct PinHandler {
    pin: crate::HostKeyPin,
    mismatch: Arc<AtomicBool>,
}

struct CloseOnDrop(Cancellation);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl client::Handler for PinHandler {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        presented: &PublicKeyOrCertificate,
    ) -> impl Future<Output = Result<bool, Self::Error>> {
        let accepted = matches!(presented, PublicKeyOrCertificate::PublicKey { key, .. }
            if key.key_data() == self.pin.0.key_data());
        if !accepted {
            self.mismatch.store(true, Ordering::Release);
        }
        std::future::ready(Ok(accepted))
    }
}

async fn bounded<T>(
    future: impl Future<Output = Result<T, Error>>,
    cancelled: &mut watch::Receiver<bool>,
    deadline: Instant,
) -> Result<T, Error> {
    tokio::select! {
        biased;
        _ = cancelled.wait_for(|value| *value) => Err(Error::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(Error::Deadline),
        result = future => result,
    }
}

pub(crate) async fn run(
    request: ExecRequest,
    credentials: Credentials,
    control: Cancellation,
    input: mpsc::Receiver<Input>,
    events: mpsc::Sender<Event>,
) -> Result<Completion, Error> {
    let _close_on_drop = CloseOnDrop(control.clone());
    let mut cancelled = control.subscribe();
    let socket = bounded(
        async { TcpStream::connect(request.endpoint).await.map_err(|_| Error::Transport) },
        &mut cancelled,
        request.deadline,
    )
    .await?;
    socket.set_nodelay(true).map_err(|_| Error::Transport)?;
    let socket = socket.into_std().map_err(|_| Error::Transport)?;
    control.install(socket.try_clone().map_err(|_| Error::Transport)?)?;
    let socket = Transport::new(TcpStream::from_std(socket).map_err(|_| Error::Transport)?, &control);
    let config = Arc::new(client::Config {
        window_size: 128 * 1024,
        maximum_packet_size: u32::try_from(CHUNK).map_err(|_| Error::InvalidOptions)?,
        channel_buffer_size: QUEUE,
        nodelay: true,
        ..client::Config::default()
    });
    let mismatch = Arc::new(AtomicBool::new(false));
    let handler = PinHandler {
        pin: request.pin,
        mismatch: Arc::clone(&mismatch),
    };
    let connecting = client::connect_stream(config, socket, handler);
    tokio::pin!(connecting);
    let connected = tokio::select! {
        biased;
        _ = cancelled.wait_for(|value| *value) => Err(Error::Cancelled),
        () = tokio::time::sleep_until(request.deadline) => Err(Error::Deadline),
        result = &mut connecting => {
            result.map_err(|_| if mismatch.load(Ordering::Acquire) { Error::HostKeyMismatch } else { Error::Transport })
        }
    };
    let mut session = match connected {
        Ok(session) => session,
        Err(reason @ (Error::Cancelled | Error::Deadline)) => {
            // connect_stream spawns its own session during KEX: close the original
            // TCP connection, then finish the still-owned connect future and join it.
            control.cancel();
            let retire = async {
                if let Ok(session) = connecting.await {
                    let _ = session.await;
                }
            };
            tokio::time::timeout(SHUTDOWN, retire)
                .await
                .map_err(|_| Error::ShutdownTimeout)?;
            return Err(reason);
        }
        Err(error) => {
            control.cancel();
            return Err(error);
        }
    };
    let result = bounded(
        exchange(
            &mut session,
            credentials,
            &request.username,
            &request.command,
            request.limits,
            input,
            events,
        ),
        &mut cancelled,
        request.deadline,
    )
    .await;
    control.cancel();
    // Socket shutdown is independent of russh's bounded message queue. Do not
    // wait to enqueue disconnect behind a blocked input/output operation.
    tokio::time::timeout(SHUTDOWN, session)
        .await
        .map_err(|_| Error::ShutdownTimeout)?
        .ok();
    result
}

async fn exchange(
    session: &mut client::Handle<PinHandler>,
    credentials: Credentials,
    username: &str,
    command: &str,
    limits: crate::Limits,
    input: mpsc::Receiver<Input>,
    events: mpsc::Sender<Event>,
) -> Result<Completion, Error> {
    let authenticated = session
        .authenticate_publickey(username, PrivateKeyWithHashAlg::new(credentials.0, None))
        .await
        .map_err(|_| Error::Transport)?;
    if !authenticated.success() {
        return Err(Error::Authentication);
    }
    let mut channel = session.channel_open_session().await.map_err(|_| Error::Transport)?;
    channel
        .exec(true, command.as_bytes())
        .await
        .map_err(|_| Error::Transport)?;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => {
                emit(&events, Event::ExecAccepted).await?;
                break;
            }
            Some(ChannelMsg::WindowAdjusted { .. }) => {}
            Some(ChannelMsg::Failure) => return Err(Error::ExecRejected),
            _ => return Err(Error::Protocol),
        }
    }
    let (read, write) = channel.split();
    let output = read_output(read, limits, events);
    tokio::pin!(output);
    tokio::select! {
        result = &mut output => result,
        result = write_input(write, input) => {
            result?;
            output.await
        }
    }
}

async fn write_input(
    write: russh::ChannelWriteHalf<client::Msg>,
    mut input: mpsc::Receiver<Input>,
) -> Result<(), Error> {
    while let Some(message) = input.recv().await {
        match message {
            Input::Data(bytes) => write.data(&bytes[..]).await.map_err(|_| Error::Transport)?,
            Input::Eof => break,
        }
    }
    write.eof().await.map_err(|_| Error::Transport)
}

async fn emit(events: &mpsc::Sender<Event>, event: Event) -> Result<(), Error> {
    events.send(event).await.map_err(|_| Error::Cancelled)
}

async fn read_output(
    mut read: russh::ChannelReadHalf,
    limits: crate::Limits,
    events: mpsc::Sender<Event>,
) -> Result<Completion, Error> {
    let mut stdout = 0usize;
    let mut stderr = 0usize;
    let mut eof = false;
    let mut exit = None;
    while let Some(message) = read.wait().await {
        let event = match message {
            ChannelMsg::Data { data } => {
                charge(&mut stdout, data.len(), limits.stdout, eof)?;
                Event::Stdout(data.to_vec())
            }
            ChannelMsg::ExtendedData { data, ext: 1 } => {
                charge(&mut stderr, data.len(), limits.stderr, eof)?;
                Event::Stderr(data.to_vec())
            }
            ChannelMsg::Eof if !eof => {
                eof = true;
                Event::RemoteEof
            }
            ChannelMsg::ExitStatus { exit_status } if exit.is_none() => {
                let status = Exit::Status(exit_status);
                exit = Some(status.clone());
                Event::Exit(status)
            }
            ChannelMsg::ExitSignal {
                signal_name,
                core_dumped,
                ..
            } if exit.is_none() => {
                let name = signal_name_text(&signal_name);
                if name.len() > 128 {
                    return Err(Error::Protocol);
                }
                let signal = Exit::Signal {
                    name: name.to_owned(),
                    core_dumped,
                };
                exit = Some(signal.clone());
                Event::Exit(signal)
            }
            ChannelMsg::Close => {
                emit(&events, Event::RemoteClose).await?;
                return Ok(Completion {
                    exit: exit.ok_or(Error::MissingExit)?,
                    remote_eof: eof,
                });
            }
            ChannelMsg::WindowAdjusted { .. } => continue,
            _ => return Err(Error::Protocol),
        };
        emit(&events, event).await?;
    }
    Err(Error::Transport)
}

fn signal_name_text(signal: &russh::Sig) -> &str {
    match signal {
        russh::Sig::ABRT => "ABRT",
        russh::Sig::ALRM => "ALRM",
        russh::Sig::FPE => "FPE",
        russh::Sig::HUP => "HUP",
        russh::Sig::ILL => "ILL",
        russh::Sig::INT => "INT",
        russh::Sig::KILL => "KILL",
        russh::Sig::PIPE => "PIPE",
        russh::Sig::QUIT => "QUIT",
        russh::Sig::SEGV => "SEGV",
        russh::Sig::TERM => "TERM",
        russh::Sig::USR1 => "USR1",
        russh::Sig::Custom(name) => name,
    }
}

fn charge(total: &mut usize, bytes: usize, limit: usize, eof: bool) -> Result<(), Error> {
    if eof || bytes > CHUNK {
        return Err(Error::Protocol);
    }
    *total = total
        .checked_add(bytes)
        .filter(|total| *total <= limit)
        .ok_or(Error::DataLimit)?;
    Ok(())
}
