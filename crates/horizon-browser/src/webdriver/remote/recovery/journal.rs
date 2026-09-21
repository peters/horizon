//! Private recovery identity, persisted before a hosted session becomes usable.
use super::{RemoteAllocation, RemoteRecoveryStatus, SessionProbe, State};
use crate::webdriver::remote::{RemoteHost, RemoteSessionRequest};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    reference: String,
    endpoint: String,
    quota_key: String,
    session: Option<String>,
    released: bool,
}

impl Record {
    fn load(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::other("Remote recovery journal must be private"));
            }
        }
        if !metadata.is_file() || metadata.len() > 8192 {
            return Err(io::Error::other("Invalid remote recovery journal"));
        }
        let mut bytes = Vec::new();
        file.take(8193).read_to_end(&mut bytes)?;
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| io::Error::other("Invalid remote recovery journal"))?;
        if record.version != 1
            || record.reference.is_empty()
            || record.reference.len() > 128
            || record.session.as_ref().is_some_and(|s| {
                s.is_empty()
                    || s.len() > 512
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
        {
            return Err(io::Error::other("Invalid remote recovery identity"));
        }
        Ok(record)
    }
}

pub(super) struct Journal {
    path: PathBuf,
    record: Record,
}
impl Journal {
    fn save(&self) -> io::Result<()> {
        let temporary = self.path.with_extension("pending");
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, &self.record)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        #[cfg(unix)]
        if let Some(parent) = self.path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
    pub(super) fn identify(&mut self, session: &str) -> io::Result<()> {
        self.record.session = Some(session.into());
        self.save()
    }
    pub(super) fn release(&mut self) {
        self.record.released = true;
        // A failed write leaves the prior exact identity available for probing.
        let _ = self.save();
    }
}
impl RemoteAllocation {
    /// Retain this allocation in a caller-owned private directory before launch.
    /// Credentials are never serialized. The host must keep the original binding.
    /// # Errors
    /// Fails if the private journal cannot be durably written, or launch already began.
    pub fn retain_journal(&self, path: &Path, request: &RemoteSessionRequest) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.identity.is_some() || state.retired || state.journal.is_some() {
            return Err(io::Error::other(
                "Remote recovery journal must be installed before launch",
            ));
        }
        let journal = Journal {
            path: path.into(),
            record: Record {
                version: 1,
                reference: self.reference.clone(),
                endpoint: request.endpoint.clone(),
                quota_key: request.quota_key.clone(),
                session: None,
                released: false,
            },
        };
        journal.save()?;
        state.journal = Some(journal);
        Ok(())
    }

    /// Reconstruct only recovery, never a running browser or an allocation request.
    /// The supplied provider/account binding must match the journal exactly.
    /// # Errors
    /// Refuses malformed/private-file violations or a changed provider/account binding.
    pub fn restore_journal(path: &Path, request: &RemoteSessionRequest) -> io::Result<Self> {
        let record = Record::load(path)?;
        if record.endpoint != request.endpoint || record.quota_key != request.quota_key {
            return Err(io::Error::other("Remote recovery identity or binding does not match"));
        }
        if record.released {
            return Ok(Self::from_journal(path, record, None));
        }
        let host = RemoteHost::connect(request).map_err(|_| io::Error::other("Remote recovery binding is invalid"))?;
        let identity = record
            .session
            .as_ref()
            .filter(|_| !record.released)
            .map(|session| SessionProbe::new(host.transport.clone(), session.clone(), host.report.clone()));
        Ok(Self::from_journal(path, record, identity))
    }

    /// Restore durable release evidence without a provider or credential binding.
    /// Unreleased identities still require their original binding for recovery.
    /// # Errors
    /// Refuses malformed identities and private-file violations.
    pub fn restore_released_journal(path: &Path) -> io::Result<Option<Self>> {
        let record = Record::load(path)?;
        Ok(record.released.then(|| Self::from_journal(path, record, None)))
    }

    fn from_journal(path: &Path, record: Record, identity: Option<SessionProbe>) -> Self {
        let status = if record.released {
            RemoteRecoveryStatus::Released
        } else if identity.is_some() {
            RemoteRecoveryStatus::Unresolved
        } else {
            RemoteRecoveryStatus::IdentityUnavailable
        };
        Self {
            reference: record.reference.clone(),
            state: Arc::new(Mutex::new(State {
                identity,
                status,
                retired: true,
                journal: Some(Journal {
                    path: path.into(),
                    record,
                }),
                ..State::default()
            })),
        }
    }
}

#[cfg(test)]
mod tests;
