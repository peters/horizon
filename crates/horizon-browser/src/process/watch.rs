//! The watch that ends one browser when the Horizon that started it is gone.
//!
//! A window that a person closes runs the exit cleanup of Horizon, and that
//! cleanup ends every browser. A Horizon that the system stops with `SIGTERM` or
//! `SIGINT`, and one that loses its process to a crash or to `SIGKILL`, runs no
//! cleanup at all: its browser would stay alive with no window, hold its profile
//! and keep the pages of the person open.
//!
//! One small child of Horizon is the promise that this cannot happen, the same way
//! a recorder of speech input is held by a pipe:
//!
//! 1. It holds the read end of a pipe whose write end belongs to this process.
//! 2. It waits on that pipe, and the pipe gives it end-of-file when this process
//!    is gone, whatever ended it: the kernel closes every descriptor of a dead
//!    process.
//! 3. It sends `SIGTERM` to the process group of the browser, and `SIGKILL` one
//!    second later to a group that still holds a process.
//!
//! The watch of a browser that this process ends itself stops with it, so a normal
//! quit leaves no child behind. A browser that Horizon did not start has no watch,
//! because the watch is made at the moment that Horizon starts the browser.

use std::process::{Child, ChildStdin, Command, Stdio};

#[cfg(all(test, unix))]
use std::time::Duration;

/// The group of the browser is signalled `SIGTERM` first, and this wait follows
/// before the group is killed.
const KILL_DELAY_SECONDS: u32 = 1;

/// How long a test waits for a process that a watch ends.
#[cfg(all(test, unix))]
const REAP_BOUND: Duration = Duration::from_secs(5);

/// The watch of one browser: its own child, and the write end of the pipe that
/// tells that child that this process is alive.
#[derive(Debug)]
pub(super) struct BrowserWatch {
    child: Option<Child>,
    /// The write end of the pipe. Dropping it tells the watcher that this process
    /// is gone, which is what the kernel does to it when the process dies.
    beacon: Option<ChildStdin>,
}

