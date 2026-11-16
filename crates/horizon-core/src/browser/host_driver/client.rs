//! This computer's end of the link to a driver that runs beside a browser on a
//! browser host.
//!
//! One `ssh` child runs the driver of the host, and this module speaks the
//! protocol of [`horizon_browser::host_driver`] on its standard input and
//! output. The picture arrives pushed: a frame that comes in is decoded into the
//! frame slot of the panel and acknowledged, which is what lets the driver send
//! the next one. Nothing here polls the page for a picture, which is the reason
//! for the driver on the host in the first place.
//!
//! The link is not the session. A link that ends leaves the browser and its page
//! running on that machine, so a panel that connects again attaches to the same
//! page, and the welcome of the link says whether it did.

use std::collections::VecDeque;
use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use horizon_browser::FrameSlot;
use horizon_browser::host_driver::{
    HostDriverFrame, HostDriverReply, HostDriverRequest, HostDriverWelcome, HostSystem, MAX_HOST_DRIVER_LINE_BYTES,
    read_line as read_wire_line,
};
use horizon_browser_control::ssh_forward::{Ssh, ssh_command_with_options, valid_profile, valid_target};

use super::HostDriverEngine;
use crate::browser::attached_host::{AttachedHostFailure, HOST_LINK_VERSION};

/// Bound of the first answer of the driver: the `ssh` connect, the
/// authentication and the start of the driver on that machine are in it. The
/// driver binds its socket before it starts the browser, so a cold browser of a
/// host is not part of this wait.
pub(super) const FIRST_ANSWER: Duration = Duration::from_secs(20);

/// Lines of `ssh` that a failure keeps, newest last, and the bound of one line.
const STDERR_TAIL_LINES: usize = 8;
const STDERR_LINE_CHARS: usize = 400;

/// Bound of the wait for the last words of `ssh` after its output ended.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// The exit code of a shell that found no program of that name.
const COMMAND_NOT_FOUND_EXIT: i32 = 127;

/// The exit code of `ssh` when the connection itself failed.
const SSH_CONNECTION_FAILURE: i32 = 255;

/// The page of a panel, as the link of that panel names it to the driver of its
/// browser host.
#[derive(Clone, Copy, Debug)]
pub struct LinkPage<'a> {
    /// The identity of the panel, which stays the same across a lost link and a
    /// restart, so the driver gives this panel its own page back.
    pub id: &'a str,
    /// Whether this page keeps nothing of the profile: a page of an isolated
    /// context of the same browser.
    pub private: bool,
}

/// The driver of a browser host: where it runs, and the program that this
/// computer starts there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDriverHost {
    ssh_target: String,
    profile: String,
    engine: HostDriverEngine,
    command: String,
}

/// The program that starts the driver of a browser host when the entry names
/// none.
pub const DEFAULT_HOST_DRIVER_COMMAND: &str = "horizon --browser-host-driver";

impl HostDriverHost {
    /// # Errors
    /// The SSH target must be one name that `ssh` takes as one argument, the
    /// profile must be one profile name of a browser host, and the command must
    /// be one line that the remote shell passes as this program alone.
    pub fn new(
        ssh_target: impl Into<String>,
        profile: impl Into<String>,
        engine: HostDriverEngine,
        command: Option<&str>,
    ) -> crate::error::Result<Self> {
        let ssh_target = ssh_target.into().trim().to_string();
        if !valid_target(&ssh_target) {
            return Err(crate::error::Error::Config(format!(
                "`{ssh_target}` is not one SSH name without a leading dash, a space or a control character"
            )));
        }
        let profile = profile.into().trim().to_string();
        if !valid_profile(&profile) {
            return Err(crate::error::Error::Config(format!(
                "`{profile}` is not one profile name of a browser host: 1 to 40 of a-z, 0-9 and '-', with a letter or a digit at each end"
            )));
        }
        let command = command
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .unwrap_or(DEFAULT_HOST_DRIVER_COMMAND)
            .to_string();
        if !crate::config::valid_driver_command(&command) {
            return Err(crate::error::Error::Config(format!(
                "`{command}` is not one command of a browser host driver: letters, digits and `._:/=@+-`, with single spaces between words"
            )));
        }
        Ok(Self {
            ssh_target,
            profile,
            engine,
            command,
        })
    }

    /// The SSH target of the host, as `ssh` resolves it.
    #[must_use]
    pub fn ssh_target(&self) -> &str {
        &self.ssh_target
    }

