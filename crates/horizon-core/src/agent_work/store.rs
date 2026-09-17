use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::PanelKind;

use super::{HookEvent, SuspendRecord, TranscriptSnapshot, TurnLedger, TurnState};

const SCHEMA_VERSION: u32 = 1;
const MAX_RECORD_BYTES: u64 = 256 * 1024;
const LOCK_WAIT: Duration = Duration::from_millis(200);

#[derive(Debug, Deserialize)]
pub struct HookInput {
    #[serde(flatten)]
    pub event: HookEvent,
    pub cwd: PathBuf,
    pub transcript_path: PathBuf,
}

/// Private metadata only: prompts, tool arguments and transcript text are not
/// part of this record. The owner identity rejects delayed hooks from an older
/// process after the same panel has been reopened.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredWork {
    version: u32,
    pub panel_local_id: String,
    pub kind: PanelKind,
    pub owner_token: String,
    pub cwd: PathBuf,
    pub transcript_path: PathBuf,
    pub ledger: TurnLedger,
    pub handoff: Option<SuspendRecord>,
    pub consumed: bool,
}

#[derive(Clone, Debug)]
pub struct WorkStore {
    root: PathBuf,
}

impl WorkStore {
    #[must_use]
    pub fn new(horizon_root: &Path) -> Self {
        Self {
            root: horizon_root.join("agent-work"),
        }
    }

    /// Read an atomic snapshot without retaining a lock while inspecting a
    /// transcript or repository.
    ///
    /// # Errors
    /// Rejects invalid identities, oversized/corrupt records and future schemas.
    pub fn read(&self, panel_local_id: &str) -> io::Result<Option<StoredWork>> {
        let record = self.read_raw(panel_local_id)?;
        if let Some(record) = &record {
            self.check_health(panel_local_id, &record.owner_token)?;
        }
        Ok(record)
    }

