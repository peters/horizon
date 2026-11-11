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
//! 4. Stop the child with `SIGINT` to its own process id, wait up to 2 seconds and
//!    then kill that id. A take shorter than 0.3 seconds, or a file with no byte,
//!    is dropped and no request runs.
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

/// The most time that one take records.
pub const MAX_TAKE: Duration = Duration::from_secs(120);

/// The shortest take that a transcriber runs for.
pub const MIN_TAKE: Duration = Duration::from_millis(300);

/// The most time that one transcription may take.
pub const TRANSCRIBE_BOUND: Duration = Duration::from_secs(30);

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
        let mut bytes = NamedSecretVault::value(self, name).map_err(secret_error_text)?;
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
fn secret_error_text(error: NamedSecretError) -> String {
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

    /// `Escape`, or the end of the window: drop the take that runs.
    pub fn cancel(&self) {
        let _ = self.requests.send(Request::Cancel);
    }

    /// One event of the worker, or `None` while it has none.
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
    worker: SpeechWorker,
    requests: Receiver<Request>,
    events: Sender<SpeechEvent>,
}

impl Worker {
    fn new(worker: SpeechWorker, requests: Receiver<Request>, events: Sender<SpeechEvent>) -> Self {
        Self {
            worker,
            requests,
            events,
        }
    }

    fn run(mut self) {
        self.clear_stale_takes();
        loop {
            match self.requests.recv() {
                Ok(Request::Begin { plan, target }) => self.take(&plan, target),
                Ok(Request::End | Request::Cancel) => {}
                Ok(Request::Stop) | Err(_) => break,
            }
        }
    }

    fn take(&mut self, plan: &TakePlan, target: PanelId) {
        let Some(file) = self.new_take_file() else {
            self.failed(target, "The folder of the takes in the Horizon home cannot be made");
            return;
        };
        let started = self.worker.clock.now_ms();
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
        let elapsed = self.worker.clock.now_ms().saturating_sub(started);
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
        if bytes == 0 || elapsed < millis(self.worker.bounds.min_take) {
            remove_take_file(&file);
            self.send(target, SpeechEventKind::Dropped);
            return;
        }
        if bytes > self.worker.bounds.max_file_bytes {
            remove_take_file(&file);
            self.failed(target, "The recording of the take is larger than 25 MB");
            return;
        }
        self.send(target, SpeechEventKind::Writing);
        let file_name = file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        match self.transcribe(plan, &file, &file_name) {
            Ok(text) => {
                remove_take_file(&file);
                let text = clean_text(&text);
                if text.is_empty() {
                    self.failed(target, "The take held no text");
                } else {
                    self.send(target, SpeechEventKind::Text(text));
                }
            }
            Err(message) => {
                remove_take_file(&file);
                self.failed(target, &message);
            }
        }
    }

    /// Read the key events and the child until the take ends.
    fn record(&self, child: &mut StartedProgram, started_ms: u64) -> RecordEnd {
        loop {
            match self.requests.try_recv() {
                Ok(Request::End) => return RecordEnd::KeyUp,
                Ok(Request::Cancel | Request::Stop) => return RecordEnd::Cancelled,
                // One take runs at a time: a second press changes nothing.
                Ok(Request::Begin { .. }) => {}
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return RecordEnd::Cancelled,
            }
            if child.try_wait().is_some() {
                return RecordEnd::EndedEarly;
            }
            if self.worker.clock.now_ms().saturating_sub(started_ms) >= millis(self.worker.bounds.max_take) {
                return RecordEnd::Maxed;
            }
            self.worker.clock.sleep(self.worker.bounds.poll);
        }
    }

    /// Stop the recorder: `SIGINT` to its own process id, then `SIGKILL` when its
    /// bound passes.
    fn stop(&self, child: &mut StartedProgram) {
        if child.try_wait().is_some() {
            return;
        }
        if !child.signal(Signal::INT) {
            child.kill();
            return;
        }
        let deadline = self
            .worker
            .clock
            .now_ms()
            .saturating_add(millis(self.worker.bounds.stop_wait));
        loop {
            if child.try_wait().is_some() {
                return;
            }
            if self.worker.clock.now_ms() >= deadline {
                break;
            }
            self.worker.clock.sleep(self.worker.bounds.poll);
        }
        // The recorder ignored the signal, so this id alone is killed and reaped.
        child.kill();
    }

    fn transcribe(&self, plan: &TakePlan, file: &Path, file_name: &str) -> Result<String, String> {
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
                    .worker
                    .keys
                    .value(key_secret)
                    .map_err(|error| cut_message(&error))?;
                let Some(key) = key else {
                    return Err(format!("No secret named {key_secret} is on this computer"));
                };
                let bytes = read_take_file(file)?;
                let request = ServiceRequest {
                    endpoint,
                    model,
                    language: language.as_deref(),
                    file_name,
                    file: &bytes,
                    key: key.as_str(),
                    bound: self.worker.bounds.transcribe,
                };
                service::transcribe(&request).map_err(|error| error.to_string())
            }
        }
    }

    fn transcribe_by_command(&self, command: &CommandLine, file: &Path) -> Result<String, String> {
        let args = command.arguments_for(file);
        match run(
            Path::new(command.program()),
            &args,
            &ProgramEnvironment::inherited(),
            self.worker.bounds.transcribe,
        ) {
            Ok(output) => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            Err(ProgramFailure::TimedOut) => Err(format!(
                "{} did not answer in {} seconds",
                command.program(),
                self.worker.bounds.transcribe.as_secs()
            )),
            Err(ProgramFailure::Failed) => Err(format!("{} did not answer", command.program())),
        }
    }

    /// Delete every file that an earlier take left in the folder.
    fn clear_stale_takes(&self) {
        crate::ui_thread::blocking("the takes of an earlier run are deleted");
        let folder = self.worker.home.join(TAKE_FOLDER);
        let Ok(entries) = fs::read_dir(&folder) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), %error, "a take of an earlier run stays");
            }
        }
    }

    /// Make the folder of the takes and one empty file of this take.
    fn new_take_file(&self) -> Option<PathBuf> {
        crate::ui_thread::blocking("the folder of the takes is made");
        let folder = self.worker.home.join(TAKE_FOLDER);
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
        if let Some(wake) = &self.worker.wake {
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

#[cfg(test)]
mod tests;