impl BrowserWatch {
    /// Starts the watch of the browser whose process id is `browser`.
    ///
    /// The browser runs in its own process group, whose id is that process id, so
    /// the watch ends its helpers with it.
    ///
    /// # Errors
    /// Returns why the child of the watch cannot start. The caller logs it and
    /// keeps the browser: a browser without a watch is the behavior of every
    /// Horizon before this one.
    pub(super) fn start(browser: u32) -> std::io::Result<Self> {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(watch_script())
            .arg("horizon-browser-watch")
            .arg(browser.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let beacon = child.stdin.take();
        Ok(Self {
            child: Some(child),
            beacon,
        })
    }

    /// Ends the watch of a browser that this process ends itself. The browser is
    /// already gone or going, so the watch has nothing left to do, and it is not
    /// given the second of its own wait.
    pub(super) fn stop(&mut self) {
        self.beacon.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Lets the pipe end without ending the watch, which is what the kernel does to
    /// every descriptor of a process that dies. A test uses this to be the death of
    /// the parent, so it reads the same path as a `SIGTERM`, a `SIGINT` and a
    /// `SIGKILL` of Horizon.
    #[cfg(all(test, unix))]
    pub(super) fn release(mut self) {
        self.beacon.take();
        if let Some(mut child) = self.child.take() {
            // The watcher ends by itself after its work, and a thread of the test
            // reaps it, so the test leaves no zombie.
            let _ = std::thread::Builder::new()
                .name("watch-reaper".to_owned())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }

    /// Whether the watch left no child of its own.
    #[cfg(all(test, unix))]
    pub(super) fn has_no_child(&self) -> bool {
        self.child.is_none()
    }
    /// The longest time that a test waits for a process that a watch ends.
    #[cfg(all(test, unix))]
    pub(super) fn reap_bound() -> Duration {
        REAP_BOUND
    }
}

impl Drop for BrowserWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The script of the watch, and the reason for its shape.
///
/// The process id of the browser is a positional parameter, so no value of a
/// process is read as shell text. The watch takes the browser's own process group
/// first, because Chromium's zygotes, GPU and network helpers are members of it,
/// and it takes the process id alone when that group is gone or when the system
/// has no such group. `/bin/kill` names the negative process id after `--`, so a
/// system whose `kill` reads a dash as another option cannot miss it.
fn watch_script() -> String {
    format!(
        "while read -r _; do :; done\n\
         /bin/kill -TERM -- -\"$1\" 2>/dev/null || /bin/kill -TERM \"$1\" 2>/dev/null\n\
         sleep {KILL_DELAY_SECONDS}\n\
         /bin/kill -KILL -- -\"$1\" 2>/dev/null || /bin/kill -KILL \"$1\" 2>/dev/null\n"
    )
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::process::CommandExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{BrowserWatch, KILL_DELAY_SECONDS};

    /// One stand-in program in place of a browser. It ignores the termination
    /// signal, as a browser with a stuck helper does, and it writes the process id
    /// of its own helper to `$1`.
    const STAND_IN: &str = "\
#!/bin/sh
trap '' TERM
/bin/sleep 30 &
printf '%s\\n' \"$!\" > \"$1\"
wait
";

    /// Writes the stand-in with a `/bin/sh` child, which is what a crate without
    /// the helper of `horizon-core` does: no descriptor of the file stays open in
    /// this process, so a fork of another test thread cannot copy it and an `exec`
    /// never answers `Text file busy`.
    fn stand_in(directory: &Path) -> PathBuf {
        let script = directory.join("stand-in.sh");
        let status = Command::new("/bin/sh")
            .args(["-c", "printf '%s' \"$2\" > \"$1\" && chmod 700 \"$1\"", "stand-in"])
            .arg(&script)
            .arg(STAND_IN)
            .status()
            .expect("the child that writes the stand-in");
        assert!(status.success(), "the stand-in is written");
        script
    }

    /// Starts the stand-in the way a browser starts: in a process group of its own,
    /// with a helper in that group.
    fn start_stand_in(script: &Path, directory: &Path) -> (Child, String) {
        let helper_pid = directory.join("helper.pid");
        let mut command = Command::new(script);
        command
            .arg(&helper_pid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("the stand-in of a browser");
        let deadline = Instant::now() + Duration::from_secs(5);
        let helper = loop {
            if let Ok(text) = fs::read_to_string(&helper_pid)
                && !text.trim().is_empty()
            {
                break text.trim().to_owned();
            }
            assert!(
                Instant::now() < deadline,
                "the helper of the stand-in reported its process id"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        (child, helper)
    }

    fn process_exists(pid: &str) -> bool {
        Command::new("/bin/kill")
            .args(["-0", "--", pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Waits for a process to be gone, and answers whether it went inside the bound
    /// of the watch and its kill.
    fn gone(pid: &str) -> bool {
        let bound = BrowserWatch::reap_bound() + Duration::from_secs(u64::from(KILL_DELAY_SECONDS));
        let deadline = Instant::now() + bound;
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn wait_for_exit(child: &mut Child) -> bool {
        let deadline = Instant::now() + BrowserWatch::reap_bound();
        while Instant::now() < deadline {
            if child.try_wait().expect("the status of the stand-in").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn kill_group(pid: &str) {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// The end of the pipe is the only sign that the watch needs. A `SIGTERM`, a
    /// `SIGINT` and a `SIGKILL` of Horizon all end with the kernel closing the
    /// write end of this pipe, so dropping it is the death of the parent for each
    /// of them.
    #[test]
    fn the_watch_ends_the_browser_and_its_helpers_when_its_pipe_ends() {
        let directory = tempfile::tempdir().expect("a directory");
        let script = stand_in(directory.path());
        let (mut browser, helper) = start_stand_in(&script, directory.path());
        let watch = BrowserWatch::start(browser.id()).expect("the watch of the browser");

        watch.release();
        assert!(
            gone(&helper),
            "the helper of the browser ended with the group of its browser"
        );
        assert!(wait_for_exit(&mut browser), "the browser ended with the watch");
    }

    /// A watch that the process ends itself leaves the browser that it owns, so a
    /// normal quit stops the watch and only the cleanup of the browser ends it.
    #[test]
    fn a_watch_that_the_process_stops_leaves_the_browser_and_no_child() {
        let directory = tempfile::tempdir().expect("a directory");
        let script = stand_in(directory.path());
        let (mut browser, helper) = start_stand_in(&script, directory.path());
        let mut watch = BrowserWatch::start(browser.id()).expect("the watch of the browser");
        watch.stop();

        assert!(watch.has_no_child(), "the watch reaped its own child");
        assert!(process_exists(&helper), "a browser that Horizon keeps stays alive");
        assert!(
            browser.try_wait().expect("the status of the browser").is_none(),
            "the browser of a watch that stopped still runs"
        );
        kill_group(&browser.id().to_string());
        let _ = browser.wait();
    }
}
