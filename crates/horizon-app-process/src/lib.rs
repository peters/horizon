#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod client;
#[cfg(unix)]
mod guard;
mod storage;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("app_process_invalid: a bounded declared foreground command is required")]
    Invalid,
    #[error("app_process_state_unavailable: private process state is unavailable")]
    StateUnavailable,
    #[error("app_process_start_failed: the declared process did not start")]
    StartFailed,
    #[error("app_process_failed: the declared process failed")]
    Failed,
    #[error("app_process_timeout: the declared process exceeded its budget")]
    Timeout,
    #[error("app_process_cleanup_uncertain: reconcile the privately recorded process before retrying")]
    CleanupUncertain,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Build,
    Backend,
}

/// Trusted host request, deliberately neither Debug nor publicly serializable.
pub struct Request {
    spec: Spec,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    operation: Uuid,
    root: PathBuf,
    state: PathBuf,
    argv: Vec<String>,
    environment: BTreeMap<String, String>,
    kind: Kind,
    startup_seconds: u64,
    lifetime_seconds: u64,
}

impl Request {
    /// # Errors
    /// `state` is an existing fresh private host directory, never a tool/project-selected path.
    /// Commands are the already-authorized contract argv, without shell interpolation.
    pub fn new(
        root: &Path,
        state: &Path,
        argv: Vec<String>,
        kind: Kind,
        startup_seconds: u64,
        lifetime_seconds: u64,
    ) -> Result<Self> {
        let environment = [
            "PATH",
            "HOME",
            "USER",
            "LANG",
            "LC_ALL",
            "DOTNET_ROOT",
            "JAVA_HOME",
            "ANDROID_HOME",
            "ANDROID_SDK_ROOT",
            "SSH_AUTH_SOCK",
        ]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
        .collect();
        let request = Self {
            spec: Spec {
                operation: Uuid::new_v4(),
                root: root.canonicalize().map_err(|_| Error::Invalid)?,
                state: state.to_owned(),
                argv,
                environment,
                kind,
                startup_seconds,
                lifetime_seconds,
            },
        };
        request.spec.validate()?;
        Ok(request)
    }
}

impl Spec {
    fn validate(&self) -> Result<()> {
        if self.operation.is_nil()
            || self.argv.is_empty()
            || self.argv.len() > 64
            || self
                .argv
                .iter()
                .any(|v| v.is_empty() || v.len() > 4096 || v.chars().any(char::is_control))
            || !(1..=1200).contains(&self.startup_seconds)
            || !(self.startup_seconds..=1800).contains(&self.lifetime_seconds)
            || self.root.canonicalize().map_err(|_| Error::Invalid)? != self.root
            || !self.root.is_dir()
            || self.environment.len() > 16
            || self.environment.iter().any(|(key, value)| {
                !matches!(
                    key.as_str(),
                    "PATH"
                        | "HOME"
                        | "USER"
                        | "LANG"
                        | "LC_ALL"
                        | "DOTNET_ROOT"
                        | "JAVA_HOME"
                        | "ANDROID_HOME"
                        | "ANDROID_SDK_ROOT"
                        | "SSH_AUTH_SOCK"
                ) || value.len() > 8192
                    || value.contains('\0')
            })
        {
            return Err(Error::Invalid);
        }
        if serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() >= 32768 {
            return Err(Error::Invalid);
        }
        storage::private_directory(&self.state)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Armed {},
    Started {},
    Ready { port: u16 },
    Complete { success: bool },
    Failed { code: String },
}

/// Entrypoint for the bundled private guardian binary. No provider credentials are read here.
/// # Errors
/// Unsupported platforms fail closed before executing commands.
pub fn run_guard() -> Result<()> {
    #[cfg(unix)]
    {
        guard::run()
    }
    #[cfg(not(unix))]
    {
        Err(Error::StartFailed)
    }
}
