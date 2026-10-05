use crate::{Error, Result};
use std::process::Child;
use std::time::{Duration, Instant};

#[cfg(unix)]
pub(crate) fn exited(child: &Child) -> Result<bool> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    waitid(
        WaitId::Pid(pid(child)?),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map(|value| value.is_some())
    .map_err(|_| Error::CleanupUncertain)
}

#[cfg(unix)]
pub(crate) fn child_exit_success(child: &Child) -> Result<bool> {
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
pub(crate) fn terminate(child: &mut Child) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

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
