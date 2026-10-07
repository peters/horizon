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
    exited(child)?; // Establish retained child ownership before any reusable-PID signalling.
    match rustix::process::getpgid(Some(group)) {
        Ok(actual) if actual == group => (),
        // XNU no longer exposes a zombie's group through getpgid. A fresh positive
        // WNOWAIT result keeps this exact child PID reserved until group cleanup.
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::SRCH) if exited(child)? && mac_group::retained_leader(group)? => (),
        _ => return Err(Error::CleanupUncertain),
    }
    child.stdin.take();
    acknowledge_signal(child, group, signal(group, Signal::TERM))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !exited(child)? && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // An exited leader can still have descendants; finish group cleanup before reap.
    acknowledge_signal(child, group, signal(group, Signal::KILL))?;
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
            #[cfg(target_os = "macos")]
            Err(rustix::io::Errno::PERM) if group_cannot_execute(group)? => return Ok(()),
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

#[cfg(unix)]
fn acknowledge_signal(child: &Child, _group: rustix::process::Pid, result: rustix::io::Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::SRCH) if exited(child)? => Ok(()),
        // XNU skips zombies and returns EPERM when no live recipient remains. This is not
        // permission to ignore EPERM for a live child or any executing group member.
        #[cfg(target_os = "macos")]
        Err(rustix::io::Errno::PERM) if exited(child)? && group_cannot_execute(_group)? => Ok(()),
        Err(_) => Err(Error::CleanupUncertain),
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(3) => (),
            Err(_) => return Err(Error::CleanupUncertain),
        }
    }
    Ok(true)
}

#[cfg(target_os = "macos")]
fn group_cannot_execute(group: rustix::process::Pid) -> Result<bool> {
    mac_group::cannot_execute(group)
}

#[cfg(target_os = "macos")]
#[path = "group/mac.rs"]
mod mac_group;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    #[test]
    fn an_exited_lone_leader_is_cleaned_before_its_reserved_pid_is_reaped() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !exited(&child).unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(exited(&child).unwrap());
        terminate(&mut child).unwrap();
    }

    #[test]
    fn an_exited_inherited_group_child_cannot_acknowledge_descendant_cleanup() {
        use std::io::BufRead;
        let mut child = Command::new("/bin/sh")
            .args(["-c", "(trap '' TERM; exec sleep 60) & echo $!; exit 0"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut descendant = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut descendant)
            .unwrap();
        let descendant = rustix::process::Pid::from_raw(descendant.trim().parse().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !exited(&child).unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = terminate_with(&mut child, |_, _| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        // Stop only the test-owned descendant, never its inherited shared group.
        rustix::process::kill_process(descendant, rustix::process::Signal::KILL).unwrap();
        child.wait().unwrap();
        assert_eq!(result, Err(Error::CleanupUncertain));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_reaped_child_cannot_authorize_any_group_signal() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .process_group(0)
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert_eq!(
            terminate_with(&mut child, |_, _| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
            Err(Error::CleanupUncertain)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn a_normal_child_cannot_authorize_signalling_an_unowned_group() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        assert_eq!(
            terminate_with(&mut child, |_, _| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
            Err(Error::CleanupUncertain)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
