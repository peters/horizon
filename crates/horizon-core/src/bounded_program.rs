//! One program of this process, run as the exact child of this process.
//!
//! The keyring import and the working directory of a panel ask the programs of the
//! system a question off the UI thread. An answer travels through the stdout pipe of
//! the child and nowhere else: no argument, no environment variable, no log line and
//! no temporary file. The environment of the child is cleared and rebuilt from four
//! names, so an answer never arrives in a variable that a person or a shell exported.
//! The child is ended by its own process id when the bound passes, so no other program
//! of this computer can be touched.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::Signal;
use zeroize::Zeroizing;

/// The most bytes that one program may print. One answer of a keyring entry is at
/// most 16 KiB and a directory or a process id is shorter, so this bound is far
/// above the largest answer and far below a memory problem.
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024;

/// How often the parent looks whether its child ended.
const POLL: Duration = Duration::from_millis(10);

/// How long a start of a program tries again while this computer cannot start it
/// now. A busy machine can hold the last process slot for a moment, and another
/// thread of this process can hold a file open while its program is about to run;
/// neither is a reason to refuse the values of the person.
const SPAWN_RETRY: Duration = Duration::from_secs(2);

/// What one program that this process runs needs from the environment.
///
/// The programs of the keyring read a session bus, a home and a runtime folder of
/// the person, and a program that names a child or a directory reads neither: a
/// cleared environment keeps every variable of the shell of the person out of
/// them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramEnvironment {
    /// The folders that hold the programs, as `PATH` writes them.
    pub path: OsString,
    pub home: Option<OsString>,
    /// `DBUS_SESSION_BUS_ADDRESS`, which the Secret Service of Linux reads.
    pub session_bus: Option<OsString>,
    /// `XDG_RUNTIME_DIR`, where a session bus of Linux often lives.
    pub runtime_dir: Option<OsString>,
    /// Whether the child keeps every variable of this process instead of the four
    /// names above. A program that a person wrote and named in the settings, such
    /// as a recorder of speech input, reads the session that the person set, and
    /// it reads no value of a named secret: no named secret is a variable of this
    /// process.
    pub inherit: bool,
}

impl ProgramEnvironment {
    /// The environment of this process, for the work that a click or a panel asks
    /// for.
    #[must_use]
    pub fn of_this_process() -> Self {
        Self {
            path: std::env::var_os("PATH").unwrap_or_default(),
            home: std::env::var_os("HOME"),
            session_bus: std::env::var_os("DBUS_SESSION_BUS_ADDRESS"),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            inherit: false,
        }
    }

    /// The environment of this process as it is, for a program that a person wrote
    /// and named in the settings. The child reads the variables of the session of
    /// the person, and the caller that starts such a program never puts a value of
    /// a secret in one of them.
    #[must_use]
    pub fn inherited() -> Self {
        Self {
            inherit: true,
            ..Self::of_this_process()
        }
    }

    /// An environment whose `PATH` is `path` and nothing else that a test needs.
    /// A test points it at its own folder of stand-in programs, so the work never
    /// reads the keyring of the person.
    #[must_use]
    pub fn with_path(path: impl Into<OsString>) -> Self {
        Self {
            path: path.into(),
            home: None,
            session_bus: None,
            runtime_dir: None,
            inherit: false,
        }
    }

    /// The path of `name` in the folders of this environment, or `None` when no
    /// folder holds a program of that name.
    ///
    /// The path is resolved here rather than by [`Command`], because a child with a
    /// cleared environment must not be found through the `PATH` of another process.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        self.folders()
            .map(|folder| folder.join(name))
            .find(|candidate| is_program(candidate))
    }

    fn folders(&self) -> impl Iterator<Item = PathBuf> {
        std::env::split_paths(&self.path).filter(|folder| !folder.as_os_str().is_empty())
    }

    fn apply(&self, command: &mut Command) {
        if self.inherit {
            return;
        }
        command.env_clear();
        command.env("PATH", &self.path);
        for (name, value) in [
            ("HOME", &self.home),
            ("DBUS_SESSION_BUS_ADDRESS", &self.session_bus),
            ("XDG_RUNTIME_DIR", &self.runtime_dir),
        ] {
            if let Some(value) = value {
                command.env(name, value);
            }
        }
    }
}

