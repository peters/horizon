//! One take of speech input: the recorder while the hotkey is down, and the
//! transcription after it.
//!
//! The window sends a key event and reads an event; every wait of a take runs on
//! the worker thread of this module. One take is:
//!
//! 1. Make `<Horizon home>/speech/` (`0700`) and one file `take-<random>.wav`
//!    (`0600`), after every file that an earlier run left there is deleted.
//! 2. Start the recorder of the settings as the exact child of this process. A
//!    program that no folder of the environment holds is one sentence for the
//!    person, and no take starts.
//! 3. Read the key events and the bound of 120 seconds while the child records.
//! 4. Stop the child with `SIGINT` to its own process id, and with `SIGTERM` to
//!    that id when it still runs after 0.3 seconds: a recorder that this process
//!    started from a script with `&`, from `nohup`, or from a launcher that ignores
//!    the interrupt signal keeps that disposition across `exec`, so `SIGINT` alone
//!    never reaches it. After 2 seconds the id is killed. A take shorter than 0.3
//!    seconds, or a file with no byte, is dropped and no request runs.
//! 5. Transcribe with a bound of 30 seconds, through a service or through one
//!    command of the settings. The file is deleted after every outcome.
//!
//! The value of the named secret of the key is read on this thread at the moment
//! of the request, and it reaches one header of that request and no other byte of
//! this process.

use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::Duration;

use rustix::process::Signal;
use zeroize::Zeroizing;

use super::clock::{Clock, SystemClock};
use super::command::CommandLine;
use super::endpoint::Endpoint;
use super::service::{self, ServiceRequest};
use super::text::clean_text;
use crate::bounded_program::{ProgramEnvironment, ProgramFailure, ProgramStart, StartedProgram, run};
use crate::named_secrets::{NamedSecretError, NamedSecretVault, SecretName};
use crate::panel::PanelId;

/// The most bytes of one recording. A take above it is refused.
pub const MAX_TAKE_BYTES: u64 = 25 * 1024 * 1024;

/// The most characters of one message of a service or of a recorder that Horizon
/// keeps.
pub const MAX_MESSAGE_CHARS: usize = 200;

/// The name of the folder of the takes inside a Horizon home.
pub const TAKE_FOLDER: &str = "speech";

/// The most time that the stop of a recorder is given before it is killed.
pub const STOP_WAIT: Duration = Duration::from_secs(2);

/// How long the stop of a recorder waits after `SIGINT` before it sends
/// `SIGTERM`. The wait runs on the clock of this process, so a test reaches it by
/// waiting and never by moving its own clock.
///
/// A recorder that inherited "ignore `SIGINT`" — the disposition of a background
/// job of a non-interactive shell, of `nohup`, or of a launcher — cannot trap the
/// signal, so `SIGINT` is never seen by it. `SIGTERM` is not ignored in that
/// situation, and every recorder that the settings name closes its file on it.
pub const STOP_STEP: Duration = Duration::from_millis(300);

/// The most time that the upload of a take of `bytes` may take: the seconds of
/// that file at [`MIN_UPLOAD_RATE`], and at least the bound of the answer. A
/// long dictation on a slow uplink is then not cut in the middle of the way up.
#[must_use]
pub fn upload_bound(bytes: u64, answer: Duration) -> Duration {
    Duration::from_secs(bytes.div_ceil(MIN_UPLOAD_RATE)).max(answer)
}

/// The most time that one take records.
pub const MAX_TAKE: Duration = Duration::from_secs(120);

/// The shortest take that a transcriber runs for.
pub const MIN_TAKE: Duration = Duration::from_millis(300);

/// The most time that Horizon waits for the answer of one transcription.
pub const TRANSCRIBE_BOUND: Duration = Duration::from_secs(30);

/// The slowest uplink that a take waits for, in bytes of the file per second:
/// 128 kbit/s. The bound of an upload is the seconds of the file at this rate,
/// and never less than [`TRANSCRIBE_BOUND`].
pub const MIN_UPLOAD_RATE: u64 = 16 * 1024;

/// How many kept takes the folder holds. A take that could not be sent stays for
/// one retry, and the folder never grows past the last three.
pub const KEPT_TAKES: usize = 3;

/// How long a kept take stays in the folder.
pub const KEPT_TAKE_AGE: Duration = Duration::from_hours(24);

/// How often a take looks at its key events and at its child.
pub const POLL: Duration = Duration::from_millis(10);

