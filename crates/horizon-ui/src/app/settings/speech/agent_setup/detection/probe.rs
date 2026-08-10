//! Executable and platform-path resolution for setup agents.

#[cfg(unix)]
pub(super) mod login_shell;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(unix)]
use horizon_core::resolved_default_shell;
use horizon_core::{Config, PresetConfig};

use super::{SpeechSetupAgentAvailability, SpeechSetupProbeFailure};
use crate::app::settings::speech::agent_setup::{SpeechSetupAgent, selected_setup_preset};

#[cfg(unix)]
use self::login_shell::{LoginShellProbe, probe_login_shell};
#[cfg(unix)]
const LOGIN_SHELL_PROBE_TIMEOUT: Duration = Duration::from_millis(1_200);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AgentScanKey {
    codex_command: Option<String>,
    claude_command: Option<String>,
    environment: ProbeEnvironment,
}

impl AgentScanKey {
    pub(super) fn new(config: &Config, environment: &ProbeEnvironment) -> Self {
        Self {
            codex_command: setup_command(config, SpeechSetupAgent::Codex),
            claude_command: setup_command(config, SpeechSetupAgent::Claude),
            environment: environment.clone(),
        }
    }

    pub(super) fn command(&self, agent: SpeechSetupAgent) -> Option<&str> {
        match agent {
            SpeechSetupAgent::Codex => self.codex_command.as_deref(),
            SpeechSetupAgent::Claude => self.claude_command.as_deref(),
        }
    }
}

fn setup_command(config: &Config, agent: SpeechSetupAgent) -> Option<String> {
    selected_setup_preset(config, agent)
        .map(|preset| preset.command.unwrap_or_else(|| agent.default_command().to_string()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProbeEnvironment {
    pub(super) path: Option<OsString>,
    pub(super) path_ext: Option<OsString>,
    #[cfg(unix)]
    shell: Option<PathBuf>,
    home: Option<PathBuf>,
    cwd: Option<PathBuf>,
}

impl ProbeEnvironment {
    pub(super) fn capture(workspace_cwd: Option<&Path>) -> Self {
        Self {
            path: std::env::var_os("PATH"),
            path_ext: std::env::var_os("PATHEXT"),
            #[cfg(unix)]
            shell: Some(PathBuf::from(resolved_default_shell())),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            cwd: workspace_cwd
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .map(normalize_probe_cwd),
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(path: Option<PathBuf>, shell: Option<PathBuf>, home: PathBuf, cwd: PathBuf) -> Self {
        #[cfg(not(unix))]
        let _ = shell;
        Self {
            path: path.map(PathBuf::into_os_string),
            path_ext: None,
            #[cfg(unix)]
            shell,
            home: Some(home),
            cwd: Some(normalize_probe_cwd(cwd)),
        }
    }
}

fn normalize_probe_cwd(cwd: PathBuf) -> PathBuf {
    if cwd.is_absolute() {
        cwd
    } else {
        std::env::current_dir().map_or(cwd.clone(), |process_cwd| process_cwd.join(cwd))
    }
}

pub(super) const fn login_shell_probe_timeout() -> Duration {
    #[cfg(unix)]
    {
        LOGIN_SHELL_PROBE_TIMEOUT
    }
    #[cfg(not(unix))]
    {
        Duration::ZERO
    }
}

pub(super) fn probe_candidate(
    command: &str,
    environment: &ProbeEnvironment,
    login_shell_timeout: Duration,
) -> SpeechSetupAgentAvailability {
    #[cfg(not(unix))]
    let _ = login_shell_timeout;

    if command.is_empty() {
        return SpeechSetupAgentAvailability::Missing;
    }

    #[cfg(unix)]
    if let Some(failure) = launch_shell_file_failure(environment) {
        return SpeechSetupAgentAvailability::Unknown(failure);
    }

    if is_explicit_path(command) {
        return probe_explicit_path(command, environment);
    }

    let mut first_error = None;
    for path in path_candidates(command, environment) {
        #[cfg(windows)]
        if !windows_path_is_helper_launchable(&path) {
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(format!(
                        "`{}` uses an unsupported Windows executable type",
                        path.display()
                    )));
                }
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(format!("could not inspect `{}`: {error}", path.display()));
                    }
                    continue;
                }
            }
        }
        match executable_file_state(&path) {
            Ok(true) => return available_executable(path, environment),
            Ok(false) => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(format!("could not inspect `{}`: {error}", path.display()));
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let Some(shell) = environment.shell.as_deref() {
            match probe_login_shell(shell, command, environment, login_shell_timeout) {
                LoginShellProbe::Available(executable) => {
                    return SpeechSetupAgentAvailability::Available { executable };
                }
                LoginShellProbe::Missing => {}
                LoginShellProbe::Timeout => {
                    return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Timeout);
                }
                LoginShellProbe::Failed(error) => {
                    return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(error));
                }
            }
        }
    }

    first_error.map_or(SpeechSetupAgentAvailability::Missing, |error| {
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(error))
    })
}

