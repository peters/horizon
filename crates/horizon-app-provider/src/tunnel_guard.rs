//! Private host/guardian protocol. Credential-bearing wire values never become tools or logs.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use horizon_app_process::storage::Directory;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::tunnel::{LocalPort, Status, Tunnel, VerifiedBinary};
use crate::{Error, Result};

/// Trusted host configuration for one private guardian. Deliberately not serializable.
pub struct Request {
    pub worker: PathBuf,
    pub state: PathBuf,
    pub binary: VerifiedBinary,
    pub ports: Vec<LocalPort>,
    pub operation: Uuid,
    pub lifetime: Duration,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    operation: Uuid,
    state: PathBuf,
    binary: PathBuf,
    checksum: String,
    key: String,
    ports: Vec<LocalPort>,
    lifetime_seconds: u64,
    deadline_millis: u64,
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Event {
    Armed,
    Ready,
    Complete,
}
#[derive(Serialize)]
struct Receipt {
    operation: Uuid,
    guardian_pid: u32,
    boot_id: Option<Uuid>,
    child_pid: Option<u32>,
    binary: Option<PathBuf>,
    config: Option<PathBuf>,
    complete: bool,
}
fn emit(event: Event) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &event).map_err(|_| Error::TunnelGuardFailed)?;
    out.write_all(b"\n")
        .and_then(|()| out.flush())
        .map_err(|_| Error::TunnelGuardFailed)
}

/// Private bundled worker entrypoint; never read provider configuration or credentials from the environment.
/// # Errors
/// Unsupported hosts, invalid wire/state, failed startup and uncertain cleanup fail without diagnostics.
pub fn run_guard() -> Result<()> {
    let mut input = BufReader::new(std::io::stdin());
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .by_ref()
        .take(32769)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Error::TunnelGuardFailed)?;
    if bytes.len() > 32768 || !bytes.ends_with(b"\n") {
        return Err(Error::TunnelGuardFailed);
    }
    let wire: Wire = serde_json::from_slice(&bytes).map_err(|_| Error::TunnelGuardFailed)?;
    if wire.operation.is_nil() || !(1..=1800).contains(&wire.lifetime_seconds) {
        return Err(Error::TunnelGuardFailed);
    }
    let directory = Directory::open(&wire.state).map_err(|_| Error::TunnelGuardFailed)?;
    directory
        .new_file("initialized")
        .and_then(|file| {
            file.sync_all()
                .map_err(|_| horizon_app_process::Error::StateUnavailable)
        })
        .map_err(|_| Error::TunnelGuardFailed)?;
    let receipt = std::sync::Mutex::new(Receipt {
        operation: wire.operation,
        guardian_pid: std::process::id(),
        boot_id: horizon_app_process::boot::current(),
        child_pid: None,
        binary: None,
        config: None,
        complete: false,
    });
    directory
        .save(&*receipt.lock().map_err(|_| Error::TunnelGuardFailed)?)
        .map_err(|_| Error::TunnelGuardFailed)?;
    emit(Event::Armed)?;
    let mut start = String::new();
    input
        .by_ref()
        .take(16)
        .read_line(&mut start)
        .map_err(|_| Error::TunnelGuardFailed)?;
    if start != "start\n" {
        // No tunnel child exists before the start command. Persist this bounded cancellation.
        let mut receipt = receipt.lock().map_err(|_| Error::TunnelGuardFailed)?;
        receipt.complete = true;
        directory.save(&*receipt).map_err(|_| Error::TunnelGuardFailed)?;
        return Err(Error::TunnelGuardFailed);
    }
    let (stop_send, stop) = mpsc::channel();
    std::thread::Builder::new()
        .name("native-tunnel-parent".into())
        .spawn(move || {
            let mut byte = [0];
            let _ = input.read(&mut byte);
            let _ = stop_send.send(());
        })
        .map_err(|_| Error::TunnelGuardFailed)?;
    let mut tunnel = match start_tunnel(&wire, &receipt, &directory) {
        Ok(tunnel) => tunnel,
        Err(error) => return failed_start(&receipt, &directory, error),
    };
    // A lost Ready delivery also cleans the tunnel; Drop never substitutes for acknowledgement.
    let ready = emit(Event::Ready);
    if ready.is_ok() {
        while stop.recv_timeout(Duration::from_millis(100)) == Err(mpsc::RecvTimeoutError::Timeout) {
            if !tunnel.status()?.ready {
                break;
            }
        }
    }
    tunnel.close()?;
    let mut receipt = receipt.lock().map_err(|_| Error::TunnelGuardFailed)?;
    receipt.complete = true;
    directory.save(&*receipt).map_err(|_| Error::TunnelGuardFailed)?;
    ready?;
    emit(Event::Complete)
}

