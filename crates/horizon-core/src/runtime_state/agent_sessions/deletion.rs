use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::super::AgentSessionKey;
use super::super::claude_live_sessions::verified_live_claude_session_ids;
use super::{AgentSessionBinding, AgentSessionCatalog, PanelKind, normalize_cwd};
use crate::error::{Error, Result};

static PENDING_DELETIONS: LazyLock<Mutex<HashSet<AgentSessionKey>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

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
    Ok(AgentSessionDeletionReservation { keys })
}

impl Drop for AgentSessionDeletionReservation {
    fn drop(&mut self) {
        let mut pending = PENDING_DELETIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|key| !self.keys.contains(key));
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
    #[must_use]
    pub fn supports_saved_session_deletion(kind: PanelKind) -> bool {
        matches!(kind, PanelKind::Codex | PanelKind::Claude)
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
    if verified_live_claude_session_ids(home)?.contains(&session.session_id) {
        return Err(Error::State("This conversation is open in Claude Code".into()));
    }
    delete_claude_transcript(&home.join(".claude/projects"), session)
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

fn delete_claude_transcript(projects: &Path, session: &AgentSessionBinding) -> Result<DeletionOutcome> {
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
    stage_claude_deletion(
        &root,
        path,
        artifacts,
        |from, to| std::fs::rename(from, to),
        |path| std::fs::remove_dir_all(path),
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ClaudeDeletionManifest {
    transcript: PathBuf,
    artifacts: Option<PathBuf>,
}

pub(super) fn retained_claude_recovery_ids(projects: &Path) -> Result<HashSet<String>> {
    let root = projects.canonicalize()?;
    let Some(parent) = root.parent() else {
        return Ok(HashSet::new());
    };
    let mut unavailable = HashSet::new();
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
            unavailable.insert(id.to_owned());
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
    std::fs::write(staging.path().join("original-paths.json"), manifest)?;
    // A retained bundle must never be purged by a temporary-directory destructor.
    let directory = staging.keep();
    let saved_transcript = directory.join("transcript.deleted");
    if let Err(error) = rename(transcript, &saved_transcript) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.into());
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
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.into());
    }
    // Both artifacts are now outside discovery. A purge error is cleanup pending,
    // never a failed deletion that would advertise partially removed history.
    match purge(&directory) {
        Ok(()) => Ok(DeletionOutcome::Removed),
        Err(error) => Ok(DeletionOutcome::CleanupPending {
            directory,
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::super::AgentSessionRecord;
    use super::*;

    fn binding(id: u128) -> AgentSessionBinding {
        AgentSessionBinding::new(
            PanelKind::Claude,
            uuid::Uuid::from_u128(id).to_string(),
            Some("/example".into()),
            None,
            None,
        )
    }

    #[test]
    fn invalid_live_registry_aborts_before_touching_transcript_or_artifacts() {
        let home = tempfile::tempdir().expect("home");
        let registry = home.path().join(".claude/sessions");
        let project = home.path().join(".claude/projects/example");
        std::fs::create_dir_all(&registry).expect("registry");
        std::fs::create_dir_all(&project).expect("project");
        let session = binding(801);
        let transcript = project.join(format!("{}.jsonl", session.session_id));
        let artifacts = project.join(&session.session_id);
        std::fs::create_dir_all(artifacts.join("subagents")).expect("artifacts");
        std::fs::write(&transcript, "original conversation").expect("transcript");
        std::fs::write(artifacts.join("subagents/agent.jsonl"), "original subagent").expect("subagent");
        std::fs::write(registry.join("101.json"), "malformed").expect("bad registry");
        assert!(delete_claude_session(home.path(), &session).is_err());
        assert_eq!(
            std::fs::read_to_string(&transcript).expect("preserved transcript"),
            "original conversation"
        );
        assert_eq!(
            std::fs::read_to_string(artifacts.join("subagents/agent.jsonl")).expect("preserved subagent"),
            "original subagent"
        );
    }

    #[test]
    fn batch_preserves_protected_sessions_deduplicates_and_reports_partial_failure() {
        let a = binding(1);
        let b = binding(2);
        let c = binding(3);
        let mut catalog = AgentSessionCatalog {
            sessions: [&a, &b, &c]
                .into_iter()
                .map(|binding| AgentSessionRecord {
                    kind: binding.kind,
                    session_id: binding.session_id.clone(),
                    cwd: binding.cwd.clone(),
                    label: None,
                    updated_at: 0,
                    interactive: true,
                })
                .collect(),
        };
        let protected = HashSet::from([AgentSessionKey::new(b.kind, &b.session_id)]);
        let report = catalog.delete_with(&[a.clone(), a.clone(), b, c.clone()], &protected, |session| {
            if session.session_id == c.session_id {
                Err(Error::State("synthetic provider failure".into()))
            } else {
                Ok(DeletionOutcome::Removed)
            }
        });
        assert_eq!(report.deleted.len(), 1);
        assert_eq!(report.failures.len(), 2);
        catalog.remove_deleted_sessions(&report);
        assert_eq!(catalog.sessions.len(), 2);
        assert!(
            !catalog
                .sessions
                .iter()
                .any(|session| session.session_id == a.session_id)
        );
    }

    #[test]
    fn transcript_deletion_removes_only_selected_conversation_and_its_subagents() {
        let temp = tempfile::tempdir().expect("temporary store");
        let project = temp.path().join("project");
        std::fs::create_dir(&project).expect("project");
        let session = binding(1);
        let path = project.join(format!("{}.jsonl", session.session_id));
        let contents = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user","message":{"content":"synthetic conversation"}}).to_string();
        std::fs::write(&path, contents).expect("transcript");
        let children = path.with_extension("").join("subagents");
        std::fs::create_dir_all(&children).expect("subagents");
        std::fs::write(children.join("agent.jsonl"), "synthetic child").expect("child");
        let unrelated = project.join("unrelated.jsonl");
        std::fs::write(&unrelated, "keep").expect("unrelated");
        delete_claude_transcript(temp.path(), &session).expect("delete");
        assert!(!path.exists());
        assert!(!children.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn invalid_or_unknown_selection_never_reaches_provider() {
        let catalog = AgentSessionCatalog::default();
        let mut invalid = binding(1);
        invalid.session_id = "../outside".into();
        let report = catalog.delete_with(&[invalid, binding(2)], &HashSet::new(), |_| panic!("must not execute"));
        assert_eq!(report.failures.len(), 2);
        assert!(report.deleted.is_empty());
    }

    #[test]
    fn reservations_exclude_queued_sessions_from_all_catalogs_and_panel_launches() {
        let session = binding(0xabc_987);
        let catalog = AgentSessionCatalog {
            sessions: vec![AgentSessionRecord {
                kind: session.kind,
                session_id: session.session_id.clone(),
                cwd: normalize_cwd(session.cwd.as_deref()),
                label: None,
                updated_at: 0,
                interactive: true,
            }],
        };
        let guard = reserve_saved_session_deletions(std::slice::from_ref(&session)).expect("reserve");
        assert!(saved_session_deletion_pending(session.kind, &session.session_id));
        assert!(
            catalog
                .clone()
                .recent_for(session.kind, session.cwd.as_deref())
                .is_empty()
        );
        assert!(reserve_saved_session_deletions(std::slice::from_ref(&session)).is_err());
        let mut board = crate::Board::new();
        let workspace = board.create_workspace("Synthetic protection test");
        assert!(
            board
                .create_panel(
                    crate::PanelOptions {
                        kind: session.kind,
                        resume: crate::PanelResume::Session {
                            session_id: session.session_id.clone()
                        },
                        ..Default::default()
                    },
                    workspace
                )
                .is_err()
        );
        assert!(
            board
                .create_panel(
                    crate::PanelOptions {
                        kind: session.kind,
                        resume: crate::PanelResume::Fresh,
                        session_binding: Some(session.clone()),
                        ..Default::default()
                    },
                    workspace
                )
                .is_err()
        );
        drop(guard);
        assert!(!saved_session_deletion_pending(session.kind, &session.session_id));
        assert_eq!(catalog.recent_for(session.kind, session.cwd.as_deref()).len(), 1);
    }

    fn staged_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("private store");
        let projects = temp.path().join("projects");
        let transcript = projects.join("project/conversation.jsonl");
        let artifacts = transcript.with_extension("");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        std::fs::write(&transcript, "original transcript").expect("transcript");
        std::fs::write(artifacts.join("agent.jsonl"), "original child").expect("child");
        (temp, projects, transcript, artifacts)
    }

    #[test]
    fn staging_failures_preserve_all_original_history() {
        for failed_move in [1, 2] {
            let (temp, projects, transcript, artifacts) = staged_fixture();
            let moves = std::cell::Cell::new(0);
            let result = stage_claude_deletion(
                &projects,
                &transcript,
                Some(&artifacts),
                |from, to| {
                    moves.set(moves.get() + 1);
                    if moves.get() == failed_move {
                        Err(std::io::Error::other("injected staging failure"))
                    } else {
                        std::fs::rename(from, to)
                    }
                },
                |_| panic!("staging failure must not purge history"),
            );
            assert!(result.is_err());
            assert_eq!(
                std::fs::read_to_string(&transcript).expect("transcript"),
                "original transcript"
            );
            assert_eq!(
                std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("child"),
                "original child"
            );
            assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
        }
    }

    #[test]
    fn rollback_failure_retains_recoverable_history_outside_discovery() {
        let (temp, projects, transcript, artifacts) = staged_fixture();
        let moves = std::cell::Cell::new(0);
        let outcome = stage_claude_deletion(
            &projects,
            &transcript,
            Some(&artifacts),
            |from, to| {
                moves.set(moves.get() + 1);
                if moves.get() > 1 {
                    Err(std::io::Error::other("injected move failure"))
                } else {
                    std::fs::rename(from, to)
                }
            },
            |_| panic!("rollback failure must not purge history"),
        )
        .expect("recovery outcome");
        let bundle = std::fs::read_dir(temp.path())
            .expect("store")
            .map(|entry| entry.expect("entry").path())
            .find(|path| path != &projects)
            .expect("recovery bundle");
        let DeletionOutcome::RecoveryRequired { directory, message } = outcome else {
            panic!("must require recovery")
        };
        assert_eq!(directory, bundle);
        assert!(message.contains("restoring the transcript failed"));
        assert_eq!(
            std::fs::read_to_string(bundle.join("transcript.deleted")).expect("transcript"),
            "original transcript"
        );
        assert_eq!(
            std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("child"),
            "original child"
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("original-paths.json")).expect("manifest"))
                .expect("paths");
        assert_eq!(manifest["transcript"], transcript.to_string_lossy().as_ref());
        assert!(!bundle.starts_with(&projects));
    }

    #[test]
    fn retained_manifest_prevents_legacy_artifacts_resurrecting_parent_on_reload() {
        let temp = tempfile::tempdir().expect("store");
        let projects = temp.path().join("projects");
        let session = binding(8);
        let transcript = projects.join("project").join(format!("{}.jsonl", session.session_id));
        let artifacts = transcript.with_extension("");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let payload = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user","message":{"content":"retained history"}}).to_string();
        std::fs::write(&transcript, &payload).expect("transcript");
        std::fs::write(artifacts.join("legacy-agent.jsonl"), &payload).expect("legacy artifact");
        let unaffected = binding(9);
        let other = transcript
            .parent()
            .expect("project")
            .join(format!("{}.jsonl", unaffected.session_id));
        std::fs::write(other, serde_json::json!({"sessionId":unaffected.session_id,"cwd":"/example","type":"user","message":{"content":"keep"}}).to_string()).expect("other");
        let moves = std::cell::Cell::new(0);
        let outcome = stage_claude_deletion(
            &projects,
            &transcript,
            Some(&artifacts),
            |from, to| {
                moves.set(moves.get() + 1);
                if moves.get() == 1 {
                    std::fs::rename(from, to)
                } else {
                    Err(std::io::Error::other("move blocked"))
                }
            },
            |_| panic!("never purge recovery history"),
        )
        .expect("recovery");
        let DeletionOutcome::RecoveryRequired { directory, .. } = outcome else {
            panic!("recovery required")
        };
        for _ in 0..2 {
            let loaded = super::super::load_claude_sessions_from_dir(&projects).expect("fresh discovery");
            assert!(!loaded.iter().any(|record| record.session_id == session.session_id));
            assert!(loaded.iter().any(|record| record.session_id == unaffected.session_id));
        }
        std::fs::rename(directory.join("transcript.deleted"), &transcript).expect("manual restore");
        assert!(
            super::super::load_claude_sessions_from_dir(&projects)
                .expect("restored discovery")
                .iter()
                .any(|record| record.session_id == session.session_id)
        );
    }

    #[test]
    fn recovery_required_is_removed_from_catalog_without_claiming_deletion() {
        let session = binding(7);
        let key = AgentSessionKey::new(session.kind, &session.session_id);
        let mut catalog = AgentSessionCatalog {
            sessions: vec![AgentSessionRecord {
                kind: session.kind,
                session_id: session.session_id.clone(),
                cwd: session.cwd.clone(),
                label: None,
                updated_at: 0,
                interactive: true,
            }],
        };
        let report = catalog.delete_with(std::slice::from_ref(&session), &HashSet::new(), |_| {
            Ok(DeletionOutcome::RecoveryRequired {
                directory: "/sample/recovery".into(),
                message: "Restore transcript first".into(),
            })
        });
        assert!(report.deleted.is_empty());
        assert!(report.failures.is_empty());
        assert_eq!(report.recoveries.len(), 1);
        assert_eq!(report.recoveries[0].key, key);
        catalog.remove_deleted_sessions(&report);
        assert!(catalog.recent_for(session.kind, session.cwd.as_deref()).is_empty());
    }

    #[test]
    fn partial_purge_is_cleanup_pending_and_never_a_resumable_failure() {
        let (temp, projects, transcript, artifacts) = staged_fixture();
        let session = binding(6);
        let mut catalog = AgentSessionCatalog {
            sessions: vec![AgentSessionRecord {
                kind: session.kind,
                session_id: session.session_id.clone(),
                cwd: session.cwd.clone(),
                label: None,
                updated_at: 0,
                interactive: true,
            }],
        };
        let report = catalog.delete_with(std::slice::from_ref(&session), &HashSet::new(), |_| {
            stage_claude_deletion(
                &projects,
                &transcript,
                Some(&artifacts),
                |from, to| std::fs::rename(from, to),
                |directory| {
                    std::fs::remove_file(directory.join("artifacts/agent.jsonl"))?;
                    Err(std::io::Error::other("injected partial purge"))
                },
            )
        });
        assert_eq!(report.deleted.len(), 1);
        assert!(report.failures.is_empty());
        assert_eq!(report.cleanup_warnings.len(), 1);
        let warning = &report.cleanup_warnings[0];
        assert_eq!(warning.session_id, session.session_id);
        assert!(warning.message.contains("injected partial purge"));
        assert!(warning.directory.starts_with(temp.path()));
        assert!(!warning.directory.starts_with(&projects));
        assert!(!transcript.exists());
        assert!(!artifacts.exists());
        assert!(warning.directory.join("transcript.deleted").exists());
        catalog.remove_deleted_sessions(&report);
        assert!(catalog.recent_for(session.kind, session.cwd.as_deref()).is_empty());
    }

    #[test]
    fn successful_staged_purge_removes_every_selected_byte() {
        let (temp, projects, transcript, artifacts) = staged_fixture();
        assert!(matches!(
            stage_claude_deletion(
                &projects,
                &transcript,
                Some(&artifacts),
                |from, to| std::fs::rename(from, to),
                |path| std::fs::remove_dir_all(path)
            )
            .expect("delete"),
            DeletionOutcome::Removed
        ));
        assert!(!transcript.exists());
        assert!(!artifacts.exists());
        assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn linked_transcripts_never_delete_their_target() {
        let temp = tempfile::tempdir().expect("private store");
        let projects = temp.path().join("projects");
        let project = projects.join("project");
        std::fs::create_dir_all(&project).expect("project");
        let session = binding(5);
        let outside = temp.path().join("outside.jsonl");
        std::fs::write(&outside, "keep").expect("outside");
        std::os::unix::fs::symlink(&outside, project.join(format!("{}.jsonl", session.session_id))).expect("link");
        assert!(delete_claude_transcript(&projects, &session).is_err());
        assert_eq!(std::fs::read_to_string(outside).expect("target preserved"), "keep");
    }
}
