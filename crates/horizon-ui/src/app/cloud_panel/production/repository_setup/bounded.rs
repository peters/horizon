//! Commands of a setup that must end in time. The setup agent can write the repository's
//! git configuration, and an `include` there that leads to a FIFO holds any git command
//! that reads it; a command run here is ended, with everything it started, once its time
//! is up, so no such command holds Use, Stop, a session switch or Horizon's exit.
use std::io::{Read as _, Write as _};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long one git command of a setup may take.
pub(super) const GIT: Duration = Duration::from_secs(60);

/// How long a git listing of the whole repository may take.
pub(super) const LISTING: Duration = Duration::from_mins(5);

/// The most a command may write on standard error that Horizon keeps.
const ERROR_LIMIT: u64 = 64 * 1024;

/// The most a command may write on standard output: a setup's git commands name hashes,
/// paths and the files of `.horizon`, and a longer answer, such as a configuration value
/// the agent made huge, is refused instead of read into memory.
const OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

/// Runs `command` with `input` on its standard input for at most `limit`: its exit status
/// and output, or why it gave none, as when it did not end in time or wrote more than
/// [`OUTPUT_LIMIT`] bytes.
pub(super) fn output(mut command: Command, input: Option<Vec<u8>>, limit: Duration) -> Result<Output, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    let writer = input.zip(child.stdin.take()).map(|(bytes, mut stdin)| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        })
    });
    let stdout = child.stdout.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(OUTPUT_LIMIT + 1).read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr = child.stderr.take().map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(ERROR_LIMIT).read_to_end(&mut bytes);
            bytes
        })
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                end(&mut child);
                return Err(format!("{program} did not end within {} s", limit.as_secs()));
            }
        }
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout = stdout.and_then(|reader| reader.join().ok()).unwrap_or_default();
    if stdout.len() as u64 > OUTPUT_LIMIT {
        return Err(format!("{program} wrote more than {OUTPUT_LIMIT} bytes"));
    }
    Ok(Output {
        status,
        stdout,
        stderr: stderr.and_then(|reader| reader.join().ok()).unwrap_or_default(),
    })
}

/// Ends `child` with everything in its process group, and reaps it.
pub(super) fn end(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Ends the process group of `child` once `limit` has passed, unless the returned guard
/// is dropped first; for a command whose output is read as it arrives. The child stays
/// unreaped until its owner waits for it, so its id names no other process meanwhile.
pub(super) fn watch(child: &std::process::Child, limit: Duration) -> Watch {
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let expired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (flag, timed_out) = (done.clone(), expired.clone());
    let id = child.id();
    std::thread::spawn(move || {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if flag.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !flag.load(std::sync::atomic::Ordering::Acquire) {
            timed_out.store(true, std::sync::atomic::Ordering::Release);
            #[cfg(unix)]
            if let Some(group) = i32::try_from(id).ok().and_then(rustix::process::Pid::from_raw) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            }
            #[cfg(not(unix))]
            let _ = id;
        }
    });
    Watch { done, expired }
}

/// The guard [`watch`] returns.
pub(super) struct Watch {
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    expired: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Watch {
    /// Stops watching; whether the time ran out first and the command was ended.
    pub(super) fn finish(self) -> bool {
        self.done.store(true, std::sync::atomic::Ordering::Release);
        self.expired.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_command_that_does_not_end_in_time_is_ended() {
        let mut sleep = Command::new("sh");
        sleep.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let error = output(sleep, None, Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("did not end"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let mut cat = Command::new("cat");
        cat.arg("-");
        let echoed = output(cat, Some(b"input".to_vec()), Duration::from_secs(5)).unwrap();
        assert_eq!(echoed.stdout, b"input");
    }

    #[test]
    fn a_command_that_writes_too_much_is_refused() {
        let mut endless = Command::new("sh");
        endless.args(["-c", "yes"]);
        let started = Instant::now();
        let error = output(endless, None, Duration::from_secs(60)).unwrap_err();
        assert!(error.contains("wrote more than"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "it ends without waiting out its time"
        );
    }
}
