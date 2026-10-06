use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempPath};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{Error, Result};

pub struct VerifiedBinary {
    file: TempPath,
    checksum: String,
}

impl VerifiedBinary {
    /// # Errors
    /// The checksum is trusted host configuration, never a project/tool parameter.
    pub fn capture(path: &Path, checksum: &str) -> Result<Self> {
        if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::TunnelBinaryRejected);
        }
        let mut source = binary_file(path)?;
        let metadata = source.metadata().map_err(|_| Error::TunnelBinaryRejected)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 128 * 1024 * 1024 {
            return Err(Error::TunnelBinaryRejected);
        }
        let mut file = NamedTempFile::new().map_err(|_| Error::TunnelBinaryRejected)?;
        let mut hash = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0; 8192];
        loop {
            let count = source.read(&mut buffer).map_err(|_| Error::TunnelBinaryRejected)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > 128 * 1024 * 1024 {
                return Err(Error::TunnelBinaryRejected);
            }
            hash.update(&buffer[..count]);
            file.write_all(&buffer[..count])
                .map_err(|_| Error::TunnelBinaryRejected)?;
        }
        let actual = hash
            .finalize()
            .iter()
            .flat_map(|byte| [hex(byte >> 4), hex(byte & 15)])
            .collect::<String>();
        if actual != checksum.to_ascii_lowercase() {
            return Err(Error::TunnelBinaryRejected);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::TunnelBinaryRejected)?;
        }
        Ok(Self {
            file: file.into_temp_path(),
            checksum: actual,
        })
    }
    pub(crate) fn copy_location(&self) -> (&Path, &str) {
        (&self.file, &self.checksum)
    }
}

fn hex(value: u8) -> char {
    char::from(b"0123456789abcdef"[usize::from(value)])
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct LocalPort {
    pub address: SocketAddr,
    pub tls: bool,
}

/// Provider routing alias for this exact opaque tunnel operation.
#[must_use]
pub fn local_identifier(id: Uuid) -> String {
    format!("horizon-native-{id}")
}

pub struct ProcessRecord<'a> {
    pub id: Uuid,
    pub pid: Option<u32>,
    pub binary: &'a Path,
    pub config: &'a Path,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub id: Uuid,
    pub ready: bool,
    pub ports: Vec<LocalPort>,
    pub remaining_seconds: u64,
}

pub struct Tunnel {
    id: Uuid,
    ports: Vec<LocalPort>,
    deadline: Instant,
    child: Arc<Mutex<Child>>,
    finished: Arc<AtomicBool>,
    cleanup_confirmed: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
    guardian: Option<std::thread::JoinHandle<()>>,
    resources: Arc<Mutex<Option<(VerifiedBinary, NamedTempFile)>>>,
}

