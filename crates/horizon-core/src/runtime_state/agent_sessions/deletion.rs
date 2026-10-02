use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::super::AgentSessionKey;
use super::super::claude_live_sessions::verified_live_claude_session_ids;
use super::{AgentSessionBinding, AgentSessionCatalog, PanelKind, normalize_cwd};
use crate::error::{Error, Result};

const MAX_TRANSCRIPT_RECORD_BYTES: usize = 16 * 1024 * 1024;

static PENDING_DELETIONS: LazyLock<Mutex<HashSet<AgentSessionKey>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static PENDING_DELETION_REVISION: AtomicU64 = AtomicU64::new(0);

pub struct AgentSessionDeletionReservation {
    keys: HashSet<AgentSessionKey>,
}

#[must_use]
pub fn saved_session_deletion_pending(kind: PanelKind, id: &str) -> bool {
    PENDING_DELETIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&AgentSessionKey::new(kind, id))
}

/// Reserve conversation identities across catalogs and viewports until the guard is dropped.
///
/// # Errors
/// Returns an error if any conversation is already being deleted.
pub fn reserve_saved_session_deletions(sessions: &[AgentSessionBinding]) -> Result<AgentSessionDeletionReservation> {
    let keys: HashSet<_> = sessions
        .iter()
        .map(|session| AgentSessionKey::new(session.kind, &session.session_id))
        .collect();
    let mut pending = PENDING_DELETIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if keys.iter().any(|key| pending.contains(key)) {
        return Err(Error::State("A selected conversation is already being deleted".into()));
    }
    pending.extend(keys.iter().cloned());
    PENDING_DELETION_REVISION.fetch_add(1, Ordering::Release);
    Ok(AgentSessionDeletionReservation { keys })
}

impl Drop for AgentSessionDeletionReservation {
    fn drop(&mut self) {
        let mut pending = PENDING_DELETIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|key| !self.keys.contains(key));
        PENDING_DELETION_REVISION.fetch_add(1, Ordering::Release);
    }
}