/// The transcriber of one take: a service over HTTPS, or one command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transcriber {
    Service {
        endpoint: Endpoint,
        model: String,
        /// The language hint, such as `no`, or `None` when the settings leave it
        /// out.
        language: Option<String>,
        key_secret: SecretName,
    },
    Command(CommandLine),
}

/// One take, as the settings of the person describe it when the hotkey goes down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TakePlan {
    pub recorder: CommandLine,
    pub transcriber: Transcriber,
}

/// What one take is bounded by. A test gives shorter bounds, and the code path of a
/// bound stays the same.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TakeBounds {
    pub stop_wait: Duration,
    pub stop_step: Duration,
    pub max_take: Duration,
    pub min_take: Duration,
    pub transcribe: Duration,
    pub max_file_bytes: u64,
    pub poll: Duration,
}

impl Default for TakeBounds {
    fn default() -> Self {
        Self {
            stop_wait: STOP_WAIT,
            stop_step: STOP_STEP,
            max_take: MAX_TAKE,
            min_take: MIN_TAKE,
            transcribe: TRANSCRIBE_BOUND,
            max_file_bytes: MAX_TAKE_BYTES,
            poll: POLL,
        }
    }
}

/// Where one take reads the value of its named secret.
///
/// The value is read on the worker thread of the take, at the moment of the
/// request, and the caller hands it only to one `Authorization` header.
pub trait SpeechKeyStore: Send + Sync + std::fmt::Debug {
    /// The value of `name`, or `None` when this computer holds none.
    ///
    /// # Errors
    /// Reports why the secrets file of this computer cannot be read.
    fn value(&self, name: &SecretName) -> Result<Option<Zeroizing<String>>, String>;
}

impl SpeechKeyStore for NamedSecretVault {
    fn value(&self, name: &SecretName) -> Result<Option<Zeroizing<String>>, String> {
        let mut bytes = NamedSecretVault::value(self, name).map_err(|error| secret_error_text(&error))?;
        let Some(mut bytes) = bytes.take() else {
            return Ok(None);
        };
        let string = String::from_utf8(std::mem::take(&mut *bytes))
            .map_err(|_| "The value of a named secret of this computer is not text".to_owned())?;
        Ok(Some(Zeroizing::new(string)))
    }
}

impl SpeechKeyStore for () {
    fn value(&self, _name: &SecretName) -> Result<Option<Zeroizing<String>>, String> {
        Ok(None)
    }
}

/// One sentence about a secrets file that could not be read. It names the secret
/// and never quotes a value.
fn secret_error_text(error: &NamedSecretError) -> String {
    format!("The secrets of this computer cannot be read: {error}")
}

/// What one take tells the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechEvent {
    pub target: PanelId,
    pub kind: SpeechEventKind,
}

/// The state of one take that the window draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechEventKind {
    /// The recorder of the take runs.
    Listening,
    /// The take is transcribed, and no recorder runs any more.
    Writing,
    /// The text of the take, cleaned and ready to insert in `target`.
    Text(String),
    /// One sentence for the person.
    Failed(String),
    /// The take could not be sent and its file stays on this computer, with the
    /// sentence that says why. The window offers one retry of that file.
    Kept(String),
    /// The take ended with nothing to insert and nothing to show.
    Dropped,
}

/// What the worker thread of speech input needs.
pub struct SpeechWorker {
    /// The root of the Horizon home. The takes live in its [`TAKE_FOLDER`].
    pub home: PathBuf,
    pub bounds: TakeBounds,
    pub clock: Arc<dyn Clock>,
    /// The named secrets of this computer, which the take reads on its own thread.
    pub keys: Arc<dyn SpeechKeyStore>,
    /// Called after every event, so a window applies the answer in its next frame
    /// instead of holding a frame open for it.
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl SpeechWorker {
    /// A worker of this process: the system clock and the given home and secrets.
    #[must_use]
    pub fn new(home: PathBuf, keys: Arc<dyn SpeechKeyStore>) -> Self {
        Self {
            home,
            bounds: TakeBounds::default(),
            clock: Arc::new(SystemClock::new()),
            keys,
            wake: None,
        }
    }

    /// The worker wakes `wake` after every event of a take.
    #[must_use]
    pub fn with_wake(mut self, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.wake = Some(wake);
        self
    }

    /// A worker with the clock and the bounds of a test.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>, bounds: TakeBounds) -> Self {
        self.clock = clock;
        self.bounds = bounds;
        self
    }
}

