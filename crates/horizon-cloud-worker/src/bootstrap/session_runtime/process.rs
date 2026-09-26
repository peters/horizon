//! A single supervisor owns discovery, signalling and reaping. Unreaped direct
//! children cannot have their PID reused before we acquire a pidfd.
use super::super::store::invalid;
use rustix::process::{Pid, PidfdFlags, Signal, WaitOptions};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub pid: i32,
    boot: String,
    start: u64,
    namespace: u64,
}
impl Identity {
    pub fn capture(pid: i32) -> io::Result<Self> {
        let (start, _) = state(pid)?;
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        if boot.len() != 37 {
            return Err(invalid());
        }
        Ok(Self {
            pid,
            boot,
            start,
            namespace: fs::metadata("/proc/self/ns/pid")?.ino(),
        })
    }
    pub fn current() -> io::Result<Self> {
        Self::capture(i32::try_from(std::process::id()).map_err(|_| invalid())?)
    }
    pub fn alive(&self) -> bool {
        Self::capture(self.pid).is_ok_and(|id| id == *self)
            && state(self.pid).is_ok_and(|(_, status)| !matches!(status, 'Z' | 'X'))
    }
}
fn state(pid: i32) -> io::Result<(u64, char)> {
    if pid <= 0 {
        return Err(invalid());
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = stat.rsplit_once(") ").ok_or_else(invalid)?.1;
    let parts: Vec<_> = tail.split_whitespace().collect();
    Ok((
        parts.get(19).ok_or_else(invalid)?.parse().map_err(|_| invalid())?,
        parts.first().and_then(|s| s.chars().next()).ok_or_else(invalid)?,
    ))
}
fn children() -> io::Result<Vec<Pid>> {
    let path = format!("/proc/self/task/{}/children", std::process::id());
    let mut bytes = String::new();
    fs::File::open(path)?.take(65537).read_to_string(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(invalid());
    }
    bytes
        .split_whitespace()
        .map(|p| p.parse().ok().and_then(Pid::from_raw).ok_or_else(invalid))
        .collect()
}

pub(super) fn reap() -> io::Result<bool> {
    loop {
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(false),
            Err(rustix::io::Errno::CHILD) => return Ok(true),
            Err(error) => return Err(error.into()),
        }
    }
}

/// Only the still-intact single-threaded subreaper can prove this result. A
/// restarted controller never signals saved PIDs or fabricates an empty tree.
pub(super) fn stop(timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        for pid in children()? {
            let fd = rustix::process::pidfd_open(pid, PidfdFlags::empty())?;
            match rustix::process::pidfd_send_signal(&fd, Signal::KILL) {
                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if reap()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(invalid());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_identity_rejects_changed_boot_start_namespace_and_invalid_pid() {
        let identity = Identity::current().unwrap();
        assert!(identity.alive());
        let mut changed = identity.clone();
        changed.start += 1;
        assert!(!changed.alive());
        let mut changed = identity.clone();
        changed.namespace += 1;
        assert!(!changed.alive());
        let mut changed = identity.clone();
        changed.boot = "foreign boot".into();
        assert!(!changed.alive());
        let mut changed = identity;
        changed.pid = -1;
        assert!(!changed.alive());
    }
}
