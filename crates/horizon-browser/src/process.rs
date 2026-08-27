//! Headless Chrome lifecycle: binary discovery, spawn, ws-url capture, kill.

mod control;

#[cfg(any(windows, test))]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Child;
use std::process::{ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

#[cfg(windows)]
use process_wrap::std::{ChildWrapper, CommandWrap, JobObject};
use thiserror::Error;

use crate::AutomationDisclosurePolicy;
use crate::cdp::parse_devtools_ws_url;

pub(crate) use control::{ChromeProcessControl, ServiceProcess};

#[cfg(windows)]
pub(super) type ProcessChild = Box<dyn ChildWrapper>;
#[cfg(not(windows))]
pub(super) type ProcessChild = Child;

#[derive(Error, Debug)]
pub enum ChromeError {
    #[error("no Chrome/Chromium binary found (set browser.command in the config)")]
    NoBinary,
    #[error("failed to spawn chrome: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("chrome exited before its DevTools endpoint appeared (stderr: {0})")]
    NoDevtools(String),
    #[error("timed out waiting for the DevTools endpoint (stderr: {0})")]
    DevtoolsTimeout(String),
    #[error("browser.extra_args cannot override engine-managed Chrome switch `{0}`")]
    ProtectedExtraArg(String),
}

pub type Result<T> = std::result::Result<T, ChromeError>;

/// Candidate binary names, in preference order, scanned on `PATH`.
pub const BINARY_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "brave-browser",
    "microsoft-edge",
];

/// Windows candidates: `Path::is_file` does not apply `PATHEXT`, so the
/// `.exe` names and standard install locations are checked explicitly.
#[cfg(windows)]
pub const WINDOWS_BINARY_CANDIDATES: &[&str] = &["chrome.exe", "msedge.exe", "chromium.exe"];

/// Parameters for launching one headless Chrome instance.
pub struct ChromeLaunch {
    /// Executable to run (absolute path or bare name resolved on PATH).
    pub command: String,
    /// Per-panel profile directory. Created if missing.
    pub profile_dir: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Extra CLI arguments appended verbatim (config `browser.extra_args`).
    /// Engine-managed profile, `DevTools`, and disclosure switches are rejected.
    pub extra_args: Vec<String>,
    pub automation_disclosure: AutomationDisclosurePolicy,
}

/// A running headless `Chrome` process plus its captured `DevTools` endpoint.
pub struct ChromeProcess {
    child: Arc<Mutex<ProcessChild>>,
    control: ChromeProcessControl,
    /// Cached once the child has been reaped, so repeated
    /// [`ChromeProcess::child_status`]/[`ChromeProcess::kill`] calls stay cheap.
    exit_status: Option<std::process::ExitStatus>,
    ws_url_rx: mpsc::Receiver<String>,
    ws_url: Option<String>,
    stderr_tail: std::sync::Arc<std::sync::Mutex<String>>,
}