enum Request {
    /// One take of `plan` into `target`.
    Begin { plan: Box<TakePlan>, target: PanelId },
    /// The hotkey came up.
    End,
    /// Send the last kept take of `plan` again, without recording.
    Retry { plan: Box<TakePlan>, target: PanelId },
    /// `Escape` or the end of the window.
    Cancel,
    /// The window closes.
    Stop,
}

/// The handle of the window to the worker thread of speech input.
///
/// The thread ends when this handle is dropped. A take that runs then is cancelled,
/// so its recorder child is signalled and its file is deleted before the thread
/// ends.
pub struct SpeechInput {
    requests: Sender<Request>,
    events: Receiver<SpeechEvent>,
}

impl SpeechInput {
    /// Start the worker thread of `worker`.
    #[must_use]
    pub fn start(worker: SpeechWorker) -> Self {
        let (requests, request_rx) = channel();
        let (event_tx, events) = channel();
        let spawned = thread::Builder::new()
            .name("horizon-speech".to_owned())
            .spawn(move || Worker::new(worker, request_rx, event_tx).run());
        if let Err(error) = spawned {
            tracing::warn!(%error, "the thread of speech input did not start");
        }
        Self { requests, events }
    }

    /// Start one take of `plan` into `target`.
    pub fn begin(&self, plan: TakePlan, target: PanelId) {
        let _ = self.requests.send(Request::Begin {
            plan: Box::new(plan),
            target,
        });
    }

    /// The hotkey came up: transcribe the take that runs.
    pub fn end(&self) {
        let _ = self.requests.send(Request::End);
    }

    /// Send the last take that stayed in the folder again, without a recorder.
    pub fn retry(&self, plan: TakePlan, target: PanelId) {
        let _ = self.requests.send(Request::Retry {
            plan: Box::new(plan),
            target,
        });
    }

    /// `Escape`, or the end of the window: drop the take that runs.
    pub fn cancel(&self) {
        let _ = self.requests.send(Request::Cancel);
    }

    /// One event of the worker, or `None` while it has none.
    #[must_use]
    pub fn poll(&self) -> Option<SpeechEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for SpeechInput {
    fn drop(&mut self) {
        // The thread cancels a take that runs, so its recorder is signalled and its
        // file is deleted. No frame waits for it: the thread ends on its own, and a
        // transcription that is in flight ends at its bound.
        let _ = self.requests.send(Request::Stop);
    }
}

enum RecordEnd {
    KeyUp,
    Cancelled,
    Maxed,
    EndedEarly,
}

/// The state of the worker thread.
struct Worker {
    settings: SpeechWorker,
    requests: Receiver<Request>,
    events: Sender<SpeechEvent>,
}

impl Worker {
    fn new(settings: SpeechWorker, requests: Receiver<Request>, events: Sender<SpeechEvent>) -> Self {
        Self {
            settings,
            requests,
            events,
        }
    }

    fn run(mut self) {
        self.sweep_takes();
        loop {
            match self.requests.recv() {
                Ok(Request::Begin { plan, target }) => self.take(&plan, target),
                Ok(Request::Retry { plan, target }) => self.retry_kept(&plan, target),
                Ok(Request::End | Request::Cancel) => {}
                Ok(Request::Stop) | Err(_) => break,
            }
        }
    }