impl Tunnel {
    /// # Errors
    /// Private host entrypoint. The journal callback must durably record intent and process identity before ready.
    /// Startup errors acknowledge no authorized child or confirmed exact group/file cleanup;
    /// failed termination, unlink or parent synchronization returns `TunnelCleanupUncertain`.
    pub(crate) fn start(
        mut binary: VerifiedBinary,
        key: &str,
        ports: Vec<LocalPort>,
        id: Uuid,
        lifetime: Duration,
        journal: impl Fn(ProcessRecord<'_>) -> Result<()>,
    ) -> Result<Self> {
        let prepared = (|| {
            if lifetime.is_zero()
                || lifetime > Duration::from_mins(30)
                || key.is_empty()
                || key.len() > 2048
                || !key.bytes().all(|b| b.is_ascii_graphic())
            {
                return Err(Error::TunnelStartFailed);
            }
            let deadline = Instant::now().checked_add(lifetime).ok_or(Error::TunnelStartFailed)?;
            let only = allowlist(&ports)?;
            for port in &ports {
                TcpStream::connect_timeout(&port.address, Duration::from_millis(500))
                    .map_err(|_| Error::TunnelPortRefused)?;
            }
            if Instant::now() >= deadline {
                return Err(Error::TunnelStartFailed);
            }
            Ok((deadline, only))
        })();
        let (deadline, only) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                binary.file.disable_cleanup(true);
                retire_paths(&[binary.file.as_ref()])?;
                return Err(error);
            }
        };
        let (mut child, mut config) = spawn(&mut binary, key, &only, id, deadline, &journal)?;
        if let Err(error) = wait_ready(&mut child, deadline) {
            terminate(&mut child)?;
            retire_files(&mut binary, &mut config)?;
            return Err(error);
        }
        let child = Arc::new(Mutex::new(child));
        let worker = Arc::clone(&child);
        let finished = Arc::new(AtomicBool::new(false));
        let completed = Arc::clone(&finished);
        let cleanup_confirmed = Arc::new(AtomicBool::new(false));
        let confirmed = Arc::clone(&cleanup_confirmed);
        let resources = Arc::new(Mutex::new(Some((binary, config))));
        let retained = Arc::clone(&resources);
        let (stop_send, stop_receive) = mpsc::channel();
        let guardian = spawn_guard(move || {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero()
                    || stop_receive.recv_timeout(remaining.min(Duration::from_millis(100)))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
                if worker.lock().is_ok_and(|mut child| exited(&mut child).unwrap_or(true)) {
                    break;
                }
            }
            completed.store(true, Ordering::Release);
            let mut files = retained.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if cleanup(
                &mut worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
                &mut files,
            )
            .is_ok()
            {
                confirmed.store(true, Ordering::Release);
            }
        })
        .map_err(|_| {
            match cleanup(
                &mut child.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
                &mut resources.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            ) {
                Ok(()) => Error::TunnelStartFailed,
                Err(error) => error,
            }
        })?;
        Ok(Self {
            id,
            ports,
            deadline,
            child,
            finished,
            cleanup_confirmed,
            stop: Some(stop_send),
            guardian: Some(guardian),
            resources,
        })
    }

    /// # Errors
    /// Reports only typed state; binary diagnostics are never returned or persisted.
    pub fn status(&self) -> Result<Status> {
        let mut child = self.child.lock().map_err(|_| Error::TunnelStartFailed)?;
        let ready = !self.finished.load(Ordering::Acquire) && !exited(&mut child)? && Instant::now() < self.deadline;
        Ok(Status {
            id: self.id,
            ready,
            ports: self.ports.clone(),
            remaining_seconds: self.deadline.saturating_duration_since(Instant::now()).as_secs(),
        })
    }

    /// # Errors
    /// Cleanup is confirmed only after the waitable child group cannot execute.
    pub fn close(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(guardian) = self.guardian.take() {
            guardian.join().map_err(|_| Error::TunnelCleanupUncertain)?;
        }
        if !self.cleanup_confirmed.load(Ordering::Acquire) {
            return Err(Error::TunnelCleanupUncertain);
        }
        self.resources.lock().map_err(|_| Error::TunnelCleanupUncertain)?.take();
        Ok(())
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn allowlist(ports: &[LocalPort]) -> Result<String> {
    if ports.is_empty() || ports.len() > 16 {
        return Err(Error::TunnelPortRefused);
    }
    let mut seen = BTreeSet::new();
    let mut entries = BTreeSet::new();
    for port in ports {
        if port.address.port() == 0
            || !matches!(port.address.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
                && !matches!(port.address.ip(), IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST)
            || !seen.insert((port.address, port.tls))
        {
            return Err(Error::TunnelPortRefused);
        }
        let ssl = u8::from(port.tls);
        entries.insert(format!("{},{},{ssl}", port.address.ip(), port.address.port()));
        entries.insert(format!("localhost,{},{ssl}", port.address.port()));
        if port.address.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) {
            entries.insert(format!("bs-local.com,{},{ssl}", port.address.port()));
        }
    }
    Ok(entries.into_iter().collect::<Vec<_>>().join(","))
}

#[cfg(unix)]
fn exited(child: &mut Child) -> Result<bool> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let pid = i32::try_from(child.id())
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(Error::TunnelStartFailed)?;
    waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map(|status| status.is_some())
    .map_err(|_| Error::TunnelStartFailed)
}