/// Re-check the exact path returned by the background scan immediately before
/// creating a panel. This uses only filesystem metadata and the operating
/// system's access check, so clicking a setup action never starts a helper
/// process or evaluates shell profile scripts on the egui thread.
pub(in crate::app) fn verify_preset_command(
    preset: &PresetConfig,
    workspace_cwd: Option<&Path>,
) -> SpeechSetupAgentAvailability {
    let environment = ProbeEnvironment::capture(workspace_cwd);
    verify_preset_command_with_environment(preset, &environment)
}

pub(super) fn verify_preset_command_with_environment(
    preset: &PresetConfig,
    environment: &ProbeEnvironment,
) -> SpeechSetupAgentAvailability {
    #[cfg(unix)]
    if let Some(failure) = launch_shell_file_failure(environment) {
        return SpeechSetupAgentAvailability::Unknown(failure);
    }
    let Some(command) = preset.command.as_deref() else {
        return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "setup executable was not resolved by the background scan".to_string(),
        ));
    };
    if !is_explicit_path(command) {
        return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "setup executable was not resolved to an explicit path".to_string(),
        ));
    }
    probe_explicit_path_with(command, environment, executable_file_state)
}

#[cfg(unix)]
fn launch_shell_file_failure(environment: &ProbeEnvironment) -> Option<SpeechSetupProbeFailure> {
    match probe_launch_shell_file(environment) {
        SpeechSetupAgentAvailability::Available { .. } => None,
        SpeechSetupAgentAvailability::Missing => Some(SpeechSetupProbeFailure::Failed(
            "the configured launch shell is missing or not executable".to_string(),
        )),
        SpeechSetupAgentAvailability::Unknown(error) => Some(error),
        SpeechSetupAgentAvailability::Checking => Some(SpeechSetupProbeFailure::Failed(
            "launch shell verification did not complete".to_string(),
        )),
    }
}

#[cfg(unix)]
fn probe_launch_shell_file(environment: &ProbeEnvironment) -> SpeechSetupAgentAvailability {
    let Some(shell) = environment.shell.as_deref() else {
        return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "no launch shell is available".to_string(),
        ));
    };
    let Some(command) = shell.to_str() else {
        return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "the configured launch shell path is not valid UTF-8".to_string(),
        ));
    };
    if command == "~" || command.starts_with("~/") || command.starts_with("~\\") {
        return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "the configured launch shell uses a tilde path that the panel launcher does not expand".to_string(),
        ));
    }
    if is_explicit_path(command) {
        return probe_explicit_path(command, environment);
    }

    let mut first_error = None;
    for candidate in path_candidates(command, environment) {
        match executable_file_state(&candidate) {
            Ok(true) => return available_executable(candidate, environment),
            Ok(false) => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(format!(
                        "could not inspect launch shell `{}`: {error}",
                        candidate.display()
                    ));
                }
            }
        }
    }
    first_error.map_or(SpeechSetupAgentAvailability::Missing, |error| {
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(error))
    })
}

fn probe_explicit_path(command: &str, environment: &ProbeEnvironment) -> SpeechSetupAgentAvailability {
    probe_explicit_path_with(command, environment, executable_file_state)
}

fn probe_explicit_path_with(
    command: &str,
    environment: &ProbeEnvironment,
    file_state: fn(&Path) -> io::Result<bool>,
) -> SpeechSetupAgentAvailability {
    let path = expand_explicit_path(command, environment);
    let candidates = explicit_path_candidates(&path, environment.path_ext.as_deref());
    let mut first_error = None;
    for candidate in candidates {
        #[cfg(windows)]
        if !windows_path_is_helper_launchable(&candidate) {
            match std::fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => {
                    return SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(format!(
                        "`{}` uses an unsupported Windows executable type",
                        candidate.display()
                    )));
                }
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(format!("could not inspect `{}`: {error}", candidate.display()));
                    }
                    continue;
                }
            }
        }
        match file_state(&candidate) {
            Ok(true) => return available_executable(candidate, environment),
            Ok(false) => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(format!("could not inspect `{}`: {error}", candidate.display()));
                }
            }
        }
    }

    first_error.map_or(SpeechSetupAgentAvailability::Missing, |error| {
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(error))
    })
}

fn available_executable(path: PathBuf, environment: &ProbeEnvironment) -> SpeechSetupAgentAvailability {
    let absolute = absolute_candidate(path, environment);
    match absolute.into_os_string().into_string() {
        Ok(executable) => SpeechSetupAgentAvailability::Available { executable },
        Err(_) => SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(
            "resolved executable path is not valid UTF-8".to_string(),
        )),
    }
}

