use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::super::{AgentSessionKey, live_claude_session_ids};
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
}

#[derive(Clone, Debug)]
pub struct AgentSessionDeletionFailure {
    pub session_id: String,
    pub message: String,
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
        });
    }

    fn delete_with(
        &self,
        sessions: &[AgentSessionBinding],
        protected: &HashSet<AgentSessionKey>,
        delete: impl Fn(&AgentSessionBinding) -> Result<()>,
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
                Ok(()) => report.deleted.push(key),
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

fn delete_provider_session(session: &AgentSessionBinding) -> Result<()> {
    match session.kind {
        PanelKind::Codex => delete_codex_session(&session.session_id),
        PanelKind::Claude => {
            if live_claude_session_ids().contains(&session.session_id) {
                return Err(Error::State("This conversation is open in Claude Code".into()));
            }
            let home = std::env::var_os("HOME").ok_or_else(|| Error::State("HOME is unavailable".into()))?;
            delete_claude_transcript(&Path::new(&home).join(".claude/projects"), session)
        }
        _ => Err(Error::State("Unsupported deletion provider".into())),
    }
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

fn delete_claude_transcript(projects: &Path, session: &AgentSessionBinding) -> Result<()> {
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
    match std::fs::symlink_metadata(&children) {
        Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(children)?,
        Ok(_) => return Err(Error::State("Refusing to delete linked conversation artifacts".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::remove_file(path)?;
    Ok(())
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
                Ok(())
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
                cwd: session.cwd.clone(),
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