#[cfg(not(unix))]
fn exited(child: &mut Child) -> Result<bool> {
    child
        .try_wait()
        .map(|status| status.is_some())
        .map_err(|_| Error::TunnelStartFailed)
}

fn spawn_guard(work: impl FnOnce() + Send + 'static) -> std::io::Result<std::thread::JoinHandle<()>> {
    #[cfg(test)]
    if FAIL_GUARD.with(std::cell::Cell::get) {
        return Err(std::io::Error::other("synthetic guardian failure"));
    }
    std::thread::Builder::new()
        .name("native-tunnel-guard".into())
        .spawn(work)
}

#[cfg(test)]
thread_local! {
    static FAIL_TERMINATE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn terminate(child: &mut Child) -> Result<()> {
    #[cfg(test)]
    if FAIL_TERMINATE.with(std::cell::Cell::get) {
        return Err(Error::TunnelCleanupUncertain);
    }
    horizon_app_process::stop_child_group(child).map_err(|_| Error::TunnelCleanupUncertain)
}

fn cleanup(child: &mut Child, files: &mut Option<(VerifiedBinary, NamedTempFile)>) -> Result<()> {
    terminate(child)?;
    if let Some((binary, config)) = files.as_mut() {
        retire_files(binary, config)?;
    }
    files.take();
    Ok(())
}

fn retire_files(binary: &mut VerifiedBinary, config: &mut NamedTempFile) -> Result<()> {
    retire_paths(&[config.path(), binary.file.as_ref()])
}
fn retire_paths(paths: &[&Path]) -> Result<()> {
    for path in paths {
        std::fs::remove_file(path).map_err(|_| Error::TunnelCleanupUncertain)?;
        let parent = path.parent().ok_or(Error::TunnelCleanupUncertain)?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| Error::TunnelCleanupUncertain)?;
    }
    Ok(())
}