    fn take(&mut self, plan: &TakePlan, target: PanelId) {
        // The clock is read before the file exists, so a file in the folder means
        // that the length of the take is already running.
        let started = self.settings.clock.now_ms();
        let Some(file) = self.new_take_file() else {
            self.failed(target, "The folder of the takes in the Horizon home cannot be made");
            return;
        };
        let program = plan.recorder.program().to_owned();
        let args = plan.recorder.arguments_for(&file);
        let mut child = match StartedProgram::start(Path::new(&program), &args, &ProgramEnvironment::inherited()) {
            Ok(child) => child,
            Err(ProgramStart::NotInstalled) => {
                remove_take_file(&file);
                self.failed(target, &format!("{program} is not installed on this computer"));
                return;
            }
            Err(ProgramStart::Failed) => {
                remove_take_file(&file);
                self.failed(target, &format!("{program} could not be started"));
                return;
            }
        };
        self.send(target, SpeechEventKind::Listening);
        let end = self.record(&mut child, started);
        self.stop(&mut child);
        let elapsed = self.settings.clock.now_ms().saturating_sub(started);
        if matches!(end, RecordEnd::Cancelled) {
            remove_take_file(&file);
            self.send(target, SpeechEventKind::Dropped);
            return;
        }
        if matches!(end, RecordEnd::EndedEarly) {
            let message = match child.last_stderr_line() {
                Some(line) => format!("{program} ended early: {}", cut_message(&line)),
                None => format!("{program} ended early"),
            };
            remove_take_file(&file);
            self.failed(target, &message);
            return;
        }
        let bytes = take_bytes(&file);
        if bytes == 0 || elapsed < millis(self.settings.bounds.min_take) {
            remove_take_file(&file);
            self.send(target, SpeechEventKind::Dropped);
            return;
        }
        if bytes > self.settings.bounds.max_file_bytes {
            remove_take_file(&file);
            let megabytes = (self.settings.bounds.max_file_bytes / (1024 * 1024)).max(1);
            self.failed(
                target,
                &format!("The recording of the take is larger than {megabytes} MB"),
            );
            return;
        }
        if let Err(error) = close_take_header(&file, bytes) {
            tracing::warn!(path = %file.display(), %error, "the header of a take stays open");
        }
        self.send(target, SpeechEventKind::Writing);
        self.transcribe_take(plan, target, &file);
    }

    /// Transcribe `file` and report its text, or keep the file when the failure
    /// says that the same request can work later.
    fn transcribe_take(&mut self, plan: &TakePlan, target: PanelId, file: &Path) {
        let file_name = file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        match self.transcribe(plan, file, &file_name) {
            Ok(text) => {
                remove_take_file(file);
                let text = clean_text(&text);
                if text.is_empty() {
                    self.failed(target, "The take held no text");
                } else {
                    self.send(target, SpeechEventKind::Text(text));
                }
            }
            Err(failure) if failure.kept => {
                // The person's speech stays on this computer, private, for one
                // retry: a slow uplink or a service that is away is no reason to
                // lose a dictation.
                self.sweep_takes();
                tracing::info!(
                    path = %file.display(),
                    "a take that could not be sent stays for a retry"
                );
                self.send(target, SpeechEventKind::Kept(failure.message));
            }
            Err(failure) => {
                remove_take_file(file);
                self.failed(target, &failure.message);
            }
        }
    }

    /// Transcribe the newest take that stayed in the folder, without a recorder.
    fn retry_kept(&mut self, plan: &TakePlan, target: PanelId) {
        let Some(file) = self.newest_kept_take() else {
            self.failed(target, "There is no kept take to send again");
            return;
        };
        if take_bytes(&file) > self.settings.bounds.max_file_bytes {
            remove_take_file(&file);
            self.failed(target, "The kept take is larger than a take may be");
            return;
        }
        self.send(target, SpeechEventKind::Writing);
        self.transcribe_take(plan, target, &file);
    }

