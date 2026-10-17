use std::{net::SocketAddr, sync::Arc};

use russh::keys::{Algorithm, PrivateKey, PublicKey, decode_secret_key};
use tokio::time::Instant;
use zeroize::Zeroizing;

/// Errors contain categories, never commands, credentials or server diagnostic text.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid or oversized session options")]
    InvalidOptions,
    #[error("invalid or unsupported key (Ed25519 required)")]
    InvalidKey,
    #[error("server host key does not match the caller's pin")]
    HostKeyMismatch,
    #[error("SSH authentication refused")]
    Authentication,
    #[error("SSH command request refused")]
    ExecRejected,
    #[error("SSH transport failed")]
    Transport,
    #[error("unexpected SSH channel message")]
    Protocol,
    #[error("remote close without exit status or signal")]
    MissingExit,
    #[error("configured data limit exceeded")]
    DataLimit,
    #[error("session cancelled")]
    Cancelled,
    #[error("session deadline exceeded")]
    Deadline,
    #[error("session shutdown did not complete within its bound")]
    ShutdownTimeout,
    #[error("a caller-owned Tokio runtime is required")]
    NoRuntime,
    #[error("session already finished")]
    Finished,
}

/// Exact public host key; comments do not participate in verification. No TOFU.
pub struct HostKeyPin(pub(crate) PublicKey);

impl HostKeyPin {
    /// Parse an explicitly trusted OpenSSH Ed25519 public key.
    ///
    /// # Errors
    /// Rejects oversized, malformed, certificate or unsupported keys.
    pub fn from_openssh(key: &str) -> Result<Self, Error> {
        if key.len() > 16 * 1024 {
            return Err(Error::InvalidKey);
        }
        let key = PublicKey::from_openssh(key).map_err(|_| Error::InvalidKey)?;
        if key.algorithm() != Algorithm::Ed25519 {
            return Err(Error::InvalidKey);
        }
        Ok(Self(key))
    }
}

/// Owned parsed secret. No Debug, serialization, agent lookup or implicit file access.
pub struct Credentials(pub(crate) Arc<PrivateKey>);

impl Credentials {
    /// Parse caller-provided OpenSSH key material, optionally encrypted.
    /// Temporary copies are zeroized; the caller still owns its original buffers.
    ///
    /// # Errors
    /// Rejects oversized, malformed, encrypted-without-password or non-Ed25519 keys.
    pub fn from_openssh(key: &str, password: Option<&str>) -> Result<Self, Error> {
        if key.len() > 64 * 1024 || password.is_some_and(|p| p.len() > 4096) {
            return Err(Error::InvalidKey);
        }
        let key = Zeroizing::new(key.to_owned());
        let password = password.map(|p| Zeroizing::new(p.to_owned()));
        let parsed = decode_secret_key(&key, password.as_deref().map(String::as_str)).map_err(|_| Error::InvalidKey)?;
        if parsed.algorithm() != Algorithm::Ed25519 {
            return Err(Error::InvalidKey);
        }
        Ok(Self(Arc::new(parsed)))
    }
}

/// Cumulative byte limits. Queues and individual input chunks have fixed bounds.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub stdin: usize,
    pub stdout: usize,
    pub stderr: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            stdin: 1024 * 1024,
            stdout: 4 * 1024 * 1024,
            stderr: 4 * 1024 * 1024,
        }
    }
}

/// One direct TCP connection and one remote shell command. Does not implement argv escaping.
pub struct ExecRequest {
    pub(crate) endpoint: SocketAddr,
    pub(crate) username: String,
    pub(crate) command: String,
    pub(crate) pin: HostKeyPin,
    pub(crate) deadline: Instant,
    pub(crate) limits: Limits,
}

impl ExecRequest {
    /// Bound borrowed input before allocating it. Deadline covers connect through final close.
    ///
    /// # Errors
    /// Rejects empty/oversized input or an elapsed deadline.
    pub fn new(
        endpoint: SocketAddr,
        username: &str,
        command: &str,
        pin: HostKeyPin,
        deadline: Instant,
    ) -> Result<Self, Error> {
        if username.is_empty()
            || username.len() > 256
            || username.contains('\0')
            || command.is_empty()
            || command.len() > 32 * 1024
            || command.contains('\0')
            || deadline <= Instant::now()
        {
            return Err(Error::InvalidOptions);
        }
        Ok(Self {
            endpoint,
            username: username.to_owned(),
            command: command.to_owned(),
            pin,
            deadline,
            limits: Limits::default(),
        })
    }

    /// Set cumulative limits, each at most 64 MiB. Zero permits no bytes on that stream.
    ///
    /// # Errors
    /// Rejects limits above the package's hard bound.
    pub fn with_limits(mut self, limits: Limits) -> Result<Self, Error> {
        if [limits.stdin, limits.stdout, limits.stderr]
            .into_iter()
            .any(|n| n > 64 * 1024 * 1024)
        {
            return Err(Error::InvalidOptions);
        }
        self.limits = limits;
        Ok(self)
    }
}

/// Remote termination information, distinct from transport closure.
#[derive(Clone, PartialEq, Eq)]
pub enum Exit {
    Status(u32),
    Signal { name: String, core_dumped: bool },
}

impl std::fmt::Debug for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(status) => f.debug_tuple("Status").field(status).finish(),
            Self::Signal { core_dumped, .. } => f
                .debug_struct("Signal")
                .field("name", &"<redacted>")
                .field("core_dumped", core_dumped)
                .finish(),
        }
    }
}

/// Untrusted remote bytes are intentionally not Debug-formatted.
pub enum Event {
    /// The server accepted the SSH exec request; not application readiness.
    ExecAccepted,
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    RemoteEof,
    Exit(Exit),
    RemoteClose,
}

/// Returned only after remote close and local transport retirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub exit: Exit,
    pub remote_eof: bool,
}