fn spawn(
    binary: &mut VerifiedBinary,
    key: &str,
    only: &str,
    id: Uuid,
    deadline: Instant,
    journal: &impl Fn(ProcessRecord<'_>) -> Result<()>,
) -> Result<(Child, NamedTempFile)> {
    let Ok(mut config) = tempfile::Builder::new().suffix(".yml").tempfile() else {
        binary.file.disable_cleanup(true);
        retire_paths(&[binary.file.as_ref()])?;
        return Err(Error::TunnelStartFailed);
    };
    // Hold exact paths on every failure, including before exec; only explicit retirement permits completion.
    binary.file.disable_cleanup(true);
    config.disable_cleanup(true);
    let prepared = (|| {
        let escaped_key = Zeroizing::new(serde_json::to_string(key).map_err(|_| Error::TunnelStartFailed)?);
        let content = Zeroizing::new(format!("key: {}\n", escaped_key.as_str()));
        config
            .write_all(content.as_bytes())
            .and_then(|()| config.flush())
            .map_err(|_| Error::TunnelStartFailed)?;
        journal(ProcessRecord {
            id,
            pid: None,
            binary: binary.file.as_ref(),
            config: config.path(),
        })
    })();
    if let Err(error) = prepared {
        retire_files(binary, &mut config)?;
        return Err(error);
    }
    let mut command = Command::new(&binary.file);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .args(["--config-file"])
        .arg(config.path())
        .args([
            "--local-identifier",
            &local_identifier(id),
            "--only",
            only,
            "--disable-dashboard",
            "--disable-proxy-discovery",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match spawn_executable(&mut command, deadline) {
        Ok(child) => child,
        Err(error) => {
            retire_files(binary, &mut config)?;
            return Err(error);
        }
    };
    if let Err(error) = journal(ProcessRecord {
        id,
        pid: Some(child.id()),
        binary: binary.file.as_ref(),
        config: config.path(),
    }) {
        terminate(&mut child)?;
        retire_files(binary, &mut config)?;
        return Err(error);
    }
    Ok((child, config))
}

fn spawn_executable(command: &mut Command, deadline: Instant) -> Result<Child> {
    for attempt in 0..10 {
        if Instant::now() >= deadline {
            return Err(Error::TunnelStartFailed);
        }
        match command.spawn() {
            Ok(child) => return Ok(child),
            // Concurrent fork/exec can briefly inherit a writable staging FD.
            // ETXTBSY guarantees exec did not begin; retry no other failure.
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 9 => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return Err(Error::TunnelStartFailed),
        }
    }
    Err(Error::TunnelStartFailed)
}

fn wait_ready(child: &mut Child, deadline: Instant) -> Result<()> {
    let Some(mut stdout) = child.stdout.take() else {
        return Err(Error::TunnelStartFailed);
    };
    let (ready_send, ready_receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("native-tunnel-output".into())
        .spawn(move || {
            let mut line = Zeroizing::new(Vec::new());
            let mut byte = [0];
            while stdout.read(&mut byte).is_ok_and(|n| n != 0) {
                if byte[0] == b'\n' {
                    if line
                        .windows(b"You can now access your local server".len())
                        .any(|v| v == b"You can now access your local server")
                    {
                        let _ = ready_send.send(());
                    }
                    line.zeroize();
                } else if line.len() < 8192 {
                    line.push(byte[0]);
                }
            }
        })
        .map_err(|_| Error::TunnelStartFailed)?;
    if ready_receive
        .recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(30)),
        )
        .is_err()
    {
        return Err(Error::TunnelStartFailed);
    }
    if Instant::now() >= deadline || exited(child)? {
        return Err(Error::TunnelStartFailed);
    }
    Ok(())
}

#[cfg(unix)]
fn binary_file(path: &Path) -> Result<File> {
    let flags = rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC;
    rustix::fs::open(path, flags, rustix::fs::Mode::empty())
        .map(File::from)
        .map_err(|_| Error::TunnelBinaryRejected)
}

#[cfg(not(unix))]
fn binary_file(_path: &Path) -> Result<File> {
    Err(Error::TunnelBinaryRejected)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;

    fn fixture() -> VerifiedBinary {
        script(b"#!/bin/sh\necho 'You can now access your local servers'\nexec sleep 60\n")
    }

    fn script(bytes: &[u8]) -> VerifiedBinary {
        let mut source = NamedTempFile::new().unwrap();
        source.write_all(bytes).unwrap();
        let digest = Sha256::digest(bytes)
            .iter()
            .flat_map(|byte| [hex(byte >> 4), hex(byte & 15)])
            .collect::<String>();
        VerifiedBinary::capture(source.path(), &digest).unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transient_executable_busy_cannot_duplicate_a_tunnel_start() {
        let directory = tempfile::tempdir().unwrap();
        let counter = directory.path().join("starts");
        let binary = script(
            format!(
                "#!/bin/sh\nprintf x >> '{}'\necho 'You can now access your local servers'\nexec sleep 60\n",
                counter.display()
            )
            .as_bytes(),
        );
        let writable = std::fs::OpenOptions::new().write(true).open(&binary.file).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            drop(writable);
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut tunnel = Tunnel::start(
            binary,
            "synthetic-key",
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
            Uuid::new_v4(),
            Duration::from_secs(3),
            |_| Ok(()),
        )
        .unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(counter).unwrap(), b"x");
        tunnel.close().unwrap();
    }

    #[test]
    fn expired_journal_callback_cannot_start_a_tunnel_binary() {
        let directory = tempfile::tempdir().unwrap();
        let counter = directory.path().join("starts");
        let binary = script(
            format!(
                "#!/bin/sh\nprintf x >> '{}'\necho 'You can now access your local servers'\nexec sleep 60\n",
                counter.display()
            )
            .as_bytes(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let outcome = Tunnel::start(
            binary,
            "synthetic-key",
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
            Uuid::new_v4(),
            Duration::from_millis(10),
            |_| {
                std::thread::sleep(Duration::from_millis(30));
                Ok(())
            },
        );
        assert!(matches!(outcome, Err(Error::TunnelStartFailed)));
        assert!(!counter.exists());
    }

    #[test]
    fn only_declared_loopback_ports_are_accepted() {
        for address in ["0.0.0.0:8080", "192.168.1.1:8080", "127.0.0.1:0", "127.0.0.2:8080"] {
            assert_eq!(
                allowlist(&[LocalPort {
                    address: address.parse().unwrap(),
                    tls: false
                }])
                .err(),
                Some(Error::TunnelPortRefused)
            );
        }
        let only = allowlist(&[LocalPort {
            address: "127.0.0.1:8080".parse().unwrap(),
            tls: false,
        }])
        .unwrap();
        assert_eq!(only, "127.0.0.1,8080,0,bs-local.com,8080,0,localhost,8080,0");
        assert!(!only.contains('*'));
    }

    #[test]
    fn owned_tunnel_expires_and_removes_its_key_file_without_a_close_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let records = Mutex::new(Vec::<(Option<u32>, PathBuf, PathBuf)>::new());
        let mut tunnel = Tunnel::start(
            fixture(),
            "synthetic-tunnel-key",
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
            Uuid::new_v4(),
            Duration::from_millis(100),
            |record| {
                assert_eq!(record.config.extension().and_then(|value| value.to_str()), Some("yml"));
                assert_eq!(
                    std::fs::read_to_string(record.config).unwrap(),
                    "key: \"synthetic-tunnel-key\"\n"
                );
                records
                    .lock()
                    .unwrap()
                    .push((record.pid, record.binary.to_owned(), record.config.to_owned()));
                Ok(())
            },
        )
        .unwrap();
        assert!(tunnel.status().unwrap().ready);
        assert!(
            !serde_json::to_string(&tunnel.status().unwrap())
                .unwrap()
                .contains("synthetic-tunnel-key")
        );
        std::thread::sleep(Duration::from_millis(250));
        assert!(!tunnel.status().unwrap().ready);
        tunnel.close().unwrap();
        tunnel.close().unwrap();
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert!(records[0].0.is_none());
        assert!(records[1].0.is_some());
        assert!(!records[1].1.exists());
        assert!(!records[1].2.exists());
    }

    #[test]
    fn journal_failure_after_spawn_releases_owned_files_and_process() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let paths = Mutex::new(Vec::<PathBuf>::new());
        let outcome = Tunnel::start(
            fixture(),
            "synthetic-tunnel-key",
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
            Uuid::new_v4(),
            Duration::from_secs(1),
            |record| {
                paths
                    .lock()
                    .unwrap()
                    .extend([record.binary.to_owned(), record.config.to_owned()]);
                if record.pid.is_some() {
                    Err(Error::ProviderFailed)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(outcome.err(), Some(Error::ProviderFailed));
        assert!(paths.lock().unwrap().iter().all(|path| !path.exists()));
    }
    #[test]
    fn exited_leader_cannot_leave_a_term_resistant_descendant() {
        let folder = tempfile::tempdir().unwrap();
        let pid_file = folder.path().join("child.pid");
        let mut binary = script(
            format!(
                "#!/bin/sh\n(trap '' TERM; exec sleep 60) &\necho $! > '{}'\nexit 0\n",
                pid_file.display()
            )
            .as_bytes(),
        );
        let (mut child, mut config) = spawn(
            &mut binary,
            "synthetic-key",
            "localhost,8080,0",
            Uuid::new_v4(),
            Instant::now() + Duration::from_secs(3),
            &|_| Ok(()),
        )
        .unwrap();
        while !exited(&mut child).unwrap() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let descendant: i32 = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
        terminate(&mut child).unwrap();
        retire_files(&mut binary, &mut config).unwrap();
        let pid = rustix::process::Pid::from_raw(descendant).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            // A retained orphan zombie cannot execute, even before its parent reaps it.
            #[cfg(target_os = "linux")]
            let zombie = std::fs::read_to_string(format!("/proc/{descendant}/stat"))
                .is_ok_and(|value| value.split(") ").nth(1).is_some_and(|tail| tail.starts_with('Z')));
            #[cfg(target_os = "macos")]
            let zombie = std::process::Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &descendant.to_string()])
                .output()
                .is_ok_and(|output| {
                    output.status.success()
                        && std::str::from_utf8(&output.stdout).is_ok_and(|state| state.trim().starts_with('Z'))
                });
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            let zombie = false;
            let absent = match rustix::process::test_kill_process(pid) {
                Ok(()) => false,
                Err(rustix::io::Errno::SRCH) => true,
                Err(error) => panic!("cannot verify owned descendant: {error}"),
            };
            if zombie || absent {
                break;
            }
            assert!(Instant::now() < deadline, "owned descendant survived cleanup");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn tunnel_does_not_inherit_parent_only_credentials() {
        const MARKER: &str = "HORIZON_SYNTHETIC_PARENT_SECRET";
        if std::env::var_os(MARKER).is_none() {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tunnel::tests::tunnel_does_not_inherit_parent_only_credentials",
                ])
                .env(MARKER, "synthetic-parent-secret")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let binary = script(b"#!/bin/sh\n[ -z \"$HORIZON_SYNTHETIC_PARENT_SECRET\" ] || exit 1\necho 'You can now access your local servers'\nexec sleep 60\n");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut tunnel = Tunnel::start(
            binary,
            "synthetic-key",
            vec![LocalPort {
                address: listener.local_addr().unwrap(),
                tls: false,
            }],
            Uuid::new_v4(),
            Duration::from_secs(5),
            |_| Ok(()),
        )
        .unwrap();
        assert!(tunnel.status().unwrap().ready);
        tunnel.close().unwrap();
    }
    #[test]
    fn every_postspawn_startup_failure_preserves_files_when_cleanup_is_uncertain() {
        for stage in ["journal", "readiness", "guardian"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let observed = Mutex::new(None::<(u32, PathBuf, PathBuf)>);
            FAIL_TERMINATE.set(true);
            FAIL_GUARD.set(stage == "guardian");
            let binary = if stage == "readiness" {
                script(b"#!/bin/sh\nexit 0\n")
            } else {
                fixture()
            };
            let result = Tunnel::start(
                binary,
                "synthetic-key",
                vec![LocalPort {
                    address: listener.local_addr().unwrap(),
                    tls: false,
                }],
                Uuid::new_v4(),
                Duration::from_secs(3),
                |record| {
                    if let Some(pid) = record.pid {
                        *observed.lock().unwrap() = Some((pid, record.binary.to_owned(), record.config.to_owned()));
                        if stage == "journal" {
                            return Err(Error::ProviderFailed);
                        }
                    }
                    Ok(())
                },
            );
            FAIL_TERMINATE.set(false);
            FAIL_GUARD.set(false);
            assert_eq!(result.err(), Some(Error::TunnelCleanupUncertain));
            let (pid, binary, config) = observed.into_inner().unwrap().unwrap();
            assert!(
                binary.is_file() && config.is_file(),
                "{stage} discarded private cleanup files"
            );
            // Fixture-only cleanup: these exact children were spawned above and remain waitable.
            let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
            rustix::process::kill_process_group(pid, rustix::process::Signal::KILL).unwrap();
            rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty()).unwrap();
            std::fs::remove_file(binary).unwrap();
            std::fs::remove_file(config).unwrap();
        }
    }
}