    /// The newest take that an earlier failure kept, when one is there.
    fn newest_kept_take(&self) -> Option<PathBuf> {
        crate::ui_thread::blocking("the folder of the takes is read");
        let folder = self.settings.home.join(TAKE_FOLDER);
        let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in fs::read_dir(&folder).ok()?.flatten() {
            let path = entry.path();
            if !is_take_file(&path) {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            if newest.as_ref().is_none_or(|(at, _)| modified > *at) {
                newest = Some((modified, path));
            }
        }
        newest.map(|(_, path)| path)
    }

    /// Read the key events and the child until the take ends.
    fn record(&self, child: &mut StartedProgram, started_ms: u64) -> RecordEnd {
        loop {
            match self.requests.try_recv() {
                Ok(Request::End) => return RecordEnd::KeyUp,
                // One take runs at a time: a second press changes nothing.
                Ok(Request::Begin { .. } | Request::Retry { .. }) | Err(TryRecvError::Empty) => {}
                Ok(Request::Cancel | Request::Stop) | Err(TryRecvError::Disconnected) => {
                    return RecordEnd::Cancelled;
                }
            }
            if child.try_wait().is_some() {
                return RecordEnd::EndedEarly;
            }
            if self.settings.clock.now_ms().saturating_sub(started_ms) >= millis(self.settings.bounds.max_take) {
                return RecordEnd::Maxed;
            }
            self.settings.clock.sleep(self.settings.bounds.poll);
        }
    }

    /// Stop the recorder: `SIGINT` to its own process id, `SIGTERM` to that id
    /// when it still runs after [`STOP_STEP`], and `SIGKILL` when its bound passes.
    ///
    /// Each signal goes to the exact process id of the recorder and to no group. A
    /// recorder that ignores the interrupt signal ends on the termination signal;
    /// one that ignores both ends by the kill, so the stop is never longer than
    /// [`STOP_WAIT`].
    ///
    /// The step before the termination signal runs on the clock of this process and
    /// not on the clock of a take: it is the time that a real recorder is given to
    /// close its file after `SIGINT`, so a test reaches it by waiting and never by
    /// moving its own clock. The two bounds of the stop stay on the clock of the
    /// take.
    fn stop(&self, child: &mut StartedProgram) {
        if child.try_wait().is_some() {
            return;
        }
        let now = self.settings.clock.now_ms();
        // A step that is longer than the whole stop would keep the termination
        // signal out of it, so the step never passes the bound.
        let step_at = std::time::Instant::now() + self.settings.bounds.stop_step.min(self.settings.bounds.stop_wait);
        let deadline = now.saturating_add(millis(self.settings.bounds.stop_wait));
        if !child.signal(Signal::INT) {
            child.kill();
            return;
        }
        let mut asked_to_terminate = false;
        loop {
            if child.try_wait().is_some() {
                return;
            }
            if self.settings.clock.now_ms() >= deadline {
                break;
            }
            if !asked_to_terminate && std::time::Instant::now() >= step_at {
                asked_to_terminate = true;
                if !child.signal(Signal::TERM) {
                    break;
                }
            }
            self.settings.clock.sleep(self.settings.bounds.poll);
        }
        // The recorder ignored both signals, so this id alone is killed and reaped.
        child.kill();
    }

    fn transcribe(&self, plan: &TakePlan, file: &Path, file_name: &str) -> Result<String, TakeFailure> {
        crate::ui_thread::blocking("one take is transcribed");
        match &plan.transcriber {
            Transcriber::Command(command) => self.transcribe_by_command(command, file),
            Transcriber::Service {
                endpoint,
                model,
                language,
                key_secret,
            } => {
                let key = self
                    .settings
                    .keys
                    .value(key_secret)
                    .map_err(|error| TakeFailure::dropped(cut_message(&error)))?;
                let Some(key) = key else {
                    return Err(TakeFailure::dropped(format!(
                        "No secret named {key_secret} is on this computer"
                    )));
                };
                let bytes = read_take_file(file).map_err(TakeFailure::dropped)?;
                let upload = upload_bound(bytes.len() as u64, self.settings.bounds.transcribe);
                let request = ServiceRequest {
                    endpoint,
                    model,
                    language: language.as_deref(),
                    file_name,
                    file: &bytes,
                    key: key.as_str(),
                    bound: self.settings.bounds.transcribe,
                    upload,
                };
                service::transcribe(&request).map_err(|error| TakeFailure {
                    message: error.to_string(),
                    kept: error.keeps_the_take(),
                })
            }
        }
    }

    fn transcribe_by_command(&self, command: &CommandLine, file: &Path) -> Result<String, TakeFailure> {
        let args = command.arguments_for(file);
        match run(
            Path::new(command.program()),
            &args,
            &ProgramEnvironment::inherited(),
            self.settings.bounds.transcribe,
        ) {
            Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            // A command that a slow machine did not answer in time says nothing
            // about the take, so the file stays for a retry; a command that ran
            // and refused does not.
            Err(ProgramFailure::TimedOut) => Err(TakeFailure {
                message: format!(
                    "{} did not answer in {} seconds",
                    command.program(),
                    self.settings.bounds.transcribe.as_secs()
                ),
                kept: true,
            }),
            Err(ProgramFailure::Failed) => Err(TakeFailure::dropped(format!("{} did not answer", command.program()))),
        }
    }

    /// Delete the takes that the folder may not hold: every take older than
    /// [`KEPT_TAKE_AGE`], and every take past the newest [`KEPT_TAKES`].
    ///
    /// A take that could not be sent stays for one retry, so the folder holds at
    /// most three private recordings for a day. A start of the window sweeps too.
    fn sweep_takes(&self) {
        crate::ui_thread::blocking("the folder of the takes is swept");
        let folder = self.settings.home.join(TAKE_FOLDER);
        let Ok(entries) = fs::read_dir(&folder) else {
            return;
        };
        let now = std::time::SystemTime::now();
        let mut takes: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_take_file(&path) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(now);
            let age = now.duration_since(modified).unwrap_or_default();
            if age > KEPT_TAKE_AGE {
                remove_take_file(&path);
                continue;
            }
            takes.push((modified, path));
        }
        takes.sort_by_key(|take| std::cmp::Reverse(take.0));
        for (_, path) in takes.into_iter().skip(KEPT_TAKES) {
            remove_take_file(&path);
        }
    }