fn start_tunnel(wire: &Wire, receipt: &std::sync::Mutex<Receipt>, directory: &Directory) -> Result<Tunnel> {
    let binary = VerifiedBinary::capture(&wire.binary, &wire.checksum)?;
    let remaining =
        horizon_app_process::lifetime::remaining(wire.deadline_millis).map_err(|_| Error::TunnelGuardFailed)?;
    Tunnel::start(
        binary,
        &wire.key,
        wire.ports.clone(),
        wire.operation,
        remaining,
        |record| {
            let mut receipt = receipt.lock().map_err(|_| Error::TunnelGuardFailed)?;
            receipt.child_pid = record.pid;
            receipt.binary = Some(record.binary.to_owned());
            receipt.config = Some(record.config.to_owned());
            directory.save(&*receipt).map_err(|_| Error::TunnelGuardFailed)
        },
    )
}

fn failed_start(receipt: &std::sync::Mutex<Receipt>, directory: &Directory, error: Error) -> Result<()> {
    // These Tunnel::start errors imply no child started or exact cleanup succeeded.
    // Cleanup uncertainty never reaches a terminal acknowledgement.
    if matches!(
        error,
        Error::TunnelStartFailed | Error::TunnelPortRefused | Error::TunnelBinaryRejected | Error::TunnelGuardFailed
    ) {
        let mut receipt = receipt.lock().map_err(|_| Error::TunnelGuardFailed)?;
        receipt.complete = true;
        directory.save(&*receipt).map_err(|_| Error::TunnelGuardFailed)?;
        emit(Event::Complete)?;
    }
    Err(error)
}

