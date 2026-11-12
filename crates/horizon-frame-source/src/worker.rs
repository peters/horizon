//! The child of one session: its socket, its three streams and its exact process
//! id.
//!
//! [`run`] starts the program of the settings with the picture socket as descriptor
//! 3 and reads it on threads of its own, and it is the one place that stops that
//! program. The stop is ordered: `bye`, then `SIGTERM` after [`STOP_WAIT`], then
//! `SIGKILL` after another [`STOP_WAIT`], and every signal goes to the process id
//! of this child and to no process group.
//!
//! The program cannot outlive this process either. Horizon holds the write end of
//! the standard input of the program and the read end of its picture socket, and
//! the kernel closes both when this process ends, whatever ends it. A program that
//! reads its socket then ends by itself, which is what its contract says.
use std::{
    io::Read,
    os::fd::OwnedFd,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        mpsc::{Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use command_fds::{CommandFdExt, FdMapping};
use horizon_vnc::Status;
use rustix::process::Signal;

use crate::{
    Error,
    control::{self, Message, Outgoing, Phase},
    picture,
    session::{Config, Event, IDLE_POLL, MAX_LABEL_CHARS, POLL, STOP_WAIT, Shared, ViewQueue},
    text,
};

/// The file descriptor of the picture socket in the child.
pub const PICTURE_FD: i32 = 3;

/// The largest standard error of one program that is kept, and the last line with
/// it. A program that prints more has its last line as the news.
const MAX_STDERR_BYTES: u64 = 64 * 1024;

/// How long the reader of the pictures waits for a frame that did not take the last
/// picture, so the picture that waits is converted for the next frame.
const TAKEN_WAIT: Duration = Duration::from_millis(30);

/// Run one viewer until its program ends, its reader ends, or the UI stops it.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the worker of one viewer owns its configuration, its shared state and its own end of the queue"
)]
pub(crate) fn run(
    config: Config,
    shared: Arc<Shared>,
    sender: Sender<Event>,
    events: Receiver<Event>,
) -> Result<(), Error> {
    let (mut process, socket) = start(&config)?;
    let mut input = process.stdin.take();
    let stdout = process.stdout.take();
    let stderr = process.stderr.take();

    let pictures = {
        let shared = Arc::clone(&shared);
        let sender = sender.clone();
        spawn("frame-source-pictures", move || {
            let fault = read_pictures(socket, &shared);
            let _ = sender.send(Event::PicturesEnded(fault));
        })?
    };
    let control = match stdout {
        Some(stdout) => {
            let sender = sender.clone();
            Some(spawn("frame-source-control", move || {
                let fault = read_control(stdout, &sender);
                let _ = sender.send(Event::ControlEnded(fault));
            })?)
        }
        None => None,
    };
    let errors = match stderr {
        Some(stderr) => {
            let sender = sender.clone();
            Some(spawn("frame-source-stderr", move || {
                if let Some(line) = last_line(stderr) {
                    let _ = sender.send(Event::Stderr(line));
                }
            })?)
        }
        None => None,
    };

    let end = supervise(&config, &shared, &events, &mut process, &mut input);
    // `bye` and the signals are the whole of the stop: the threads end with it.
    stop(&config, &mut process, &mut input);
    let _ = pictures.join();
    if let Some(thread) = control {
        let _ = thread.join();
    }
    if let Some(thread) = errors {
        let _ = thread.join();
    }
    shared.publish_status(end.unwrap_or_else(|| end_of_itself(&config, &shared)));
    Ok(())
}

/// One thread of one viewer.
fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> Result<thread::JoinHandle<()>, Error> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map_err(|error| Error::NotStarted(error.to_string()))
}