    /// Make the folder of the takes and one empty file of this take.
    fn new_take_file(&self) -> Option<PathBuf> {
        crate::ui_thread::blocking("the folder of the takes is made");
        let folder = self.settings.home.join(TAKE_FOLDER);
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder).ok()?;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).ok()?;
        let name = format!("take-{}.wav", uuid::Uuid::new_v4().simple());
        let path = folder.join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        Some(path)
    }

    fn send(&self, target: PanelId, kind: SpeechEventKind) {
        let _ = self.events.send(SpeechEvent { target, kind });
        if let Some(wake) = &self.settings.wake {
            wake();
        }
    }

    fn failed(&self, target: PanelId, message: &str) {
        self.send(target, SpeechEventKind::Failed(cut_message(message)));
    }
}

/// The size of the file of one take, or 0 when it cannot be read.
fn take_bytes(file: &Path) -> u64 {
    crate::ui_thread::blocking("the file of a take is measured");
    fs::metadata(file).map_or(0, |metadata| metadata.len())
}

/// The bytes of the file of one take.
fn read_take_file(file: &Path) -> Result<Vec<u8>, String> {
    crate::ui_thread::blocking("the file of a take is read");
    fs::read(file).map_err(|_| "The file of the take cannot be read".to_owned())
}

/// Write the two length fields of an unclosed header of `file` from its size.
///
/// A recorder that the stop had to kill leaves the two length fields of a canonical
/// PCM header at zero or at `0xFFFFFFFF`, and a service reads such a file as a
/// recording of no data. The fields are written here, before either transcriber reads
/// the file, and a file of any other shape is left as it is. A file that cannot be
/// repaired is still sent: it may be the shape of another recorder.
fn close_take_header(file: &Path, bytes: u64) -> std::io::Result<()> {
    if bytes <= wav::CANONICAL_HEADER as u64 {
        return Ok(());
    }
    crate::ui_thread::blocking("the header of a take is read and written");
    let mut header = [0u8; wav::CANONICAL_HEADER];
    let mut take = fs::OpenOptions::new().read(true).write(true).open(file)?;
    {
        use std::io::Read as _;
        take.read_exact(&mut header)?;
    }
    if !wav::close(&mut header, bytes) {
        return Ok(());
    }
    {
        use std::io::{Seek as _, SeekFrom, Write as _};
        take.seek(SeekFrom::Start(4))?;
        take.write_all(&header[4..8])?;
        take.seek(SeekFrom::Start(40))?;
        take.write_all(&header[40..44])?;
    }
    Ok(())
}

/// Whether `path` is one take of this folder, and not another file.
fn is_take_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("take-") && name.to_ascii_lowercase().ends_with(".wav"))
}

/// Why one take was not transcribed, and whether its file stays for a retry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TakeFailure {
    message: String,
    /// The same request can work later, so the file of the take stays on this
    /// computer. A login that was refused or a service that answered with an
    /// error would answer the same way, so its take is dropped at once.
    kept: bool,
}

impl TakeFailure {
    /// A failure that says nothing about the take: the file goes.
    fn dropped(message: String) -> Self {
        Self { message, kept: false }
    }
}

/// Delete the file of one take.
fn remove_take_file(file: &Path) {
    crate::ui_thread::blocking("the file of a take is deleted");
    match fs::remove_file(file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(path = %file.display(), %error, "the file of a take stays"),
    }
}

/// One message as one line of at most [`MAX_MESSAGE_CHARS`] characters.
fn cut_message(message: &str) -> String {
    let line = clean_text(message);
    line.chars().take(MAX_MESSAGE_CHARS).collect()
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

mod wav;

#[cfg(test)]
mod ended_tests;
#[cfg(test)]
mod kept_tests;
#[cfg(test)]
mod tests;