/// Private captured tunnel lease; no raw credential, process identity or binary path is publicly serializable.
pub struct GuardedTunnel {
    child: Child,
    input: Option<ChildStdin>,
    events: mpsc::Receiver<Result<Event>>,
    id: Uuid,
    ports: Vec<LocalPort>,
    deadline: Instant,
    started: bool,
    complete: bool,
}
impl GuardedTunnel {
    pub(crate) fn start(request: Request, key: &str, journal: impl FnOnce(Uuid, u32) -> Result<()>) -> Result<Self> {
        let Request {
            worker,
            state,
            binary,
            ports,
            operation: id,
            lifetime,
        } = request;
        if lifetime.is_zero() || lifetime > Duration::from_mins(30) || id.is_nil() {
            return Err(Error::TunnelGuardFailed);
        }
        let deadline = Instant::now() + lifetime;
        let deadline_millis =
            horizon_app_process::lifetime::deadline_after(lifetime).map_err(|_| Error::TunnelGuardFailed)?;
        let (path, checksum) = binary.copy_location();
        let wire = Wire {
            operation: id,
            state,
            binary: path.to_owned(),
            checksum: checksum.to_owned(),
            key: key.to_owned(),
            ports: ports.clone(),
            lifetime_seconds: lifetime.as_secs(),
            deadline_millis,
        };
        let encoded = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| Error::TunnelGuardFailed)?);
        if encoded.len() >= 32768 {
            return Err(Error::TunnelGuardFailed);
        }
        let mut command = Command::new(&worker);
        command
            .arg("--tunnel-guard")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|_| Error::TunnelGuardFailed)?;
        let input = child.stdin.take().ok_or(Error::TunnelGuardFailed)?;
        let output = child.stdout.take().ok_or(Error::TunnelGuardFailed)?;
        let (send, events) = mpsc::channel();
        std::thread::Builder::new()
            .name("native-tunnel-events".into())
            .spawn(move || {
                let mut output = BufReader::new(output);
                loop {
                    let mut bytes = Vec::new();
                    let value = if output.by_ref().take(257).read_until(b'\n', &mut bytes).is_err()
                        || bytes.is_empty()
                        || bytes.len() > 256
                        || !bytes.ends_with(b"\n")
                    {
                        Err(Error::TunnelCleanupUncertain)
                    } else {
                        serde_json::from_slice(&bytes).map_err(|_| Error::TunnelCleanupUncertain)
                    };
                    let failed = value.is_err();
                    if send.send(value).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| {
                let _ = child.kill();
                let _ = child.wait();
                Error::TunnelGuardFailed
            })?;
        let mut guarded = Self {
            child,
            input: Some(input),
            events,
            id,
            ports,
            deadline,
            started: false,
            complete: false,
        };
        let input = guarded.input.as_mut().ok_or(Error::TunnelGuardFailed)?;
        serde_json::to_writer(&mut *input, &wire).map_err(|_| Error::TunnelGuardFailed)?;
        input
            .write_all(b"\n")
            .and_then(|()| input.flush())
            .map_err(|_| Error::TunnelGuardFailed)?;
        if !matches!(guarded.event(Duration::from_secs(5))?, Event::Armed) {
            return Err(Error::TunnelGuardFailed);
        }
        journal(id, guarded.child.id())?;
        horizon_app_process::lifetime::remaining(deadline_millis).map_err(|_| Error::TunnelGuardFailed)?;
        guarded.started = true;
        let input = guarded.input.as_mut().ok_or(Error::TunnelGuardFailed)?;
        input
            .write_all(b"start\n")
            .and_then(|()| input.flush())
            .map_err(|_| Error::TunnelGuardFailed)?;
        guarded.confirm_ready()?;
        Ok(guarded)
    }
    fn confirm_ready(&mut self) -> Result<()> {
        match self.event(Duration::from_secs(40))? {
            Event::Ready => (),
            Event::Complete => {
                self.complete = true;
                return Err(Error::TunnelStartFailed);
            }
            Event::Armed => return Err(Error::TunnelGuardFailed),
        }
        Ok(())
    }
    fn event(&self, timeout: Duration) -> Result<Event> {
        self.events
            .recv_timeout(timeout)
            .map_err(|_| Error::TunnelCleanupUncertain)?
    }
    /// # Errors
    /// Only typed local readiness is returned; provider identifiers remain inside the host.
    pub fn status(&mut self) -> Result<Status> {
        if self
            .child
            .try_wait()
            .map_err(|_| Error::TunnelCleanupUncertain)?
            .is_some()
            && !self.complete
        {
            if !matches!(self.events.try_recv(), Ok(Ok(Event::Complete))) {
                return Err(Error::TunnelCleanupUncertain);
            }
            self.complete = true;
        }
        Ok(Status {
            id: self.id,
            ready: !self.complete && Instant::now() < self.deadline,
            ports: self.ports.clone(),
            remaining_seconds: self.deadline.saturating_duration_since(Instant::now()).as_secs(),
        })
    }
    /// # Errors
    /// Closing the host pipe causes the surviving worker to stop and confirm its child before completion.
    pub fn close(&mut self) -> Result<()> {
        self.input.take();
        if !self.started {
            // EOF lets the armed guardian durably acknowledge that no child was authorized.
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while self
                .child
                .try_wait()
                .map_err(|_| Error::TunnelCleanupUncertain)?
                .is_none()
            {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = self.child.kill();
            self.child.wait().map_err(|_| Error::TunnelCleanupUncertain)?;
            self.complete = true;
            return Ok(());
        }
        if !self.complete {
            if !matches!(self.event(Duration::from_secs(15))?, Event::Complete) {
                return Err(Error::TunnelCleanupUncertain);
            }
            self.complete = true;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self
            .child
            .try_wait()
            .map_err(|_| Error::TunnelCleanupUncertain)?
            .is_none()
        {
            if Instant::now() >= deadline {
                return Err(Error::TunnelCleanupUncertain);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}
impl Drop for GuardedTunnel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