/// The events of one viewer until it ends, and the status that ended it.
fn supervise(
    config: &Config,
    shared: &Shared,
    events: &Receiver<Event>,
    child: &mut Child,
    stdin: &mut Option<ChildStdin>,
) -> Option<Status> {
    let mut viewer = Viewer {
        config,
        shared,
        views: ViewQueue::default(),
        sent_visible: None,
    };
    loop {
        let now = Instant::now();
        let wait = viewer.views.deadline(now).map_or(IDLE_POLL, |left| left.min(IDLE_POLL));
        match events.recv_timeout(wait) {
            Ok(event) => {
                if let Some(status) = viewer.handle(event, stdin.as_mut().map(|stdin| &mut *stdin)) {
                    return Some(status);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Some(Status::Stopped),
        }
        if let Some(size) = viewer.views.due(Instant::now()) {
            let size = viewer.clamp(size);
            write(
                stdin.as_mut().map(|stdin| &mut *stdin),
                &control::view(Some(size[0]), Some(size[1]), None),
            );
        }
        // A program that ended by itself is the end of the viewer, whatever its
        // threads say, and its own words are the reason. The messages that it wrote
        // before its end are read first, so a program that answers one word and then
        // exits keeps the word in the sentence of the panel.
        if child.try_wait().ok().flatten().is_some() {
            while let Ok(event) = events.try_recv() {
                if let Some(status) = viewer.handle(event, stdin.as_mut().map(|stdin| &mut *stdin)) {
                    return Some(status);
                }
            }
            return None;
        }
    }
}

/// One viewer and what it has told its program.
struct Viewer<'a> {
    config: &'a Config,
    shared: &'a Shared,
    views: ViewQueue,
    sent_visible: Option<bool>,
}

impl Viewer<'_> {
    /// Apply one event, and answer the status that ends the viewer.
    fn handle(&mut self, event: Event, stdin: Option<&mut ChildStdin>) -> Option<Status> {
        match event {
            Event::Send(message) => write(stdin, &message),
            Event::Size(size) => self.views.offer(size, Instant::now()),
            Event::Visible(visible) => {
                if self.sent_visible == Some(visible) {
                    return None;
                }
                self.sent_visible = Some(visible);
                self.shared.lock().visible = visible;
                write(stdin, &control::view(None, None, Some(visible)));
            }
            Event::Message(message) => return self.message(message),
            Event::PicturesEnded(fault) => {
                return Some(match fault {
                    Some(fault) => Status::disconnected(fault.to_string()),
                    None => Status::disconnected(format!("{} closed its picture socket", self.label())),
                });
            }
            Event::ControlEnded(Some(fault)) => return Some(Status::disconnected(fault.to_string())),
            Event::ControlEnded(None) => {}
            Event::Stderr(line) => self.shared.lock().last_stderr = Some(line),
            Event::Stop => return Some(Status::Stopped),
        }
        None
    }

    /// Apply one message of the program.
    fn message(&self, message: Message) -> Option<Status> {
        match message {
            Message::Hello(hello) => {
                let reason = text::label(hello.input_reason.as_deref(), MAX_LABEL_CHARS);
                let status = {
                    let mut state = self.shared.lock();
                    state.server_name = Some(hello.source);
                    state.hello = Some(crate::session::InputHello {
                        allowed: hello.input,
                        reason,
                    });
                    Status::Connected
                };
                self.shared.publish_status(status);
            }
            Message::Stream(size) => {
                let mut state = self.shared.lock();
                // The source is the size that the program captures, and the widget
                // never asks for more than that.
                state.source_size = Some([size.source_width as usize, size.source_height as usize]);
                state.desktop = state.desktop.or(Some([size.width as usize, size.height as usize]));
            }
            Message::State(state) => {
                if state.phase == Phase::Ended {
                    return Some(Status::disconnected(
                        text::label(state.reason.as_deref(), MAX_LABEL_CHARS)
                            .unwrap_or_else(|| format!("{} ended", self.label())),
                    ));
                }
            }
            Message::Stats(stats) => self.shared.lock().stats = Some(stats),
            Message::Ok { .. } | Message::Ignored => {}
            Message::Refused { code, message, .. } => {
                if code == "busy" {
                    return Some(Status::disconnected(
                        text::label(Some(&message), MAX_LABEL_CHARS)
                            .unwrap_or_else(|| format!("{} serves another viewer", self.label())),
                    ));
                }
                self.shared.lock().input_error = Some(message);
            }
        }
        None
    }

    /// The size to ask for: the size that the viewer draws at, at most the size of
    /// the source.
    fn clamp(&self, size: [u32; 2]) -> [u32; 2] {
        let Some(source) = self.shared.lock().source_size else {
            return size;
        };
        [
            size[0].min(u32::try_from(source[0]).unwrap_or(u32::MAX)).max(1),
            size[1].min(u32::try_from(source[1]).unwrap_or(u32::MAX)).max(1),
        ]
    }

    fn label(&self) -> String {
        if self.config.name.is_empty() {
            self.config.program.display().to_string()
        } else {
            self.config.name.clone()
        }
    }
}