impl ChromeProcess {
    /// Spawn the configured browser with a private profile dir.
    ///
    /// # Errors
    /// Fails when the binary is missing or the process cannot start.
    pub(crate) fn spawn(launch: &ChromeLaunch, control: ChromeProcessControl) -> Result<Self> {
        let command = resolve_binary(&launch.command)?;
        prepare_profile_dir(&launch.profile_dir)?;
        let args = launch_args(launch)?;

        let mut command = Command::new(command);
        command.args(&args).stdout(Stdio::null()).stderr(Stdio::piped());
        // Give Chrome an isolated process group so emergency cleanup can
        // terminate helpers as well as the browser parent on Unix. On Windows,
        // `spawn_process` assigns the child tree to a retained Job Object.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = spawn_process(command)?;
        let Some(stderr) = take_stderr(&mut child) else {
            let _ = kill_and_reap(&mut child, Duration::from_secs(3));
            return Err(std::io::Error::other("failed to capture chrome stderr").into());
        };

        let (ws_url_tx, ws_url_rx) = mpsc::channel();
        let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_tail = tail.clone();
        let stderr_reader = std::thread::Builder::new().name("chrome-stderr".into()).spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Some(ws) = parse_devtools_ws_url(&line) {
                    if ws_url_tx.send(ws).is_err() {
                        break;
                    }
                    continue;
                }
                // Keep a short stderr tail for diagnostics.
                let mut tail = tail.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 8 * 1024 {
                    // Keep the *newest* half, cut on a char boundary
                    // (a blind byte split can panic on UTF-8).
                    let cut = tail.floor_char_boundary(4 * 1024);
                    tail.drain(..cut);
                }
            }
        });
        if let Err(error) = stderr_reader {
            let _ = kill_and_reap(&mut child, Duration::from_secs(3));
            return Err(error.into());
        }

        let child = Arc::new(Mutex::new(child));
        control.register(&child);
        Ok(Self {
            child,
            control,
            exit_status: None,
            ws_url_rx,
            ws_url: None,
            stderr_tail,
        })
    }

    /// Block until the browser reports its `DevTools` endpoint, the caller
    /// cancels startup, or `timeout` elapses.
    ///
    /// # Errors
    /// Fails on timeout or when the process exits before reporting.
    pub fn wait_ws_url(&mut self, timeout: Duration, mut cancelled: impl FnMut() -> bool) -> Result<Option<String>> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if cancelled() {
                return Ok(None);
            }
            if let Some(url) = self.ws_url.clone() {
                return Ok(Some(url));
            }
            match self.ws_url_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(url) => {
                    self.ws_url = Some(url.clone());
                    return Ok(Some(url));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.child_status().is_some() {
                        return Err(ChromeError::NoDevtools(self.stderr_tail_snapshot()));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ChromeError::NoDevtools(self.stderr_tail_snapshot()));
                }
            }
        }
        Err(ChromeError::DevtoolsTimeout(self.stderr_tail_snapshot()))
    }

    /// Poll the child for exit without blocking.
    #[must_use]
    pub fn child_status(&mut self) -> Option<std::process::ExitStatus> {
        if self.exit_status.is_some() {
            return self.exit_status;
        }
        let status = {
            let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            poll_and_cleanup_exited_tree(&mut child).ok().flatten()
        };
        self.exit_status = status;
        status
    }

    /// Kill the browser. Chrome's child processes (zygotes, GPU, network
    /// service) exit with their parent; a short reap wait follows.
    #[must_use]
    pub fn kill(&mut self) -> bool {
        if self.child_status().is_some() {
            return true;
        }
        let mut child = self.child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pid = child.id();
        match kill_and_reap(&mut child, Duration::from_secs(3)) {
            Ok(Some(status)) => {
                self.exit_status = Some(status);
                true
            }
            Ok(None) => {
                tracing::warn!(pid, "Chrome did not exit within the reap deadline");
                false
            }
            Err(error) => {
                tracing::warn!(pid, "failed to kill or reap Chrome: {error}");
                false
            }
        }
    }

    fn stderr_tail_snapshot(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .to_string()
    }
}