impl AgentSessionDeletionReservation {
    #[must_use]
    pub fn delete_saved_sessions(
        &self,
        catalog: &AgentSessionCatalog,
        sessions: &[AgentSessionBinding],
        protected: &HashSet<AgentSessionKey>,
    ) -> AgentSessionDeletionReport {
        catalog.delete_with(sessions, protected, |session| {
            if !self
                .keys
                .contains(&AgentSessionKey::new(session.kind, &session.session_id))
            {
                return Err(Error::State("Conversation was not reserved for deletion".into()));
            }
            delete_provider_session(session)
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct AgentSessionDeletionReport {
    pub deleted: Vec<AgentSessionKey>,
    pub failures: Vec<AgentSessionDeletionFailure>,
    pub cleanup_warnings: Vec<AgentSessionDeletionCleanupWarning>,
    pub recoveries: Vec<AgentSessionDeletionRecovery>,
}

#[derive(Clone, Debug)]
pub struct AgentSessionDeletionFailure {
    pub session_id: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct AgentSessionDeletionCleanupWarning {
    pub session_id: String,
    pub directory: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct AgentSessionDeletionRecovery {
    pub key: AgentSessionKey,
    pub session_id: String,
    pub directory: PathBuf,
    pub message: String,
}

enum DeletionOutcome {
    Removed,
    CleanupPending { directory: PathBuf, message: String },
    RecoveryRequired { directory: PathBuf, message: String },
}

impl AgentSessionCatalog {
    /// Changes whenever process-wide deletion reservations change.
    #[must_use]
    pub fn pending_deletion_revision() -> u64 {
        PENDING_DELETION_REVISION.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn supports_saved_session_deletion(kind: PanelKind) -> bool {
        kind == PanelKind::Codex || (cfg!(unix) && kind == PanelKind::Claude)
    }

    /// Permanently remove explicitly selected, unprotected saved conversations.
    /// Provider operations run synchronously; callers should use a worker thread.
    #[must_use]
    pub fn delete_saved_sessions(
        &self,
        sessions: &[AgentSessionBinding],
        protected: &HashSet<AgentSessionKey>,
    ) -> AgentSessionDeletionReport {
        match reserve_saved_session_deletions(sessions) {
            Ok(reservation) => reservation.delete_saved_sessions(self, sessions, protected),
            Err(error) => AgentSessionDeletionReport {
                failures: sessions
                    .iter()
                    .map(|session| AgentSessionDeletionFailure {
                        session_id: session.session_id.clone(),
                        message: error.to_string(),
                    })
                    .collect(),
                ..Default::default()
            },
        }
    }

    pub fn remove_deleted_sessions(&mut self, report: &AgentSessionDeletionReport) {
        self.sessions.retain(|session| {
            !report
                .deleted
                .contains(&AgentSessionKey::new(session.kind, &session.session_id))
                && !report
                    .recoveries
                    .iter()
                    .any(|recovery| recovery.key == AgentSessionKey::new(session.kind, &session.session_id))
        });
    }

    fn delete_with(
        &self,
        sessions: &[AgentSessionBinding],
        protected: &HashSet<AgentSessionKey>,
        delete: impl Fn(&AgentSessionBinding) -> Result<DeletionOutcome>,
    ) -> AgentSessionDeletionReport {
        let mut report = AgentSessionDeletionReport::default();
        let mut seen = HashSet::new();
        for session in sessions {
            let key = AgentSessionKey::new(session.kind, &session.session_id);
            if !seen.insert(key.clone()) {
                continue;
            }
            let result = self
                .validate_deletion(session, protected)
                .and_then(|()| delete(session));
            match result {
                Ok(DeletionOutcome::RecoveryRequired { directory, message }) => {
                    report.recoveries.push(AgentSessionDeletionRecovery {
                        key,
                        session_id: session.session_id.clone(),
                        directory,
                        message,
                    });
                }
                Ok(outcome) => {
                    report.deleted.push(key);
                    if let DeletionOutcome::CleanupPending { directory, message } = outcome {
                        report.cleanup_warnings.push(AgentSessionDeletionCleanupWarning {
                            session_id: session.session_id.clone(),
                            directory,
                            message,
                        });
                    }
                }
                Err(error) => report.failures.push(AgentSessionDeletionFailure {
                    session_id: session.session_id.clone(),
                    message: error.to_string(),
                }),
            }
        }
        report
    }

    fn validate_deletion(&self, session: &AgentSessionBinding, protected: &HashSet<AgentSessionKey>) -> Result<()> {
        let key = AgentSessionKey::new(session.kind, &session.session_id);
        if protected.contains(&key) {
            return Err(Error::State("This session is attached to an open panel".into()));
        }
        if !Self::supports_saved_session_deletion(session.kind) {
            return Err(Error::State(
                "Deleting saved conversations is unavailable for this provider".into(),
            ));
        }
        if uuid::Uuid::parse_str(&session.session_id).is_err() {
            return Err(Error::State("Invalid saved conversation ID".into()));
        }
        if !self.sessions.iter().any(|record| {
            record.kind == session.kind
                && record.session_id == session.session_id
                && normalize_cwd(record.cwd.as_deref()) == normalize_cwd(session.cwd.as_deref())
        }) {
            return Err(Error::State(
                "This conversation is no longer in the selected catalog".into(),
            ));
        }
        Ok(())
    }
}

fn delete_provider_session(session: &AgentSessionBinding) -> Result<DeletionOutcome> {
    match session.kind {
        PanelKind::Codex => delete_codex_session(&session.session_id).map(|()| DeletionOutcome::Removed),
        PanelKind::Claude => {
            let home = std::env::var_os("HOME").ok_or_else(|| Error::State("HOME is unavailable".into()))?;
            delete_claude_session(Path::new(&home), session)
        }
        _ => Err(Error::State("Unsupported deletion provider".into())),
    }
}

fn delete_claude_session(home: &Path, session: &AgentSessionBinding) -> Result<DeletionOutcome> {
    verify_claude_session_inactive(home, session)?;
    delete_claude_transcript_with(&home.join(".claude/projects"), session, || {
        verify_claude_session_inactive(home, session)
    })
}

fn verify_claude_session_inactive(home: &Path, session: &AgentSessionBinding) -> Result<()> {
    let id = uuid::Uuid::parse_str(&session.session_id)
        .map_err(|_| Error::State("Invalid Claude conversation ID".into()))?;
    if verified_live_claude_session_ids(home)?.contains(&id.to_string()) {
        return Err(Error::State("This conversation is open in Claude Code".into()));
    }
    Ok(())
}

fn delete_codex_session(session_id: &str) -> Result<()> {
    let stderr = tempfile::tempfile()?;
    let mut child = Command::new("codex")
        .args(["delete", "--force", session_id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr.try_clone()?)
        .spawn()
        .map_err(|error| {
            Error::State(format!(
                "Cannot run codex delete; install a Codex CLI that supports it: {error}"
            ))
        })?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            let mut stderr = stderr;
            stderr.seek(SeekFrom::Start(0))?;
            let mut message = String::new();
            stderr.take(4096).read_to_string(&mut message)?;
            return Err(Error::State(format!(
                "Codex could not delete the conversation: {}",
                message.trim()
            )));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::State(
                "Codex deletion timed out; refresh the list before retrying".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn delete_claude_transcript_with(
    projects: &Path,
    session: &AgentSessionBinding,
    verify_inactive: impl FnOnce() -> Result<()>,
) -> Result<DeletionOutcome> {
    if uuid::Uuid::parse_str(&session.session_id).is_err() {
        return Err(Error::State("Invalid Claude conversation ID".into()));
    }
    let root = projects.canonicalize()?;
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join(format!("{}.jsonl", session.session_id));
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_file() {
            return Err(Error::State(
                "Refusing to delete a linked or non-file transcript".into(),
            ));
        }
        validate_claude_transcript_identity(&path, session)?;
        let record = super::load_claude_project_session_summary(&path, 0)?;
        if !record.is_some_and(|record| {
            record.session_id == session.session_id
                && normalize_cwd(record.cwd.as_deref()) == normalize_cwd(session.cwd.as_deref())
        }) {
            return Err(Error::State("Claude transcript identity or folder changed".into()));
        }
        matches.push(path);
    }
    if matches.len() != 1 {
        return Err(Error::State("Expected one unambiguous Claude transcript".into()));
    }
    let path = &matches[0];
    let children = path.with_extension("");
    let artifacts = match std::fs::symlink_metadata(&children) {
        Ok(metadata) if metadata.file_type().is_dir() => Some(children.as_path()),
        Ok(_) => return Err(Error::State("Refusing to delete linked conversation artifacts".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    verify_inactive()?;
    stage_claude_deletion(
        &root,
        path,
        artifacts,
        |from, to| std::fs::rename(from, to),
        |path| std::fs::remove_dir_all(path),
    )
}

#[derive(serde::Deserialize)]
struct ClaudeTranscriptIdentity {
    #[serde(
        rename = "sessionId",
        default,
        deserialize_with = "deserialize_present_identity_field"
    )]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_identity_field")]
    cwd: Option<String>,
}

fn deserialize_present_identity_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn validate_claude_transcript_identity(path: &Path, session: &AgentSessionBinding) -> Result<()> {
    let expected_cwd = normalize_cwd(session.cwd.as_deref());
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut line = String::new();
    let mut identity_found = false;
    loop {
        line.clear();
        let bytes = reader
            .by_ref()
            .take(MAX_TRANSCRIPT_RECORD_BYTES as u64 + 1)
            .read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        if bytes > MAX_TRANSCRIPT_RECORD_BYTES {
            return Err(Error::State(
                "Claude transcript record exceeds the 16 MiB validation limit".into(),
            ));
        }
        if line.trim().is_empty() {
            continue;
        }
        if !line.trim_start().starts_with('{') {
            return Err(Error::State(
                "Cannot verify identity of malformed Claude transcript".into(),
            ));
        }
        let identity: ClaudeTranscriptIdentity = serde_json::from_str(&line)
            .map_err(|_| Error::State("Cannot verify identity of malformed Claude transcript".into()))?;
        if let Some(cwd) = identity.cwd
            && (cwd.is_empty() || normalize_cwd(Some(&cwd)) != expected_cwd)
        {
            return Err(Error::State("Claude transcript has a conflicting folder".into()));
        }
        if let Some(id) = identity.session_id {
            if id.is_empty() || id != session.session_id {
                return Err(Error::State(
                    "Claude transcript has an invalid or conflicting session ID".into(),
                ));
            }
            identity_found = true;
        }
    }
    if !identity_found {
        return Err(Error::State("Claude transcript has no embedded session ID".into()));
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ClaudeDeletionManifest {
    transcript: PathBuf,
    artifacts: Option<PathBuf>,
}

#[derive(Default)]
pub(super) struct ClaudeRecoveryExclusions {
    pub session_ids: HashSet<String>,
    pub artifact_directories: HashSet<PathBuf>,
}

pub(super) fn retained_claude_recovery_exclusions(projects: &Path) -> Result<ClaudeRecoveryExclusions> {
    let root = projects.canonicalize()?;
    let Some(parent) = root.parent() else {
        return Ok(ClaudeRecoveryExclusions::default());
    };
    let mut unavailable = ClaudeRecoveryExclusions::default();
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.file_name().to_string_lossy().starts_with(".horizon-delete-") {
            continue;
        }
        let bundle = entry.path();
        let manifest_path = bundle.join("original-paths.json");
        let retained = bundle.join("transcript.deleted");
        if !std::fs::symlink_metadata(&manifest_path).is_ok_and(|metadata| metadata.file_type().is_file())
            || !std::fs::symlink_metadata(&retained).is_ok_and(|metadata| metadata.file_type().is_file())
        {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(manifest_path)?
            .take(65_536)
            .read_to_end(&mut bytes)?;
        let Ok(manifest) = serde_json::from_slice::<ClaudeDeletionManifest>(&bytes) else {
            continue;
        };
        let original_parent = manifest.transcript.parent().and_then(|path| path.canonicalize().ok());
        if original_parent.is_some_and(|parent| parent.starts_with(&root))
            && !manifest.transcript.is_file()
            && let Some(id) = manifest.transcript.file_stem().and_then(std::ffi::OsStr::to_str)
            && uuid::Uuid::parse_str(id).is_ok()
        {
            unavailable.session_ids.insert(id.to_owned());
            if let Some(artifacts) = manifest.artifacts
                && artifacts == manifest.transcript.with_extension("")
                && let Ok(artifacts) = artifacts.canonicalize()
                && artifacts.starts_with(&root)
            {
                unavailable.artifact_directories.insert(artifacts);
            }
        }
    }
    Ok(unavailable)
}

fn stage_claude_deletion(
    projects: &Path,
    transcript: &Path,
    artifacts: Option<&Path>,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
    purge: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<DeletionOutcome> {
    if !cfg!(unix) {
        return Err(Error::State(
            "Claude deletion requires durable directory updates, supported only on Unix hosts".into(),
        ));
    }
    stage_claude_deletion_with_sync(projects, transcript, artifacts, rename, purge, sync_deletion_directory)
}

fn sync_deletion_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

fn stage_claude_deletion_with_sync(
    projects: &Path,
    transcript: &Path,
    artifacts: Option<&Path>,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
    purge: impl Fn(&Path) -> std::io::Result<()>,
    sync: impl Fn(&Path) -> std::io::Result<()>,
) -> Result<DeletionOutcome> {
    let parent = projects
        .parent()
        .ok_or_else(|| Error::State("Claude storage has no staging parent".into()))?;
    // Discovery recursively scans projects, so retained bundles must live outside it.
    let staging = tempfile::Builder::new().prefix(".horizon-delete-").tempdir_in(parent)?;
    let manifest = serde_json::to_vec(&ClaudeDeletionManifest {
        transcript: transcript.to_owned(),
        artifacts: artifacts.map(Path::to_owned),
    })
    .map_err(std::io::Error::other)?;
    let mut manifest_file = std::fs::File::create(staging.path().join("original-paths.json"))?;
    manifest_file.write_all(&manifest)?;
    manifest_file.sync_all()?;
    drop(manifest_file);
    sync(staging.path())?;
    sync(parent)?;
    let source = transcript
        .parent()
        .ok_or_else(|| Error::State("Transcript has no parent".into()))?;
    let sync_moves = |directory: &Path| sync(directory).and_then(|()| sync(source));
    // A retained bundle must never be purged by a temporary-directory destructor.
    let directory = staging.keep();
    let saved_transcript = directory.join("transcript.deleted");
    if let Err(error) = rename(transcript, &saved_transcript) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.into());
    }
    if let Err(error) = sync_moves(&directory) {
        return Ok(DeletionOutcome::RecoveryRequired {
            directory,
            message: format!(
                "Transcript staged but directory sync failed: {error}. Retained history has not been purged."
            ),
        });
    }
    if let Some(artifacts) = artifacts
        && let Err(error) = rename(artifacts, &directory.join("artifacts"))
    {
        if let Err(rollback) = rename(&saved_transcript, transcript) {
            return Ok(DeletionOutcome::RecoveryRequired {
                message: format!(
                    "Conversation staging failed: {error}; restoring the transcript failed: {rollback}. Original paths are recorded in original-paths.json. Restore the saved transcript before resuming."
                ),
                directory,
            });
        }
        if let Err(sync_error) = sync_moves(&directory) {
            return Ok(DeletionOutcome::RecoveryRequired {
                directory,
                message: format!(
                    "Transcript restored but directory sync failed: {sync_error}. Inspect original paths before retrying."
                ),
            });
        }
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.into());
    }
    if let Err(error) = sync_moves(&directory) {
        return Ok(DeletionOutcome::RecoveryRequired {
            directory,
            message: format!(
                "History staged but directory sync failed: {error}. Retained history has not been purged."
            ),
        });
    }
    // Both artifacts are now outside discovery. A purge error is cleanup pending,
    // never a failed deletion that would advertise partially removed history.
    match purge(&directory).and_then(|()| sync(parent)) {
        Ok(()) => Ok(DeletionOutcome::Removed),
        Err(error) => Ok(DeletionOutcome::CleanupPending {
            directory,
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests;