fn absolute_candidate(path: PathBuf, environment: &ProbeEnvironment) -> PathBuf {
    if path.is_absolute() {
        return path;
    }

    let base = environment
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    if base.is_absolute() {
        base.join(path)
    } else {
        match std::env::current_dir() {
            Ok(current) => current.join(base).join(path),
            Err(_) => base.join(path),
        }
    }
}

pub(super) fn is_explicit_path(command: &str) -> bool {
    let path = Path::new(command);
    path.is_absolute()
        || command.starts_with("~/")
        || command.starts_with("~\\")
        || command.contains('/')
        || command.contains('\\')
}

pub(super) fn expand_explicit_path(command: &str, environment: &ProbeEnvironment) -> PathBuf {
    let expanded = if command == "~" {
        environment.home.clone().unwrap_or_else(|| PathBuf::from(command))
    } else if let Some(rest) = command.strip_prefix("~/").or_else(|| command.strip_prefix("~\\")) {
        environment
            .home
            .as_ref()
            .map_or_else(|| PathBuf::from(command), |home| home.join(rest))
    } else {
        PathBuf::from(command)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        environment
            .cwd
            .as_ref()
            .map_or(expanded.clone(), |cwd| cwd.join(expanded))
    }
}

pub(super) fn path_candidates(command: &str, environment: &ProbeEnvironment) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        return windows_path_candidates(command, environment);
    }
    #[cfg(not(windows))]
    {
        let Some(path) = environment.path.as_deref() else {
            return Vec::new();
        };
        std::env::split_paths(path)
            .map(|directory| absolute_candidate(directory, environment).join(command))
            .collect()
    }
}

#[cfg(any(windows, test))]
pub(super) fn windows_path_candidates(command: &str, environment: &ProbeEnvironment) -> Vec<PathBuf> {
    let Some(path) = environment.path.as_deref() else {
        return Vec::new();
    };
    std::env::split_paths(path)
        .flat_map(|directory| {
            let directory = absolute_candidate(directory, environment);
            windows_executable_name_candidates(command, environment.path_ext.as_deref())
                .into_iter()
                .map(move |name| directory.join(name))
        })
        .collect()
}

fn explicit_path_candidates(path: &Path, path_ext: Option<&OsStr>) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let path_text = path.as_os_str().to_string_lossy();
        executable_name_candidates(&path_text, path_ext)
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
    #[cfg(not(windows))]
    {
        let _ = path_ext;
        vec![path.to_path_buf()]
    }
}

#[cfg(windows)]
fn executable_name_candidates(command: &str, path_ext: Option<&OsStr>) -> Vec<OsString> {
    windows_executable_name_candidates(command, path_ext)
}

#[cfg(any(windows, test))]
pub(super) fn windows_executable_name_candidates(command: &str, path_ext: Option<&OsStr>) -> Vec<OsString> {
    if windows_file_extension(command).is_some() {
        return vec![OsString::from(command)];
    }

    windows_path_extensions(path_ext)
        .into_iter()
        .map(|extension| OsString::from(format!("{command}{extension}")))
        .collect()
}

#[cfg(any(windows, test))]
fn windows_file_extension(command: &str) -> Option<&str> {
    let file_name = command.rsplit(['/', '\\']).next()?;
    let (_, extension) = file_name.rsplit_once('.')?;
    (!extension.is_empty()).then_some(extension)
}

#[cfg(any(windows, test))]
pub(super) fn windows_path_is_helper_launchable(path: &Path) -> bool {
    path.extension().is_none_or(|extension| {
        let extension = extension.to_string_lossy();
        matches!(extension.to_ascii_lowercase().as_str(), "com" | "exe" | "bat" | "cmd")
    })
}

#[cfg(any(windows, test))]
fn windows_path_extensions(path_ext: Option<&OsStr>) -> Vec<String> {
    let source = path_ext.filter(|value| !value.is_empty()).map_or_else(
        || ".COM;.EXE;.BAT;.CMD".to_string(),
        |value| value.to_string_lossy().into_owned(),
    );
    let mut extensions = Vec::new();
    for extension in source
        .split(';')
        .map(str::trim)
        .filter(|extension| !extension.is_empty())
    {
        let normalized = if extension.starts_with('.') {
            extension.to_string()
        } else {
            format!(".{extension}")
        };
        if !extensions
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&normalized))
        {
            extensions.push(normalized);
        }
    }
    extensions
}

fn executable_file_state(path: &Path) -> io::Result<bool> {
    if !metadata_executable_file_state(path)? {
        return Ok(false);
    }

    #[cfg(unix)]
    {
        match rustix::fs::access(path, rustix::fs::Access::EXEC_OK) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(io::Error::from(error)),
        }
    }
    #[cfg(not(unix))]
    {
        Ok(true)
    }
}

fn metadata_executable_file_state(path: &Path) -> io::Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Ok(false);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(true)
    }
}