fn prepare_profile_dir(profile_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(profile_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn launch_args(launch: &ChromeLaunch) -> Result<Vec<String>> {
    validate_extra_args(&launch.extra_args, launch.automation_disclosure)?;
    let mut args = vec![
        "--headless=new".to_string(),
        // Port 0: the OS picks a free port; we learn it from stderr.
        "--remote-debugging-port=0".to_string(),
        "--remote-debugging-address=127.0.0.1".to_string(),
        format!("--user-data-dir={}", launch.profile_dir.display()),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        format!("--window-size={},{}", launch.width, launch.height),
        "--disable-features=TranslateUI".to_string(),
    ];
    if launch.automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals {
        args.push("--disable-blink-features=AutomationControlled".to_string());
    }
    // A disposable per-panel profile must never block startup on a macOS
    // keychain creation prompt.
    #[cfg(target_os = "macos")]
    args.push("--use-mock-keychain".to_string());
    args.extend(launch.extra_args.iter().cloned());
    args.push("about:blank".to_string());
    Ok(args)
}

fn validate_extra_args(extra_args: &[String], automation_disclosure: AutomationDisclosurePolicy) -> Result<()> {
    for argument in extra_args {
        let Some(switch) = argument
            .strip_prefix("--")
            .or_else(|| argument.strip_prefix('-'))
            .or_else(|| argument.strip_prefix('/'))
        else {
            continue;
        };
        let name = switch.split(['=', ':']).next().unwrap_or(switch).to_ascii_lowercase();
        let overrides_disclosure = automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals
            && matches!(
                name.as_str(),
                "enable-automation" | "disable-blink-features" | "enable-blink-features"
            );
        if name.starts_with("remote-debugging-")
            || matches!(name.as_str(), "remote-allow-origins" | "user-data-dir")
            || overrides_disclosure
        {
            return Err(ChromeError::ProtectedExtraArg(argument.clone()));
        }
    }
    Ok(())
}

pub(super) fn spawn_process(command: Command) -> std::io::Result<ProcessChild> {
    #[cfg(windows)]
    {
        let mut command = CommandWrap::from(command);
        command.wrap(JobObject);
        command.spawn()
    }
    #[cfg(not(windows))]
    {
        let mut command = command;
        command.spawn()
    }
}

pub(super) fn take_stderr(child: &mut ProcessChild) -> Option<ChildStderr> {
    #[cfg(windows)]
    {
        child.stderr().take()
    }
    #[cfg(not(windows))]
    {
        child.stderr.take()
    }
}

/// Kill and reap a child with deadline-bounded status polling. Windows uses a
/// retained Job Object so descendant completion is part of the reap result.
pub(super) fn kill_and_reap(
    child: &mut ProcessChild,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    if let Some(status) = poll_and_cleanup_exited_tree(child)? {
        return Ok(Some(status));
    }
    terminate_process_tree(child)?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = poll_and_cleanup_exited_tree(child)? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Reap an exited process leader and immediately terminate any descendants
/// still living in its owned process tree. `WebDriver` services can exit before
/// the browser they launched, leaving a locked profile behind unless the
/// process group is cleaned at the point the exit is observed.
pub(super) fn poll_and_cleanup_exited_tree(
    child: &mut ProcessChild,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let status = child.try_wait()?;
    if status.is_some() {
        cleanup_reaped_process_tree(child)?;
    }
    Ok(status)
}

#[cfg(unix)]
fn cleanup_reaped_process_tree(child: &mut ProcessChild) -> std::io::Result<()> {
    // This runs immediately after reaping the exact retained child handle.
    // The Unix process group remains addressable while an owned descendant is
    // alive, even after its leader has exited.
    terminate_process_tree(child)
}

#[cfg(windows)]
fn cleanup_reaped_process_tree(child: &mut ProcessChild) -> std::io::Result<()> {
    // The retained Job Object outlives the exited service leader. Terminate
    // and wait on that exact kernel-owned tree before reporting cleanup, so a
    // Firefox/Safari descendant cannot keep the profile locked.
    child.start_kill()?;
    child.wait().map(|_| ())
}

#[cfg(not(any(unix, windows)))]
fn cleanup_reaped_process_tree(_child: &mut ProcessChild) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn terminate_process_tree(child: &mut ProcessChild) -> std::io::Result<()> {
    let process_group = format!("-{}", child.id());
    let tree_killed = Command::new("/bin/kill")
        // GNU kill otherwise accepts a negative PID as another option and
        // can report success without signaling the process group.
        .args(["-KILL", "--", process_group.as_str()])
        // The status drives the exact-child fallback below. Do not leak a
        // partial process-group diagnostic when that fallback still reaps the
        // owned Chrome tree successfully.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if tree_killed || child.try_wait()?.is_some() {
        Ok(())
    } else {
        child.kill()
    }
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut ProcessChild) -> std::io::Result<()> {
    // `JobObjectChild::start_kill` terminates the exact retained Job Object,
    // including descendants, without reopening a numeric PID.
    child.start_kill()
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(child: &mut ProcessChild) -> std::io::Result<()> {
    child.kill()
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        if self.kill() {
            self.control.clear(&self.child);
        }
    }
}

/// Resolve a configured command (absolute path or PATH name) or fall back to
/// the standard candidate list.
///
/// # Errors
/// Returns `ChromeError::NoBinary` when no candidate resolves.
pub fn resolve_binary(command: &str) -> Result<PathBuf> {
    let path = Path::new(command);
    if path.is_absolute() {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(ChromeError::NoBinary);
    }
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return Err(ChromeError::NoBinary);
    };
    #[cfg(windows)]
    let windows_path_names = windows_path_names(command);
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
        #[cfg(windows)]
        for command_with_extension in &windows_path_names {
            let candidate = dir.join(command_with_extension);
            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(ChromeError::NoBinary)
}

#[cfg(windows)]
fn windows_path_names(command: &str) -> Vec<OsString> {
    if Path::new(command).extension().is_some() {
        return Vec::new();
    }
    let extensions = std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
    path_names_with_extensions(command, &extensions.to_string_lossy())
}

#[cfg(any(windows, test))]
fn path_names_with_extensions(command: &str, extensions: &str) -> Vec<OsString> {
    extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(|extension| {
            let mut candidate = OsString::from(command);
            if !extension.starts_with('.') {
                candidate.push(".");
            }
            candidate.push(extension);
            candidate
        })
        .collect()
}

/// Resolve using an explicit command or the standard candidate list.
///
/// # Errors
/// Returns `ChromeError::NoBinary` when no candidate resolves.
pub fn resolve_binary_or_default(command: &Option<String>) -> Result<PathBuf> {
    if let Some(command) = command
        && !command.trim().is_empty()
    {
        return resolve_binary(command);
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        for name in BINARY_CANDIDATES {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(name);
                if is_executable_file(&candidate) {
                    return Ok(candidate);
                }
            }
        }
        #[cfg(windows)]
        for name in WINDOWS_BINARY_CANDIDATES {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    #[cfg(windows)]
    for candidate in standard_installations() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    #[cfg(target_os = "macos")]
    for candidate in standard_installations() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(ChromeError::NoBinary)
}

/// Standard Windows install locations, in preference order.
#[cfg(windows)]
fn standard_installations() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for var in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
        let Ok(root) = std::env::var(var) else {
            continue;
        };
        candidates.push(PathBuf::from(&root).join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(PathBuf::from(&root).join(r"Microsoft\Edge\Application\msedge.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.extend(per_user_windows_installations(Path::new(&local)));
    }
    candidates
}

#[cfg(any(windows, test))]
fn per_user_windows_installations(local_app_data: &Path) -> [PathBuf; 3] {
    [
        local_app_data.join(r"Google\Chrome\Application\chrome.exe"),
        local_app_data.join(r"Microsoft\Edge\Application\msedge.exe"),
        local_app_data.join(r"Programs\chromium\Application\chrome.exe"),
    ]
}

/// Standard macOS install locations (browsers installed as `.app` bundles
/// are not on `PATH`), in preference order.
#[cfg(target_os = "macos")]
fn standard_installations() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        PathBuf::from("/Applications/Arc.app/Contents/MacOS/Arc"),
    ]
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file() && path.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_existing_binary() {
        // The running test binary always exists and is portable.
        let exe = std::env::current_exe().unwrap();
        let resolved = resolve_binary(exe.to_str().unwrap()).unwrap();
        assert_eq!(resolved, exe);
    }

    #[cfg(unix)]
    #[test]
    fn resolves_unix_shell() {
        // /bin/sh is present on every supported Unix platform.
        let resolved = resolve_binary("/bin/sh").unwrap();
        assert_eq!(resolved, PathBuf::from("/bin/sh"));
    }

    #[test]
    fn rejects_missing_binary() {
        assert!(matches!(
            resolve_binary("/definitely/not/a/real/binary"),
            Err(ChromeError::NoBinary)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn profile_directory_permissions_are_private_and_existing_modes_are_tightened() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temp dir");
        let profile_dir = root.path().join("profile");
        std::fs::create_dir(&profile_dir).expect("profile dir");
        std::fs::set_permissions(&profile_dir, std::fs::Permissions::from_mode(0o755))
            .expect("permissive profile mode");

        prepare_profile_dir(&profile_dir).expect("private profile dir");

        let mode = std::fs::metadata(&profile_dir)
            .expect("profile metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn process_control_force_terminates_and_reaps_the_exact_child() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]).process_group(0);
        let child = command
            .spawn()
            .unwrap_or_else(|error| panic!("spawn test child: {error}"));
        let child = Arc::new(Mutex::new(child));
        let control = ChromeProcessControl::default();
        control.register(&child);

        assert!(!control.is_reaped());
        assert!(control.terminate(Duration::from_secs(2)));
        assert!(control.is_reaped());
        assert!(
            child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_wait()
                .is_ok_and(|status| status.is_some())
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_control_force_honors_deadline_while_child_handle_is_busy() {
        use std::os::unix::process::CommandExt;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]).process_group(0);
        let child = Arc::new(Mutex::new(
            command
                .spawn()
                .unwrap_or_else(|error| panic!("spawn test child: {error}")),
        ));
        let control = ChromeProcessControl::default();
        control.register(&child);
        let child_guard = child.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            let started = Instant::now();
            let terminated = worker_control.terminate(Duration::from_millis(20));
            (terminated, started.elapsed())
        });

        let (terminated, elapsed) = worker.join().unwrap_or_else(|error| std::panic::resume_unwind(error));
        assert!(!terminated);
        assert!(elapsed < Duration::from_millis(500));
        drop(child_guard);
        assert!(control.terminate(Duration::from_secs(2)));
    }

    #[cfg(unix)]
    #[test]
    fn process_control_waits_for_late_child_registration_before_succeeding() {
        use std::os::unix::process::CommandExt;

        let control = ChromeProcessControl::default();
        let worker_control = control.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = started_tx.send(());
            let _ = result_tx.send(worker_control.terminate(Duration::from_secs(2)));
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("wait for cleanup worker: {error}"));
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]).process_group(0);
        let child = Arc::new(Mutex::new(
            command
                .spawn()
                .unwrap_or_else(|error| panic!("spawn late test child: {error}")),
        ));
        control.register(&child);

        assert!(
            result_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|error| panic!("wait for forced cleanup: {error}"))
        );
        worker.join().unwrap_or_else(|error| std::panic::resume_unwind(error));
        assert!(control.is_reaped());
        assert!(
            child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_wait()
                .is_ok_and(|status| status.is_some())
        );
    }

    #[test]
    fn process_control_succeeds_after_spawn_settles_without_a_child() {
        let control = ChromeProcessControl::default();
        let worker_control = control.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = started_tx.send(());
            let _ = result_tx.send(worker_control.terminate(Duration::from_secs(2)));
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("wait for cleanup worker: {error}"));
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        control.mark_registration_settled();

        assert!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_else(|error| panic!("wait for settled cleanup: {error}"))
        );
        worker.join().unwrap_or_else(|error| std::panic::resume_unwind(error));
    }

    #[test]
    fn windows_path_extensions_are_applied_to_bare_commands() {
        let names = path_names_with_extensions("chrome", ".COM;.EXE;CMD");
        assert_eq!(
            names,
            [
                OsString::from("chrome.COM"),
                OsString::from("chrome.EXE"),
                OsString::from("chrome.CMD")
            ]
        );
    }

    #[test]
    fn windows_per_user_browser_locations_cover_chrome_edge_and_chromium() {
        let candidates = per_user_windows_installations(Path::new(r"C:\Users\test\AppData\Local"));

        assert_eq!(
            candidates,
            [
                PathBuf::from(r"C:\Users\test\AppData\Local").join(r"Google\Chrome\Application\chrome.exe"),
                PathBuf::from(r"C:\Users\test\AppData\Local").join(r"Microsoft\Edge\Application\msedge.exe"),
                PathBuf::from(r"C:\Users\test\AppData\Local").join(r"Programs\chromium\Application\chrome.exe"),
            ]
        );
    }

    #[test]
    fn parses_ws_url_variants() {
        assert_eq!(
            parse_devtools_ws_url("DevTools listening on ws://127.0.0.1:37677/devtools/browser/70f45c84-x"),
            Some("ws://127.0.0.1:37677/devtools/browser/70f45c84-x".to_string())
        );
        assert_eq!(parse_devtools_ws_url("something else"), None);
    }

    #[test]
    fn rejects_extra_args_that_override_managed_browser_switches() {
        for arguments in [
            vec!["--remote-debugging-port=9222".to_string()],
            vec!["-remote-debugging-address=0.0.0.0".to_string()],
            vec!["/remote-debugging-address=0.0.0.0".to_string()],
            vec!["/remote-debugging-port:9222".to_string()],
            vec!["--remote-debugging-address".to_string(), "0.0.0.0".to_string()],
            vec!["--remote-debugging-pipe".to_string()],
            vec!["--remote-allow-origins=https://example.com".to_string()],
            vec!["-remote-allow-origins=https://example.com".to_string()],
            vec!["--user-data-dir".to_string(), "/tmp/shared".to_string()],
            vec!["/user-data-dir=C:\\shared".to_string()],
            vec!["--REMOTE-DEBUGGING-ADDRESS=0.0.0.0".to_string()],
            vec!["--Remote-Allow-Origins=https://example.com".to_string()],
            vec!["/USER-DATA-DIR:C:\\shared".to_string()],
            vec!["--enable-automation".to_string()],
            vec!["--disable-blink-features=Foo".to_string()],
            vec!["--enable-blink-features=AutomationControlled".to_string()],
        ] {
            assert!(matches!(
                validate_extra_args(&arguments, AutomationDisclosurePolicy::MinimizeCommonSignals),
                Err(ChromeError::ProtectedExtraArg(_))
            ));
        }
    }

    #[test]
    fn accepts_unmanaged_extra_args() {
        assert!(
            validate_extra_args(
                &[
                    "--force-device-scale-factor=2".to_string(),
                    "--disable-extensions".to_string(),
                    "--disable-gpu".to_string(),
                ],
                AutomationDisclosurePolicy::MinimizeCommonSignals,
            )
            .is_ok()
        );
    }

    #[test]
    fn default_launch_uses_gpu_negotiation_and_minimizes_common_disclosure() {
        let launch = ChromeLaunch {
            command: "chrome".to_string(),
            profile_dir: PathBuf::from("profile"),
            width: 1280,
            height: 800,
            extra_args: Vec::new(),
            automation_disclosure: AutomationDisclosurePolicy::MinimizeCommonSignals,
        };
        let args = launch_args(&launch).unwrap_or_default();

        assert!(!args.iter().any(|argument| argument == "--disable-gpu"));
        assert!(
            args.iter()
                .any(|argument| argument == "--disable-blink-features=AutomationControlled")
        );
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
    }

    #[test]
    fn browser_default_disclosure_allows_caller_owned_blink_switches() {
        let launch = ChromeLaunch {
            command: "chrome".to_string(),
            profile_dir: PathBuf::from("profile"),
            width: 1280,
            height: 800,
            extra_args: vec!["--enable-blink-features=ExperimentalFoo".to_string()],
            automation_disclosure: AutomationDisclosurePolicy::BrowserDefault,
        };
        let args = launch_args(&launch).unwrap_or_default();

        assert!(
            args.iter()
                .all(|argument| argument != "--disable-blink-features=AutomationControlled")
        );
        assert!(
            args.iter()
                .any(|argument| argument == "--enable-blink-features=ExperimentalFoo")
        );
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
    }
}
