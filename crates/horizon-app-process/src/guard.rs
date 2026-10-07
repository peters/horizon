use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::group::{child_exit_success, exited, terminate};
use crate::{Error, Event, Kind, Result, Spec, storage};

#[derive(Serialize)]
struct Receipt {
    version: u32,
    operation: uuid::Uuid,
    guardian_pid: u32,
    child_pid: Option<u32>,
    complete: bool,
    task: std::path::PathBuf,
    task_identity: Option<(u64, u64)>,
}

struct Task {
    path: std::path::PathBuf,
    directory: std::fs::File,
    parent: std::fs::File,
}
impl Task {
    fn new(path: &std::path::Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let parent = std::fs::File::from(
            rustix::fs::openat(
                rustix::fs::CWD,
                path.parent().ok_or(Error::StateUnavailable)?,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        let name = path.file_name().ok_or(Error::StateUnavailable)?;
        rustix::fs::mkdirat(&parent, name, rustix::fs::Mode::RWXU).map_err(|_| Error::StateUnavailable)?;
        let directory = std::fs::File::from(
            rustix::fs::openat(
                &parent,
                name,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        let metadata = directory.metadata().map_err(|_| Error::StateUnavailable)?;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(Error::StateUnavailable);
        }
        parent.sync_all().map_err(|_| Error::StateUnavailable)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            parent,
        })
    }
    fn identity(&self) -> Result<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let metadata = self.directory.metadata().map_err(|_| Error::StateUnavailable)?;
        Ok((metadata.dev(), metadata.ino()))
    }
    fn retire(&self) -> Result<()> {
        storage::retire_held_child(
            &self.parent,
            self.path.file_name().ok_or(Error::StateUnavailable)?,
            &self.directory,
        )
    }
}

fn no_child_failure(directory: &storage::Directory, receipt: &mut Receipt, error: Error) -> Result<()> {
    receipt.complete = true;
    directory.save(receipt)?;
    emit(&Event::Failed {
        code: code(error).into(),
    })?;
    Err(error)
}

fn emit(event: &Event) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, event).map_err(|_| Error::CleanupUncertain)?;
    stdout
        .write_all(b"\n")
        .and_then(|()| stdout.flush())
        .map_err(|_| Error::CleanupUncertain)
}

#[cfg(not(unix))]
pub(crate) fn run() -> Result<()> {
    Err(Error::StartFailed)
}

#[cfg(unix)]
pub(crate) fn run() -> Result<()> {
    let (spec, mut input) = read_spec()?;
    // Hold the exact workspace inode before arming; child cwd cannot follow a replaced root.
    let root = std::fs::File::open(&spec.root).map_err(|_| Error::Invalid)?;
    if super::root_identity(&root)? != (spec.root_device, spec.root_inode) {
        return Err(Error::Invalid);
    }
    rustix::process::fchdir(&root).map_err(|_| Error::Invalid)?;
    let directory = storage::Directory::open(&spec.state)?;
    directory
        .new_file("initialized")?
        .sync_all()
        .map_err(|_| Error::StateUnavailable)?;
    let mut receipt = Receipt {
        version: 1,
        operation: spec.operation,
        guardian_pid: std::process::id(),
        child_pid: None,
        complete: false,
        task: task_path(spec.operation)?,
        task_identity: None,
    };
    directory.save(&receipt)?;
    emit(&Event::Armed {})?;
    let mut start = String::new();
    input
        .by_ref()
        .take(16)
        .read_line(&mut start)
        .map_err(|_| Error::Invalid)?;
    if start != "start\n" {
        return no_child_failure(&directory, &mut receipt, Error::StartFailed);
    }
    let (stop_send, stop_receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("native-process-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            let _ = stop_send.send(());
        })
        .map_err(|_| Error::StartFailed)?;
    if crate::lifetime::remaining(spec.deadline_millis).is_err() {
        return no_child_failure(&directory, &mut receipt, Error::Timeout);
    }
    let log = Arc::new(Mutex::new((directory.new_file("output.log")?, 0_usize)));
    let task = Task::new(&receipt.task)?;
    receipt.task_identity = Some(task.identity()?);
    directory.save(&receipt)?;
    let mut command = child_command(&spec, &task);
    if crate::lifetime::remaining(spec.deadline_millis).is_err() {
        task.retire()?;
        return no_child_failure(&directory, &mut receipt, Error::Timeout);
    }
    let Ok(mut child) = command.spawn() else {
        task.retire()?;
        return no_child_failure(&directory, &mut receipt, Error::StartFailed);
    };
    receipt.child_pid = Some(child.id());
    let mut readers = Readers::new();
    let outcome = monitor(
        &mut child,
        &spec,
        &receipt,
        &directory,
        Arc::clone(&log),
        &stop_receive,
        &mut readers,
    );
    let cleanup = stop_child(&mut child, &spec, &readers);
    readers.finish()?;
    cleanup?;
    if matches!(spec.kind, Kind::Backend) && !readers.backend_closed.load(Ordering::Acquire) {
        return Err(Error::CleanupUncertain);
    }
    log.lock()
        .map_err(|_| Error::StateUnavailable)?
        .0
        .sync_all()
        .map_err(|_| Error::StateUnavailable)?;
    // Closing the parent's pipe makes a qualified backend helper clean its nested process group.
    task.retire()?;
    receipt.complete = true;
    directory.save(&receipt)?;
    match outcome {
        Ok(success) => emit(&Event::Complete { success }),
        Err(error) => {
            let _ = emit(&Event::Failed {
                code: code(error).into(),
            });
            Err(error)
        }
    }
}