/// Whether `path` is a program that this process may run.
fn is_program(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// What one program printed.
pub struct ProgramOutput {
    pub status: ExitStatus,
    /// Wiped when it goes: a program of the keyring import answers the values of a
    /// person, and no caller keeps an answer of this process longer than it needs.
    pub stdout: Zeroizing<Vec<u8>>,
    /// Wiped when it goes, because a value never reaches this stream but a program
    /// of the person must not keep anything either.
    pub stderr: Zeroizing<Vec<u8>>,
}

impl ProgramOutput {
    /// The code that a child ended with, or `None` when a signal ended it.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }
}

/// Why a program did not answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramFailure {
    /// The bound passed, so the child was ended by its own process id.
    TimedOut,
    /// The child could not be started, read or reaped.
    Failed,
}

/// Run `program`, a path that [`ProgramEnvironment::find`] answered, with `args`
/// and a cleared environment. The whole run is bounded by `bound`.
///
/// The child is ended by its own pid when the bound passes: [`Child::kill`] sends
/// the signal to that process and to no group, so no other program of this
/// computer can be touched.
pub fn run(
    program: &Path,
    args: &[OsString],
    environment: &ProgramEnvironment,
    bound: Duration,
) -> Result<ProgramOutput, ProgramFailure> {
    crate::ui_thread::blocking("a program of this process runs and is awaited");
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    environment.apply(&mut command);
    let mut child = spawn(&mut command)?;
    let reading = reading(&mut child);
    let deadline = Instant::now() + bound;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, stderr) = reading.finish()?;
                return Ok(ProgramOutput { status, stdout, stderr });
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, "a program of this process could not be awaited");
                let _ = child.kill();
                let _ = child.wait();
                return Err(ProgramFailure::Failed);
            }
        }
        if Instant::now() >= deadline {
            // The bound passed: this pid alone, and then reap it.
            let _ = child.kill();
            let _ = child.wait();
            drop(reading.finish());
            return Err(ProgramFailure::TimedOut);
        }
        thread::sleep(POLL);
    }
}

/// Why a program that a caller started did not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramStart {
    /// No folder of the environment holds a program of that name.
    NotInstalled,
    /// The program is there, and this process could not start it.
    Failed,
}

/// One program of this process that runs until its caller stops it.
///
/// A recorder of speech input runs while a hotkey is down, so its caller starts it,
/// polls it and stops it by its own process id: [`StartedProgram::signal`] sends one
/// signal to that id and to no process group, so no other program of this computer
/// can be touched. The standard error of the child is read while it runs, so a child
/// that fills the pipe cannot stop before its end, and the last line of it answers
/// why a child ended early.
pub struct StartedProgram {
    child: Child,
    stderr: Option<thread::JoinHandle<Result<Zeroizing<Vec<u8>>, ProgramFailure>>>,
}

impl StartedProgram {
    /// Start `program` with `args` and the environment that `environment` applies.
    ///
    /// # Errors
    /// Reports [`ProgramStart::NotInstalled`] when no folder of the environment
    /// holds the program, and [`ProgramStart::Failed`] when the child cannot be
    /// started.
    pub fn start(program: &Path, args: &[OsString], environment: &ProgramEnvironment) -> Result<Self, ProgramStart> {
        crate::ui_thread::blocking("a program of this process is started and left to run");
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        environment.apply(&mut command);
        let child = spawn_started(&mut command)?;
        let mut child = child;
        let stderr = child
            .stderr
            .take()
            .map(|stderr| thread::spawn(move || read_capped(stderr)));
        Ok(Self { child, stderr })
    }

