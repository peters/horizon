//! Headless Chrome lifecycle: binary discovery, spawn, ws-url capture, kill.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::browser::cdp::parse_devtools_ws_url;

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
    pub extra_args: Vec<String>,
}

/// A running headless `Chrome` process plus its captured `DevTools` endpoint.
pub struct ChromeProcess {
    child: Child,
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
    pub fn spawn(launch: &ChromeLaunch) -> Result<Self> {
        let command = resolve_binary(&launch.command)?;
        std::fs::create_dir_all(&launch.profile_dir)?;

        let mut args: Vec<String> = vec![
            "--headless=new".to_string(),
            // Port 0: the OS picks a free port; we learn it from stderr.
            // Bound to 127.0.0.1 by default — never exposed beyond loopback.
            "--remote-debugging-port=0".to_string(),
            format!("--user-data-dir={}", launch.profile_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-gpu".to_string(),
            format!("--window-size={},{}", launch.width, launch.height),
            // Chrome >= 111 rejects DevTools clients without an allowed
            // origin. Local tooling only.
            "--remote-allow-origins=*".to_string(),
            "--disable-features=TranslateUI".to_string(),
        ];
        args.extend(launch.extra_args.iter().cloned());
        args.push("about:blank".to_string());

        let mut child = Command::new(command)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| std::io::Error::other("failed to capture chrome stderr"))?;

        let (ws_url_tx, ws_url_rx) = mpsc::channel();
        let tail = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_tail = tail.clone();
        std::thread::Builder::new()
            .name("chrome-stderr".into())
            .spawn(move || {
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
            })?;

        Ok(Self {
            child,
            exit_status: None,
            ws_url_rx,
            ws_url: None,
            stderr_tail,
        })
    }

    /// Block until the browser reports its `DevTools` endpoint (or `timeout`).
    ///
    /// # Errors
    /// Fails on timeout or when the process exits before reporting.
    pub fn wait_ws_url(&mut self, timeout: Duration) -> Result<String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(url) = self.ws_url.take() {
                return Ok(url);
            }
            match self.ws_url_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(url) => {
                    self.ws_url = Some(url.clone());
                    return Ok(url);
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
        let status = self.child.try_wait().ok().flatten();
        self.exit_status = status;
        status
    }

    /// Kill the browser. Chrome's child processes (zygotes, GPU, network
    /// service) exit with their parent; a short reap wait follows.
    pub fn kill(&mut self) {
        if self.child_status().is_some() {
            return;
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child_status().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Final reap in case it exited between our last poll and now.
        if self.child_status().is_none() {
            self.exit_status = self.child.wait().ok();
        }
    }

    fn stderr_tail_snapshot(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .to_string()
    }
}

impl Drop for ChromeProcess {
    fn drop(&mut self) {
        self.kill();
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
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(ChromeError::NoBinary)
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
    let Some(path_var) = std::env::var_os("PATH") else {
        return Err(ChromeError::NoBinary);
    };
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
        candidates.push(PathBuf::from(root).join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(PathBuf::from(root).join(r"Microsoft\Edge\Application\msedge.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Programs\chromium\Application\chrome.exe"));
    }
    candidates
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

    #[test]
    fn parses_ws_url_variants() {
        assert_eq!(
            parse_devtools_ws_url("DevTools listening on ws://127.0.0.1:37677/devtools/browser/70f45c84-x"),
            Some("ws://127.0.0.1:37677/devtools/browser/70f45c84-x".to_string())
        );
        assert_eq!(parse_devtools_ws_url("something else"), None);
    }
}