#[cfg(unix)]
fn child_command(spec: &Spec, task: &Task) -> Command {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(&spec.argv[0]);
    command
        .args(&spec.argv[1..])
        .current_dir(".")
        .env_clear()
        .envs(&spec.environment)
        .env("HORIZON_APP_BACKEND_DIR", &task.path)
        .env("HORIZON_APP_BACKEND_HEARTBEAT", "stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    command
}

fn task_path(operation: uuid::Uuid) -> Result<std::path::PathBuf> {
    Ok(std::env::temp_dir()
        .canonicalize()
        .map_err(|_| Error::StateUnavailable)?
        .join(format!(
            "horizon-native-command-{}-{}",
            operation.simple(),
            uuid::Uuid::new_v4().simple()
        )))
}

fn stop_child(child: &mut Child, spec: &Spec, readers: &Readers) -> Result<()> {
    let cleanup = if matches!(spec.kind, Kind::Backend) {
        request_cleanup(child, readers)
    } else {
        Ok(())
    };
    child.stdin.take();
    if matches!(spec.kind, Kind::Backend) {
        // Allow bounded cooperative namespace cleanup before signalling its subprocesses.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !exited(child)? && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    terminate(child)?;
    cleanup
}

fn request_cleanup(child: &mut Child, readers: &Readers) -> Result<()> {
    let nonce = uuid::Uuid::new_v4();
    *readers.cleanup_nonce.lock().map_err(|_| Error::StateUnavailable)? = Some(nonce);
    let input = child.stdin.as_mut().ok_or(Error::CleanupUncertain)?;
    let request = serde_json::json!({"native_backend_cleanup":1, "nonce":nonce});
    serde_json::to_writer(&mut *input, &request).map_err(|_| Error::CleanupUncertain)?;
    input
        .write_all(b"\n")
        .and_then(|()| input.flush())
        .map_err(|_| Error::CleanupUncertain)
}

#[cfg(unix)]
fn read_spec() -> Result<(Spec, BufReader<std::io::Stdin>)> {
    let mut input = BufReader::new(std::io::stdin());
    let mut first = Vec::new();
    input
        .by_ref()
        .take(32769)
        .read_until(b'\n', &mut first)
        .map_err(|_| Error::Invalid)?;
    if first.len() > 32768 || !first.ends_with(b"\n") {
        return Err(Error::Invalid);
    }
    let spec: Spec = serde_json::from_slice(&first).map_err(|_| Error::Invalid)?;
    spec.validate()?;
    Ok((spec, input))
}

#[cfg(unix)]
fn monitor(
    child: &mut Child,
    spec: &Spec,
    receipt: &Receipt,
    directory: &storage::Directory,
    log: Arc<Mutex<(std::fs::File, usize)>>,
    stop: &mpsc::Receiver<()>,
    readers: &mut Readers,
) -> Result<bool> {
    directory.save(receipt)?;
    emit(&Event::Started {})?;
    let (events_send, events) = mpsc::channel();
    let stderr = child.stderr.take().ok_or(Error::StartFailed)?;
    let stderr_log = Arc::clone(&log);
    readers.spawn("native-process-stderr", stderr, stderr_log, None, None)?;
    let stdout = child.stdout.take().ok_or(Error::StartFailed)?;
    let backend = matches!(spec.kind, Kind::Backend);
    let closed = backend.then(|| (Arc::clone(&readers.cleanup_nonce), Arc::clone(&readers.backend_closed)));
    readers.spawn(
        "native-process-stdout",
        stdout,
        log,
        backend.then_some(events_send),
        closed,
    )?;
    let startup = Instant::now() + Duration::from_secs(spec.startup_seconds);
    let mut ready = !backend;
    loop {
        if stop.try_recv() != Err(mpsc::TryRecvError::Empty) {
            return Ok(false);
        }
        if crate::lifetime::remaining(spec.deadline_millis).is_err() || !ready && Instant::now() >= startup {
            return Err(Error::Timeout);
        }
        if exited(child)? {
            let success = child_exit_success(child)?;
            return if success && ready { Ok(true) } else { Err(Error::Failed) };
        }
        if !ready && let Ok(value) = events.try_recv() {
            let port = value?;
            ready = true;
            emit(&Event::Ready { port })?;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

type Cleanup = (Arc<Mutex<Option<uuid::Uuid>>>, Arc<AtomicBool>);

struct Readers {
    send: mpsc::Sender<Result<()>>,
    done: mpsc::Receiver<Result<()>>,
    started: usize,
    backend_closed: Arc<AtomicBool>,
    cleanup_nonce: Arc<Mutex<Option<uuid::Uuid>>>,
}
impl Readers {
    fn new() -> Self {
        let (send, done) = mpsc::channel();
        Self {
            send,
            done,
            started: 0,
            backend_closed: Arc::new(AtomicBool::new(false)),
            cleanup_nonce: Arc::new(Mutex::new(None)),
        }
    }
    fn spawn(
        &mut self,
        name: &str,
        input: impl Read + Send + 'static,
        log: Arc<Mutex<(std::fs::File, usize)>>,
        ready: Option<mpsc::Sender<Result<u16>>>,
        closed: Option<Cleanup>,
    ) -> Result<()> {
        let done = self.send.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let result = drain(
                    input,
                    &log,
                    ready,
                    closed.as_ref().map(|(cleaning, closed)| (&**cleaning, &**closed)),
                );
                let _ = done.send(result);
            })
            .map_err(|_| Error::StartFailed)?;
        self.started += 1;
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        for _ in 0..self.started {
            self.done
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| Error::CleanupUncertain)??;
        }
        Ok(())
    }
}

fn drain(
    mut input: impl Read,
    log: &Mutex<(std::fs::File, usize)>,
    mut ready: Option<mpsc::Sender<Result<u16>>>,
    closed: Option<(&Mutex<Option<uuid::Uuid>>, &AtomicBool)>,
) -> Result<()> {
    let mut chunk = [0; 8192];
    let mut line = Vec::new();
    let mut overflow = false;
    let mut failure = None;
    loop {
        let Ok(count) = input.read(&mut chunk) else {
            failure.get_or_insert(Error::StateUnavailable);
            break;
        };
        if count == 0 {
            break;
        }
        let mut output = log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let retained = count.min((4 * 1024 * 1024_usize).saturating_sub(output.1));
        if failure.is_none() {
            match output.0.write_all(&chunk[..retained]) {
                Ok(()) => output.1 += retained,
                Err(_) => {
                    failure = Some(Error::StateUnavailable);
                }
            }
        }
        drop(output);
        if ready.is_some() || closed.is_some() {
            for byte in &chunk[..count] {
                if *byte == b'\n' {
                    if let Some(send) = ready.take() {
                        let value = if overflow {
                            Err(Error::Failed)
                        } else {
                            horizon_app_testing::backend::ready_port(&line).map_err(|_| Error::Failed)
                        };
                        let _ = send.send(value);
                    }
                    if !overflow && let Some((cleaning, closed)) = &closed {
                        #[derive(serde::Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Closed {
                            native_backend_closed: u32,
                            nonce: uuid::Uuid,
                        }
                        if let Ok(value) = serde_json::from_slice::<Closed>(&line)
                            && value.native_backend_closed == 1
                        {
                            let expected = match cleaning.lock() {
                                Ok(expected) => *expected,
                                Err(poisoned) => {
                                    failure.get_or_insert(Error::StateUnavailable);
                                    *poisoned.into_inner()
                                }
                            };
                            if expected == Some(value.nonce) {
                                closed.store(true, Ordering::Release);
                            } else {
                                failure.get_or_insert(Error::CleanupUncertain);
                            }
                        }
                    }
                    line.clear();
                    overflow = false;
                } else if line.len() < 256 {
                    line.push(*byte);
                } else {
                    overflow = true;
                }
            }
        }
    }
    if let Some(send) = ready {
        let _ = send.send(Err(Error::Failed));
    }
    failure.map_or(Ok(()), Err)
}

fn code(error: Error) -> &'static str {
    match error {
        Error::Invalid => "app_process_invalid",
        Error::StateUnavailable => "app_process_state_unavailable",
        Error::StartFailed => "app_process_start_failed",
        Error::Failed => "app_process_failed",
        Error::Timeout => "app_process_timeout",
        Error::CleanupUncertain => "app_process_cleanup_uncertain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_pre_cleanup_ack_cannot_become_valid_at_the_read_parse_boundary() {
        struct Boundary<'a> {
            bytes: std::io::Cursor<Vec<u8>>,
            nonce: &'a Mutex<Option<uuid::Uuid>>,
            fresh: uuid::Uuid,
        }
        impl Read for Boundary<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let count = self.bytes.read(buffer)?;
                // Switch phase after bytes enter the reader, before the caller parses this record.
                *self.nonce.lock().unwrap() = Some(self.fresh);
                Ok(count)
            }
        }
        let old = uuid::Uuid::new_v4();
        let nonce = Mutex::new(None);
        let closed = AtomicBool::new(false);
        let input = Boundary {
            bytes: std::io::Cursor::new(format!("{{\"native_backend_closed\":1,\"nonce\":\"{old}\"}}\n").into_bytes()),
            nonce: &nonce,
            fresh: uuid::Uuid::new_v4(),
        };
        let file = tempfile::NamedTempFile::new().unwrap();
        let log = Mutex::new((file.reopen().unwrap(), 0));
        assert_eq!(
            drain(input, &log, None, Some((&nonce, &closed))),
            Err(Error::CleanupUncertain)
        );
        assert!(!closed.load(Ordering::Acquire));
    }

    #[test]
    fn poisoned_cleanup_state_still_drains_the_entire_child_output() {
        let nonce = uuid::Uuid::new_v4();
        let cleaning = Mutex::new(Some(nonce));
        let _ = std::panic::catch_unwind(|| {
            let _held = cleaning.lock().unwrap();
            panic!("synthetic poison");
        });
        let bytes = format!(
            "{{\"native_backend_closed\":1,\"nonce\":\"{nonce}\"}}\n{}",
            "x".repeat(16384)
        )
        .into_bytes();
        let count = bytes.len();
        let mut input = std::io::Cursor::new(bytes);
        let file = tempfile::NamedTempFile::new().unwrap();
        let log = Mutex::new((file.reopen().unwrap(), 0));
        let closed = AtomicBool::new(false);
        assert_eq!(
            drain(&mut input, &log, None, Some((&cleaning, &closed))),
            Err(Error::StateUnavailable)
        );
        assert_eq!(input.position(), count as u64);
    }

    #[test]
    fn failed_log_write_drains_output_and_never_acknowledges_retention() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let log = Mutex::new((std::fs::File::open(file.path()).unwrap(), 0));
        let bytes = vec![b'x'; 16_384];
        let mut input = std::io::Cursor::new(bytes);
        assert_eq!(drain(&mut input, &log, None, None), Err(Error::StateUnavailable));
        assert_eq!(input.position(), 16_384);
        assert_eq!(log.lock().unwrap().1, 0);
        let readers = Readers::new();
        readers.send.send(Err(Error::StateUnavailable)).unwrap();
        let readers = Readers { started: 1, ..readers };
        assert_eq!(readers.finish(), Err(Error::StateUnavailable));
    }
}