    /// The process id of the child. Every signal of this type goes to this id and
    /// to no group.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Whether the child ended, and the status that it ended with.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Send `signal` to the process id of this child.
    ///
    /// Answers `false` when the signal did not reach the child, which a child that
    /// already ended causes.
    pub fn signal(&self, signal: Signal) -> bool {
        let Ok(raw) = i32::try_from(self.id()) else {
            return false;
        };
        let Some(pid) = rustix::process::Pid::from_raw(raw) else {
            return false;
        };
        rustix::process::kill_process(pid, signal).is_ok()
    }

    /// Send `SIGKILL` to the process id of this child, and reap it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The last line of the standard error of a child that ended.
    ///
    /// Call this only after the child ended: the reader of the pipe finishes at the
    /// end of the child, so this waits for a child that still runs.
    pub fn last_stderr_line(&mut self) -> Option<String> {
        let stderr = self.stderr.take()?.join().ok()?.ok()?;
        let text = String::from_utf8_lossy(&stderr);
        let line = text.lines().rev().find(|line| !line.trim().is_empty())?;
        Some(line.trim().to_owned())
    }
}

/// Start a child of this process, and try again while this computer cannot start it
/// now. A name that no folder of the environment holds answers
/// [`ProgramStart::NotInstalled`], so a caller can name the program that a person
/// must install.
fn spawn_started(command: &mut Command) -> Result<Child, ProgramStart> {
    let deadline = Instant::now() + SPAWN_RETRY;
    loop {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProgramStart::NotInstalled);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ExecutableFileBusy
                ) && Instant::now() < deadline =>
            {
                thread::sleep(POLL);
            }
            Err(error) => {
                tracing::warn!(%error, "a program of this process did not start");
                return Err(ProgramStart::Failed);
            }
        }
    }
}

/// Start the child, and try again while this computer cannot start it now.
///
/// The two answers that are tried again are `EAGAIN`, which a full process table
/// gives, and `ETXTBSY`, which a file that another thread still has open gives. A
/// person waits for one answer, and neither answer means that the question has no
/// answer.
fn spawn(command: &mut Command) -> Result<Child, ProgramFailure> {
    let deadline = Instant::now() + SPAWN_RETRY;
    loop {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::ExecutableFileBusy
                ) && Instant::now() < deadline =>
            {
                thread::sleep(POLL);
            }
            Err(error) => {
                tracing::warn!(%error, "a program of this process did not start");
                return Err(ProgramFailure::Failed);
            }
        }
    }
}

/// The bytes of both streams of one child, wiped when they go.
type Streams = (Zeroizing<Vec<u8>>, Zeroizing<Vec<u8>>);

/// The two readers of the pipes of one child. They read while the parent waits for
/// the end of the child, so a program that prints more than one pipe buffer cannot
/// stop before its end and hold the parent to the bound.
struct Reading {
    stdout: thread::JoinHandle<Result<Zeroizing<Vec<u8>>, ProgramFailure>>,
    stderr: thread::JoinHandle<Result<Zeroizing<Vec<u8>>, ProgramFailure>>,
}

impl Reading {
    /// The bytes of both streams. A stream that could not be read, or a program
    /// that printed more than [`MAX_OUTPUT_BYTES`], is one failure, so a program
    /// cannot fill the memory of this process.
    fn finish(self) -> Result<Streams, ProgramFailure> {
        let stdout = self.stdout.join().unwrap_or(Err(ProgramFailure::Failed))?;
        let stderr = self.stderr.join().unwrap_or(Err(ProgramFailure::Failed))?;
        Ok((stdout, stderr))
    }
}

fn reading(child: &mut Child) -> Reading {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    Reading {
        stdout: thread::spawn(move || stdout.map_or_else(|| Ok(Zeroizing::new(Vec::new())), read_capped)),
        stderr: thread::spawn(move || stderr.map_or_else(|| Ok(Zeroizing::new(Vec::new())), read_capped)),
    }
}

fn read_capped(stream: impl Read) -> Result<Zeroizing<Vec<u8>>, ProgramFailure> {
    let mut bytes = Zeroizing::new(Vec::new());
    stream
        .take(MAX_OUTPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProgramFailure::Failed)?;
    if bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(ProgramFailure::Failed);
    }
    Ok(bytes)
}