fn write(stdin: Option<&mut ChildStdin>, message: &Outgoing) {
    let Some(stdin) = stdin else {
        return;
    };
    if let Err(error) = control::write(stdin, message) {
        tracing::warn!(%error, "a frame source program did not read one message");
    }
}

/// The pictures of the socket, until the socket ends or one header breaks a bound.
fn read_pictures(mut socket: std::os::unix::net::UnixStream, shared: &Shared) -> Option<Error> {
    loop {
        match picture::read(&mut socket) {
            Ok(Some(picture)) => {
                if shared.publish_picture(picture) {
                    // The frame did not take the last picture, so this one waits
                    // rather than being converted for nobody. The wait is bounded:
                    // the next picture replaces it all the same.
                    shared.wait_for_taken(TAKEN_WAIT);
                    if let Err(error) = shared.convert_waiting() {
                        return Some(error);
                    }
                }
            }
            Ok(None) => return None,
            Err(error) => return Some(error),
        }
    }
}

/// The control messages of the standard output, until the program closes it.
fn read_control(stdout: impl std::io::Read, sender: &Sender<Event>) -> Option<Error> {
    let mut stdout = std::io::BufReader::new(stdout);
    loop {
        match control::read(&mut stdout) {
            Ok(Some(message)) => {
                if sender.send(Event::Message(message)).is_err() {
                    return None;
                }
            }
            Ok(None) => return None,
            Err(error) => return Some(error),
        }
    }
}

/// The last line that the program wrote on its standard error.
fn last_line(stderr: impl Read) -> Option<String> {
    let mut bytes = Vec::new();
    stderr.take(MAX_STDERR_BYTES).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

/// Start the program with the picture socket as its descriptor 3.
fn start(config: &Config) -> Result<(Child, std::os::unix::net::UnixStream), Error> {
    let (socket, child_socket) = std::os::unix::net::UnixStream::pair()?;
    let mut command = Command::new(&config.program);
    command
        .args(&config.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let fd: OwnedFd = child_socket.into();
    command
        .fd_mappings(vec![FdMapping {
            parent_fd: fd,
            child_fd: PICTURE_FD,
        }])
        .map_err(|error| Error::NotStarted(error.to_string()))?;
    match command.spawn() {
        Ok(process) => Ok((process, socket)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::NotInstalled(config.program.display().to_string()))
        }
        Err(error) => Err(Error::NotStarted(format!("{}: {error}", config.program.display()))),
    }
}

/// Stop the exact program of one viewer: `bye`, then `SIGTERM`, then `SIGKILL`.
fn stop(config: &Config, child: &mut Child, input: &mut Option<ChildStdin>) {
    // `bye` asks the program to end with 0. The standard input is closed after the
    // message, so a program that reads its input to its end ends as well.
    if let Some(stdin) = input.as_mut() {
        let _ = control::write(stdin, &Outgoing::Bye);
    }
    drop(input.take());
    if wait_for_exit(child, STOP_WAIT) {
        return;
    }
    if let Ok(raw) = i32::try_from(child.id())
        && let Some(pid) = rustix::process::Pid::from_raw(raw)
    {
        let _ = rustix::process::kill_process(pid, Signal::TERM);
    }
    if wait_for_exit(child, STOP_WAIT) {
        return;
    }
    tracing::warn!(program = %config.program.display(), "a frame source program ignored SIGTERM");
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether the program ended within `bound`.
fn wait_for_exit(child: &mut Child, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// The status of a program that ended by itself: its own last line, or one
/// sentence.
fn end_of_itself(config: &Config, shared: &Shared) -> Status {
    let last = shared.lock().last_stderr.clone();
    let label = if config.name.is_empty() {
        config.program.display().to_string()
    } else {
        config.name.clone()
    };
    match text::label(last.as_deref(), MAX_LABEL_CHARS) {
        Some(line) => Status::disconnected(format!("{label} ended: {line}")),
        None => Status::disconnected(format!("{label} ended")),
    }
}