    /// The profile of the browser of that machine.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The engine that the driver of that machine runs.
    #[must_use]
    pub const fn engine(&self) -> HostDriverEngine {
        self.engine
    }

    /// The program that starts the driver of that machine, as the entry names
    /// it, when the entry names one.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        Some(&self.command)
    }

    /// The words that `ssh` runs on the host: the command of the entry with the
    /// profile and the engine of this panel.
    #[must_use]
    pub fn remote_command(&self) -> String {
        format!(
            "{} --profile {} --engine {}",
            self.command,
            self.profile,
            self.engine.as_str()
        )
    }

    /// The host as the failure vocabulary of a browser host names it.
    fn as_attached(&self) -> crate::browser::AttachedHost {
        crate::browser::AttachedHost::new(self.ssh_target.clone(), Some(&self.profile))
            .unwrap_or_else(|_| crate::browser::AttachedHost::for_words())
    }
}

impl std::fmt::Debug for DriverLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DriverLink")
            .field("profile", &self.welcome.profile)
            .field("engine", &self.welcome.engine)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// One message of the driver, as the reader thread hands it over.
enum IncomingLine {
    Line(Vec<u8>),
    TooLong { size: usize, limit: usize },
    Ended,
}

/// The live link to the driver of a browser host.
pub(super) struct DriverLink {
    child: Option<Child>,
    writer: Option<Box<dyn Write + Send>>,
    incoming: mpsc::Receiver<IncomingLine>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// The host of this link, for the words of a failure.
    attached: crate::browser::AttachedHost,
    /// Ends the socket of a link of a test that has no child of `ssh`.
    end_socket: Option<Box<dyn Fn() + Send>>,
    welcome: HostDriverWelcome,
    /// The page that the driver reported right after its welcome.
    initial_page: Option<horizon_browser::host_driver::HostDriverPage>,
    /// The frames this link acknowledged, so no frame is acknowledged twice.
    acknowledged: Option<u64>,
    /// The words of the end of this link, when it ended: what `ssh` said and
    /// its status. A reason that a person reads has words, also when `ssh`
    /// wrote none.
    end_words: Option<String>,
    /// Whether the driver greeted this link. A connection that never greeted
    /// and one that worked before it ended are different reasons for a person.
    greeted: bool,
    /// The largest wait of the pass that just ended, for a stall line.
    pass_wait: Option<(&'static str, Duration)>,
    frames: u64,
}

impl DriverLink {
    /// Start the driver of `host` over `ssh` and greet it.
    ///
    /// # Errors
    /// `ssh` cannot start or refuses the host (its own words are kept), the
    /// driver is not this program or another build of it, the driver runs on
    /// another system, or the driver refused the link in its words.
    /// Open the link of one panel: greet the driver, and name the page that this
    /// link drives.
    ///
    /// The page is the identity of the panel, so a link that comes back after a
    /// loss attaches to the page that the panel had rather than to the page of
    /// another panel. `private` asks for a page of an isolated context of that
    /// browser, which keeps nothing of the profile.
    pub(super) fn open(
        ssh: &Ssh,
        host: &HostDriverHost,
        quality: u32,
        page: &LinkPage<'_>,
    ) -> Result<Self, AttachedHostFailure> {
        let attached = host.as_attached();
        // The login options of a machine of the machine list stand in front of the
        // target, so a machine that Horizon rented answers with its own key.
        let mut command =
            ssh_command_with_options(ssh.program(), ssh.options(), host.ssh_target(), &host.remote_command());
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| AttachedHostFailure::unreachable(&attached, &error.to_string()))?;
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return Err(AttachedHostFailure::unreachable(
                &attached,
                "this computer could not read the words of its `ssh` child",
            ));
        };
        let incoming = read_lines(stdout);
        let stderr = read_stderr(stderr);
        let writer: Option<Box<dyn Write + Send>> = child.stdin.take().map(|stdin: ChildStdin| Box::new(stdin) as _);
        let mut link = Self {
            child: Some(child),
            writer,
            incoming,
            stderr,
            attached: attached.clone(),
            end_socket: None,
            welcome: HostDriverWelcome {
                version: HOST_LINK_VERSION,
                horizon_version: String::new(),
                system: HostSystem::here(),
                engine: host.engine().as_str().to_string(),
                profile: host.profile().to_string(),
                page: horizon_browser::host_driver::HostDriverPageState::New,
                width: 0,
                height: 0,
                quality,
            },
            initial_page: None,
            acknowledged: None,
            end_words: None,
            greeted: false,
            pass_wait: None,
            frames: 0,
        };
        link.handshake(host.engine(), quality, page)?;
        link.greeted = true;
        Ok(link)
    }

    /// Greet the driver and check what it says about itself.
    fn handshake(
        &mut self,
        engine: HostDriverEngine,
        quality: u32,
        page: &LinkPage<'_>,
    ) -> Result<(), AttachedHostFailure> {
        let attached = self.attached();
        let hello = HostDriverRequest::Hello {
            version: HOST_LINK_VERSION,
            horizon_version: env!("CARGO_PKG_VERSION").to_string(),
            system: HostSystem::here(),
            quality,
            page: page.id.to_string(),
            private: page.private,
        };
        if self.send(&hello).is_err() {
            // The program of the link can end before it takes the greet, and the
            // words of its end say why. The error of a write to a closed link says
            // nothing that the person can act on.
            return Err(self.ended());
        }
        let first = self
            .recv(FIRST_ANSWER)
            .map_err(|failure| failure.unwrap_or_else(|| AttachedHostFailure::closed(&attached)))?;
        match first {
            HostDriverReply::Welcome(welcome) => {
                check_welcome(&attached, &welcome, engine)?;
                self.welcome = welcome;
                // The driver sends the state of its page right after the
                // welcome, so the panel starts with the page that came back
                // rather than with a picture and no address.
                if let Ok(HostDriverReply::Page(page)) = self.recv(FIRST_ANSWER) {
                    self.initial_page = Some(page);
                }
                Ok(())
            }
            HostDriverReply::Refusal { code, message, .. } => {
                Err(AttachedHostFailure::refused(&attached, &code, &message))
            }
            other => Err(AttachedHostFailure::refused(
                &attached,
                "protocol_welcome",
                &format!("the driver answered {} instead of its welcome", reply_name(&other)),
            )),
        }
    }

    /// This end of a link that a test already opened, over a socket of this
    /// computer instead of a child of `ssh`.
    #[cfg(test)]
    pub(super) fn over_stream(
        stream: &std::os::unix::net::UnixStream,
        host: &HostDriverHost,
        quality: u32,
        page: &LinkPage<'_>,
    ) -> Result<Self, AttachedHostFailure> {
        let attached = host.as_attached();
        let reader = stream
            .try_clone()
            .map_err(|error| AttachedHostFailure::unreachable(&attached, &error.to_string()))?;
        let writer = stream
            .try_clone()
            .map_err(|error| AttachedHostFailure::unreachable(&attached, &error.to_string()))?;
        let mut link = Self {
            child: None,
            writer: Some(Box::new(writer)),
            incoming: read_lines(reader),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            attached,
            end_socket: stream.try_clone().ok().map(|stream| {
                Box::new(move || {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }) as Box<dyn Fn() + Send>
            }),
            welcome: HostDriverWelcome {
                version: HOST_LINK_VERSION,
                horizon_version: String::new(),
                system: HostSystem::here(),
                engine: host.engine().as_str().to_string(),
                profile: host.profile().to_string(),
                page: horizon_browser::host_driver::HostDriverPageState::New,
                width: 0,
                height: 0,
                quality,
            },
            initial_page: None,
            acknowledged: None,
            end_words: None,
            greeted: false,
            pass_wait: None,
            frames: 0,
        };
        link.handshake(host.engine(), quality, page)?;
        link.greeted = true;
        Ok(link)
    }

    /// Write one request of this computer.
    fn send(&mut self, request: &HostDriverRequest) -> Result<(), String> {
        let Some(writer) = self.writer.as_mut() else {
            return Err("the link of the browser host is closed".to_string());
        };
        let line = horizon_browser::host_driver::encode_line(request)
            .map_err(|error| format!("this computer could not write a request of the link: {error}"))?;
        let written_from = Instant::now();
        let written = writer.write_all(&line).and_then(|()| writer.flush());
        self.note_wait("write", written_from.elapsed());
        written.map_err(|error| format!("the link of the browser host could not take a request: {error}"))
    }

    /// Send one request, and answer the words of a failure.
    ///
    /// # Errors
    /// The link of the browser host ended or refused the request.
    pub(super) fn request(&mut self, request: &HostDriverRequest) -> Result<(), String> {
        self.send(request)
    }

    /// The next answer of the driver, or `None` when none arrived in `wait`.
    ///
    /// The error is `None` when the driver simply ended the link, which the
    /// panel reports as a lost link, and `Some` when the words of the failure
    /// are known.
    pub(super) fn recv(&mut self, wait: Duration) -> Result<HostDriverReply, Option<AttachedHostFailure>> {
        match self.incoming.recv_timeout(wait) {
            Ok(IncomingLine::Line(bytes)) => horizon_browser::host_driver::decode_reply(&bytes).map_err(|error| {
                Some(AttachedHostFailure::refused(
                    &self.attached(),
                    "protocol_message",
                    &error.to_string(),
                ))
            }),
            Ok(IncomingLine::TooLong { size, limit }) => Err(Some(AttachedHostFailure::refused(
                &self.attached(),
                "line_too_long",
                &format!("the driver wrote a message of {size} bytes, and a link carries at most {limit} bytes"),
            ))),
            Ok(IncomingLine::Ended) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(Some(self.ended())),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(None),
        }
    }

    /// The host of this link, for the words of a failure.
    pub(super) fn attached(&self) -> crate::browser::AttachedHost {
        self.attached.clone()
    }

    /// The words of a link that ended: those of `ssh` when it wrote any, and
    /// otherwise the plain sentence of a closed link with the status of `ssh`.
    fn ended(&mut self) -> AttachedHostFailure {
        let code = self.wait_for_exit();
        let tail = self.stderr_tail();
        self.close();
        if code == Some(COMMAND_NOT_FOUND_EXIT) {
            let reason = if tail.is_empty() {
                "the remote shell found no such program".to_string()
            } else {
                tail
            };
            self.end_words = Some(reason.clone());
            return AttachedHostFailure::no_driver(&self.attached(), &reason);
        }
        let words = if tail.is_empty() {
            match code {
                Some(code) => format!("the script of the link ended with status {code} and wrote no words"),
                None => "the script of the link ended and wrote no words".to_string(),
            }
        } else {
            tail
        };
        self.end_words = Some(words.clone());
        if code == Some(SSH_CONNECTION_FAILURE) && self.greeted {
            // The connection itself failed: a host that stopped answering, a
            // network that a laptop left, or a server that ended the session.
            // The words of `ssh` say nothing at `LogLevel=ERROR`, so this is
            // the sentence that the log and the panel keep.
            return AttachedHostFailure::connection_lost(&self.attached(), &words);
        }
        AttachedHostFailure::unreachable(&self.attached(), &words)
    }

    /// The words of the end of this link, when it ended.
    pub(super) fn end_words(&self) -> Option<&str> {
        self.end_words.as_deref()
    }

    fn wait_for_exit(&mut self) -> Option<i32> {
        let child = self.child.as_mut()?;
        let deadline = Instant::now() + EXIT_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) | Err(_) => return None,
            }
        }
    }

    fn stderr_tail(&self) -> String {
        // `ssh` writes its refusal on its standard error, and the reader thread
        // of that stream can be a pass behind the end of the output.
        let deadline = Instant::now() + EXIT_GRACE;
        while self.stderr_is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let tail = self
            .stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        tail.trim().to_string()
    }

    fn stderr_is_empty(&self) -> bool {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// The largest wait of the pass that just ended, and forget it.
    pub(super) fn take_pass_wait(&mut self) -> Option<(&'static str, Duration)> {
        self.pass_wait.take()
    }

    fn note_wait(&mut self, name: &'static str, waited: Duration) {
        if self.pass_wait.is_none_or(|(_, largest)| waited > largest) {
            self.pass_wait = Some((name, waited));
        }
    }

    /// The welcome that the driver gave this link.
    pub(super) fn welcome(&self) -> &HostDriverWelcome {
        &self.welcome
    }

    /// The page that the driver reported right after its welcome, when it did.
    pub(super) fn initial_page(&self) -> Option<&horizon_browser::host_driver::HostDriverPage> {
        self.initial_page.as_ref()
    }

    /// Decode one pushed picture into the frame slot of the panel and answer the
    /// acknowledgement that this computer owes the driver, once per frame.
    ///
    /// A frame that the slot cannot decode (a truncated or wrongly named
    /// picture) is acknowledged too: the driver must not wait for a picture that
    /// can never arrive, and the frame after it may be the one that shows.
    pub(super) fn accept_frame(&mut self, slot: &FrameSlot, frame: &HostDriverFrame) -> Option<HostDriverRequest> {
        if frame.format != horizon_browser::host_driver::HostDriverFrameFormat::Jpeg {
            tracing::warn!(
                format = frame.format.as_str(),
                "a frame of a browser host is not a JPEG"
            );
            return None;
        }
        let decoded = slot.store_base64_jpeg(&frame.data);
        if decoded != Some(frame.seq) {
            tracing::debug!(
                seq = frame.seq,
                decoded = ?decoded,
                "a frame of a browser host took a sequence of its own"
            );
        }
        self.frames = self.frames.saturating_add(1);
        self.acknowledge(frame.seq)
    }

    /// The acknowledgement of `seq`, unless this link already acknowledged it.
    fn acknowledge(&mut self, seq: u64) -> Option<HostDriverRequest> {
        if self.acknowledged.is_some_and(|last| last >= seq) {
            return None;
        }
        self.acknowledged = Some(seq);
        Some(HostDriverRequest::FrameAck { seq })
    }

    /// End this link. The session of the host keeps the browser and the page, so
    /// the next link attaches to the same page.
    pub(super) fn close(&mut self) {
        self.writer = None;
        if let Some(end) = self.end_socket.take() {
            end();
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for DriverLink {
    fn drop(&mut self) {
        self.close();
    }
}

/// Check the welcome of a driver against what this computer can use.
fn check_welcome(
    attached: &crate::browser::AttachedHost,
    welcome: &HostDriverWelcome,
    engine: HostDriverEngine,
) -> Result<(), AttachedHostFailure> {
    if welcome.version != HOST_LINK_VERSION || welcome.horizon_version != env!("CARGO_PKG_VERSION") {
        return Err(AttachedHostFailure::version_mismatch(
            attached,
            welcome.version,
            &welcome.horizon_version,
        ));
    }
    if let Some(words) = HostSystem::here().copy_refusal(&welcome.system) {
        return Err(AttachedHostFailure::other_system(attached, &words));
    }
    if welcome.engine != engine.as_str() {
        return Err(AttachedHostFailure::refused(
            attached,
            "protocol_engine",
            &format!(
                "the driver of that machine runs `{}`, and this panel asks for `{}`",
                welcome.engine,
                engine.as_str()
            ),
        ));
    }
    Ok(())
}

/// The name of one reply, for the words of a protocol failure.
fn reply_name(reply: &HostDriverReply) -> &'static str {
    match reply {
        HostDriverReply::Welcome(_) => "welcome",
        HostDriverReply::Frame(_) => "frame",
        HostDriverReply::Page(_) => "page",
        HostDriverReply::Action(_) => "action",
        HostDriverReply::Coordination(_) => "coordination",
        HostDriverReply::HandoffAck { .. } => "handoff acknowledgement",
        HostDriverReply::Audit(_) => "audit record",
        HostDriverReply::Refusal { .. } => "refusal",
        HostDriverReply::Closed => "end of the session",
    }
}

/// The standard output of `ssh`, read as lines of the wire.
fn read_lines(stdout: impl std::io::Read + Send + 'static) -> mpsc::Receiver<IncomingLine> {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("browser-host-driver-link".to_string())
        .spawn({
            let sender = sender.clone();
            move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let next = match read_wire_line(&mut reader, MAX_HOST_DRIVER_LINE_BYTES) {
                        Ok(Some(line)) => IncomingLine::Line(line),
                        Err(horizon_browser::host_driver::HostDriverLineError::TooLong { size, limit }) => {
                            IncomingLine::TooLong { size, limit }
                        }
                        // A closed link and a line that ended in the middle of a
                        // message are the same thing to a panel: the link is gone.
                        Ok(None) | Err(_) => IncomingLine::Ended,
                    };
                    let ended = matches!(next, IncomingLine::Ended | IncomingLine::TooLong { .. });
                    if sender.send(next).is_err() || ended {
                        return;
                    }
                }
            }
        });
    if spawned.is_err() {
        let _ = sender.send(IncomingLine::Ended);
    }
    receiver
}

/// The standard error of `ssh`, kept as a bounded tail of its lines.
fn read_stderr(stderr: impl std::io::Read + Send + 'static) -> Arc<Mutex<VecDeque<String>>> {
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let kept = Arc::clone(&tail);
    let spawned = std::thread::Builder::new()
        .name("browser-host-driver-words".to_string())
        .spawn(move || {
            use std::io::BufRead as _;
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else {
                    return;
                };
                let line: String = line.chars().take(STDERR_LINE_CHARS).collect();
                let mut kept = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if kept.len() >= STDERR_TAIL_LINES {
                    kept.pop_front();
                }
                kept.push_back(line);
            }
        });
    if spawned.is_err() {
        tracing::warn!("the words of a link of a browser host are not read");
    }
    tail
}
