use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::{Error, Event, Kind, Result, Spec, storage};

#[derive(Serialize)]
struct Receipt {
    version: u32,
    operation: uuid::Uuid,
    guardian_pid: u32,
    child_pid: Option<u32>,
    complete: bool,
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
    use std::os::unix::process::CommandExt;
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
        return Err(Error::StartFailed);
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
    let log = Arc::new(Mutex::new((directory.new_file("output.log")?, 0_usize)));
    let mut command = Command::new(&spec.argv[0]);
    command
        .args(&spec.argv[1..])
        .current_dir(&spec.root)
        .env_clear()
        .envs(&spec.environment)
        .env("HORIZON_APP_BACKEND_DIR", &spec.state)
        .env("HORIZON_APP_BACKEND_HEARTBEAT", "stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().map_err(|_| Error::StartFailed)?;
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
    terminate(&mut child)?;
    readers.finish()?;
    if matches!(spec.kind, Kind::Backend) && !readers.backend_closed.load(Ordering::Acquire) {
        return Err(Error::CleanupUncertain);
    }
    log.lock()
        .map_err(|_| Error::StateUnavailable)?
        .0
        .sync_all()
        .map_err(|_| Error::StateUnavailable)?;
    // Closing the parent's pipe makes a qualified backend helper clean its nested process group.
    child.stdin.take();
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
    let closed = backend.then(|| Arc::clone(&readers.backend_closed));
    readers.spawn(
        "native-process-stdout",
        stdout,
        log,
        backend.then_some(events_send),
        closed,
    )?;
    let deadline = Instant::now() + Duration::from_secs(spec.lifetime_seconds);
    let startup = Instant::now() + Duration::from_secs(spec.startup_seconds);
    let mut ready = !backend;
    loop {
        if stop.try_recv() != Err(mpsc::TryRecvError::Empty) {
            return Ok(false);
        }
        if Instant::now() >= deadline || !ready && Instant::now() >= startup {
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

struct Readers {
    send: mpsc::Sender<()>,
    done: mpsc::Receiver<()>,
    started: usize,
    backend_closed: Arc<AtomicBool>,
}
impl Readers {
    fn new() -> Self {
        let (send, done) = mpsc::channel();
        Self {
            send,
            done,
            started: 0,
            backend_closed: Arc::new(AtomicBool::new(false)),
        }
    }
    fn spawn(
        &mut self,
        name: &str,
        input: impl Read + Send + 'static,
        log: Arc<Mutex<(std::fs::File, usize)>>,
        ready: Option<mpsc::Sender<Result<u16>>>,
        closed: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        let done = self.send.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                drain(input, &log, ready, closed.as_deref());
                let _ = done.send(());
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
                .map_err(|_| Error::CleanupUncertain)?;
        }
        Ok(())
    }
}

fn drain(
    mut input: impl Read,
    log: &Mutex<(std::fs::File, usize)>,
    mut ready: Option<mpsc::Sender<Result<u16>>>,
    closed: Option<&AtomicBool>,
) {
    let mut chunk = [0; 8192];
    let mut line = Vec::new();
    let mut overflow = false;
    while let Ok(count) = input.read(&mut chunk) {
        if count == 0 {
            break;
        }
        let mut output = log.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let retained = count.min((4 * 1024 * 1024_usize).saturating_sub(output.1));
        let _ = output.0.write_all(&chunk[..retained]);
        output.1 += retained;
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
                    if !overflow && let Some(closed) = &closed {
                        #[derive(serde::Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct Closed {
                            native_backend_closed: u32,
                        }
                        if serde_json::from_slice::<Closed>(&line).is_ok_and(|value| value.native_backend_closed == 1) {
                            closed.store(true, Ordering::Release);
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
}

#[cfg(unix)]
fn exited(child: &Child) -> Result<bool> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    waitid(
        WaitId::Pid(pid(child)?),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map(|value| value.is_some())
    .map_err(|_| Error::CleanupUncertain)
}

#[cfg(unix)]
fn child_exit_success(child: &Child) -> Result<bool> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    let value = waitid(
        WaitId::Pid(pid(child)?),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map_err(|_| Error::CleanupUncertain)?;
    Ok(value.is_some_and(|value| value.exited() && value.exit_status() == Some(0)))
}

#[cfg(unix)]
fn pid(child: &Child) -> Result<rustix::process::Pid> {
    i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or(Error::CleanupUncertain)
}

#[cfg(unix)]
fn terminate(child: &mut Child) -> Result<()> {
    terminate_with(child, |group, signal| {
        rustix::process::kill_process_group(group, signal)
    })
}

#[cfg(unix)]
fn terminate_with(
    child: &mut Child,
    signal: impl Fn(rustix::process::Pid, rustix::process::Signal) -> rustix::io::Result<()>,
) -> Result<()> {
    use rustix::process::Signal;
    let group = pid(child)?;
    exited(child)?; // retain the waitable leader before any reusable-PID signalling
    child.stdin.take();
    signal(group, Signal::TERM).map_err(|_| Error::CleanupUncertain)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !exited(child)? && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // An exited leader can still have descendants; finish group cleanup before reap.
    signal(group, Signal::KILL).map_err(|_| Error::CleanupUncertain)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !exited(child)? {
        if Instant::now() >= deadline {
            return Err(Error::CleanupUncertain);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait().map_err(|_| Error::CleanupUncertain)?;
    // No more signalling after reap: a later group with this ID could belong to somebody else.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match rustix::process::test_kill_process_group(group) {
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(_) => return Err(Error::CleanupUncertain),
            Ok(()) if group_cannot_execute(group)? => return Ok(()),
            Ok(()) => (),
        }
        if Instant::now() >= deadline {
            return Err(Error::CleanupUncertain);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
fn group_cannot_execute(group: rustix::process::Pid) -> Result<bool> {
    let entries = std::fs::read_dir("/proc").map_err(|_| Error::CleanupUncertain)?;
    for (index, entry) in entries.enumerate() {
        if index >= 65536 {
            return Err(Error::CleanupUncertain);
        }
        let entry = entry.map_err(|_| Error::CleanupUncertain)?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => {
                let tail = stat.rsplit_once(") ").ok_or(Error::CleanupUncertain)?.1;
                let fields: Vec<_> = tail.split_whitespace().take(4).collect();
                if fields.len() != 4 {
                    return Err(Error::CleanupUncertain);
                }
                if fields[2].parse::<i32>().map_err(|_| Error::CleanupUncertain)? == group.as_raw_nonzero().get()
                    && !matches!(fields[0], "Z" | "X")
                {
                    return Ok(false);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::CleanupUncertain),
        }
    }
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
fn group_cannot_execute(_group: rustix::process::Pid) -> Result<bool> {
    Ok(false)
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
    use std::os::unix::process::CommandExt;

    #[test]
    fn failed_group_signal_never_acknowledges_cleanup() {
        let mut child = Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
        assert_eq!(
            terminate_with(&mut child, |_, _| Err(rustix::io::Errno::PERM)),
            Err(Error::CleanupUncertain)
        );
        terminate(&mut child).unwrap();
    }

    #[test]
    fn exited_leader_retains_identity_until_term_resistant_descendant_is_stopped() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "(trap '' TERM; exec sleep 60) & exit 0"])
            .process_group(0)
            .spawn()
            .unwrap();
        while !exited(&child).unwrap() {
            std::thread::sleep(Duration::from_millis(10));
        }
        terminate(&mut child).unwrap();
    }
}