    fn read_raw(&self, panel_local_id: &str) -> io::Result<Option<StoredWork>> {
        let path = self.record_path(panel_local_id)?;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid("work record exceeds size limit"));
        }
        let record: StoredWork = serde_json::from_slice(&bytes).map_err(|_| invalid("invalid work record"))?;
        if record.version != SCHEMA_VERSION || record.panel_local_id != panel_local_id {
            return Err(invalid("work record identity or schema mismatch"));
        }
        Ok(Some(record))
    }

    /// Bind evidence to a new launch before starting its provider process.
    /// Hooks cannot replace this owner. Use a fresh random token per launch.
    ///
    /// # Errors
    /// Returns an error for invalid identity, contention or persistence failure.
    pub fn register_owner(
        &self,
        panel: &str,
        kind: PanelKind,
        owner: &str,
        session_id: Option<&str>,
        cwd: &Path,
    ) -> io::Result<()> {
        if !matches!(kind, PanelKind::Claude | PanelKind::Codex) || !cwd.is_absolute() {
            return Err(invalid("invalid launch identity"));
        }
        let health_path = self.health_path(panel, owner)?;
        let _lock = self.lock(panel)?;
        let previous = self.read_raw(panel)?;
        let generation = previous
            .as_ref()
            .map_or(0, |record| record.ledger.generation)
            .saturating_add(1);
        let keep = previous.as_ref().is_some_and(|record| {
            record.kind == kind
                && Some(record.ledger.session_id.as_str()) == session_id
                && self.check_health(panel, &record.owner_token).is_ok()
        });
        let mut record = if keep { previous } else { None }.unwrap_or_else(|| StoredWork {
            version: SCHEMA_VERSION,
            panel_local_id: panel.to_owned(),
            kind,
            owner_token: owner.to_owned(),
            cwd: cwd.to_path_buf(),
            transcript_path: PathBuf::new(),
            ledger: TurnLedger::default(),
            handoff: None,
            consumed: false,
        });
        owner.clone_into(&mut record.owner_token);
        record.cwd = cwd.to_path_buf();
        record.ledger = TurnLedger {
            session_id: session_id.unwrap_or("").to_owned(),
            generation,
            ..TurnLedger::default()
        };
        if let Some(parent) = health_path.parent() {
            private_directory(parent)?;
        }
        let mut health = private_options().write(true).create_new(true).open(health_path)?;
        health.write_all(b"1")?;
        health.sync_all()?;
        self.write(&record)?;
        self.prune_health(panel, owner);
        Ok(())
    }

    /// Persist a lifecycle event from a panel whose launch was registered.
    ///
    /// # Errors
    /// Rejects stale owners/session identities, invalid paths, contention or I/O failure.
    pub fn apply_hook(&self, panel: &str, kind: PanelKind, owner: &str, input: &HookInput, now: i64) -> io::Result<()> {
        if input.event.session_id.is_empty()
            || input.event.session_id.len() > 512
            || !input.cwd.is_absolute()
            || !input.transcript_path.is_absolute()
        {
            return Err(invalid("invalid lifecycle identity"));
        }
        let _lock = self.lock(panel)?;
        let mut record = self.read(panel)?.ok_or_else(|| invalid("unregistered panel launch"))?;
        if record.owner_token != owner
            || record.kind != kind
            || (!record.ledger.session_id.is_empty() && record.ledger.session_id != input.event.session_id)
        {
            return Err(invalid("lifecycle event belongs to a different panel process"));
        }
        let before_ledger = record.ledger.clone();
        let metadata_changed = record.cwd != input.cwd || record.transcript_path != input.transcript_path;
        record.cwd.clone_from(&input.cwd);
        record.transcript_path.clone_from(&input.transcript_path);
        record.ledger.apply(&input.event, now);
        let mut comparable = record.ledger.clone();
        comparable.updated_at_millis = before_ledger.updated_at_millis;
        if !metadata_changed && comparable == before_ledger {
            return Ok(());
        }
        self.write(&record)
    }

    /// Persist the working-turn identity before the caller sends a cancellation.
    /// A permission event racing this operation prevents an automatic handoff.
    ///
    /// # Errors
    /// Returns an error if the identity is stale, work is no longer running, or persistence fails.
    pub fn save_handoff(&self, owner: &str, mut handoff: SuspendRecord) -> io::Result<()> {
        let _lock = self.lock(&handoff.panel_local_id)?;
        let mut record = self
            .read(&handoff.panel_local_id)?
            .ok_or_else(|| invalid("missing work record"))?;
        if record.owner_token != owner
            || record.kind != handoff.kind
            || record.ledger.session_id != handoff.session_id
            || record.ledger.prompt_id.as_deref() != Some(&handoff.prompt_id)
            || record.ledger.generation != handoff.generation
            || record.ledger.state != TurnState::Working
            || record.ledger.ended_at_millis.is_some()
            || record.cwd != Path::new(&handoff.cwd)
        {
            return Err(invalid("working turn changed before suspend"));
        }
        handoff.final_transcript = None;
        record.handoff = Some(handoff);
        record.consumed = false;
        self.write(&record)
    }

    /// Seal the handoff only after the owning agent exited. The owner must not
    /// use a transcript read while that process is still running.
    ///
    /// # Errors
    /// Returns an error for stale/missing handoffs or persistence failure.
    pub fn finish_handoff(&self, panel: &str, owner: &str, snapshot: TranscriptSnapshot) -> io::Result<()> {
        let _lock = self.lock(panel)?;
        let mut record = self.read(panel)?.ok_or_else(|| invalid("missing work record"))?;
        if record.owner_token != owner || record.ledger.ended_at_millis.is_none() {
            return Err(invalid("agent has not acknowledged shutdown"));
        }
        let handoff = record.handoff.as_mut().ok_or_else(|| invalid("missing handoff"))?;
        if handoff.session_id != record.ledger.session_id
            || handoff.prompt_id != record.ledger.prompt_id.as_deref().unwrap_or("")
            || handoff.generation != record.ledger.generation
            || record.consumed
        {
            return Err(invalid("turn changed during shutdown"));
        }
        handoff.final_transcript = Some(snapshot);
        self.write(&record)
    }

    /// Claim once, before launching the resumed turn. A crash after claiming
    /// sacrifices automatic retry rather than submitting the same work twice.
    /// The final health check is the authorization point: later invalidation
    /// cannot revoke a returned claim. Call only after provider exit has been
    /// verified, immediately before dispatch; never queue or cache this result.
    ///
    /// # Errors
    /// Returns an error on contention, invalid records or persistence failure.
    pub fn claim_handoff(&self, expected: &StoredWork) -> io::Result<bool> {
        self.claim_with(expected, || {})
    }

    fn claim_with(&self, expected: &StoredWork, after_write: impl FnOnce()) -> io::Result<bool> {
        let _lock = self.lock(&expected.panel_local_id)?;
        let Some(mut current) = self.read(&expected.panel_local_id)? else {
            return Ok(false);
        };
        if current.handoff.as_ref().is_some_and(|handoff| {
            handoff.kind != current.kind
                || handoff.session_id != current.ledger.session_id
                || Some(handoff.prompt_id.as_str()) != current.ledger.prompt_id.as_deref()
                || handoff.generation != current.ledger.generation
        }) {
            return Ok(false);
        }
        if current.consumed
            || current.handoff.is_none()
            || current.handoff != expected.handoff
            || current.ledger != expected.ledger
            || current.owner_token != expected.owner_token
            || current.kind != expected.kind
            || current.cwd != expected.cwd
            || current.transcript_path != expected.transcript_path
        {
            return Ok(false);
        }
        let snapshot = TranscriptSnapshot::read(&current.transcript_path, current.kind)?;
        if current
            .handoff
            .as_ref()
            .and_then(|record| record.final_transcript.as_ref())
            != Some(&snapshot)
        {
            return Ok(false);
        }
        current.consumed = true;
        self.write(&current)?;
        after_write();
        self.check_health(&current.panel_local_id, &current.owner_token)?;
        Ok(true)
    }

    /// Invalidate the launch independently of the record lock. A dropped hook
    /// must veto resume even if another writer is stalled while holding that lock.
    ///
    /// # Errors
    /// Returns an I/O error; only this launch's pre-created health file is touched.
    pub fn invalidate(&self, panel: &str, owner: &str) -> io::Result<()> {
        if let Ok(file) = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(self.health_path(panel, owner)?)
        {
            file.sync_all()
        } else {
            // If the health marker is inaccessible, remove this owner's
            // record under the normal lock instead. A delayed old hook
            // must never remove a newer launch's record.
            let _lock = self.lock(panel)?;
            if self.read_raw(panel)?.is_some_and(|record| record.owner_token == owner) {
                fs::remove_file(self.record_path(panel)?)?;
                #[cfg(unix)]
                File::open(&self.root)?.sync_all()?;
            }
            Ok(())
        }
    }

    fn check_health(&self, panel: &str, owner: &str) -> io::Result<()> {
        let path = self.health_path(panel, owner)?;
        // A marker that can no longer be invalidated cannot authorize work.
        let _writable = OpenOptions::new().write(true).open(&path)?;
        if fs::metadata(&path)?.len() != 1 || fs::read(path)? != b"1" {
            return Err(invalid("launch evidence was invalidated by a failed hook"));
        }
        Ok(())
    }

    fn prune_health(&self, panel: &str, current: &str) {
        let Ok(entries) = fs::read_dir(self.root.join("health").join(panel)) else {
            return;
        };
        // Registration holds the panel lock and has durably published the new
        // owner. A late old hook now falls back to the owner-checked invalidator.
        for entry in entries.take(4096).flatten() {
            if entry.file_name() != current && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    fn health_path(&self, panel: &str, owner: &str) -> io::Result<PathBuf> {
        let _ = self.record_path(panel)?;
        if !valid_id(owner) {
            return Err(invalid("invalid launch token"));
        }
        Ok(self.root.join("health").join(panel).join(owner))
    }

    fn record_path(&self, panel: &str) -> io::Result<PathBuf> {
        if !valid_id(panel) {
            return Err(invalid("invalid panel identity"));
        }
        Ok(self.root.join(format!("{panel}.json")))
    }

    fn lock(&self, panel: &str) -> io::Result<File> {
        let path = self.record_path(panel)?.with_extension("lock");
        private_directory(&self.root)?;
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, "work ledger busy"));
                }
                Err(TryLockError::Error(error)) => return Err(error),
            }
        }
    }

    fn write(&self, record: &StoredWork) -> io::Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|_| invalid("cannot serialize work record"))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid("work record exceeds size limit"));
        }
        let path = self.record_path(&record.panel_local_id)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options.mode(0o600);
        options
    }
    #[cfg(not(unix))]
    options
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests;
