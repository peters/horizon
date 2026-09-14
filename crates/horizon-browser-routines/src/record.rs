use uuid::Uuid;

use crate::RoutineError;
use crate::SCHEMA_VERSION;
use crate::recording::{RecordedAction, RecordedKind, SemanticRecording};
use crate::registry::{RoutineRegistry, create_private_dir, read_private_file, write_private};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeachLifecycle {
    Recording,
    Paused,
    Stopped,
    Discarded,
}

/// Explicit Teach-mode session: grouped semantic actions, never pointer-move samples.
pub struct TeachSession {
    recording: SemanticRecording,
    lifecycle: TeachLifecycle,
}

impl TeachSession {
    #[must_use]
    pub fn start() -> Self {
        Self {
            recording: SemanticRecording {
                schema_version: SCHEMA_VERSION,
                recording_id: Uuid::new_v4(),
                actions: Vec::new(),
            },
            lifecycle: TeachLifecycle::Recording,
        }
    }

    pub fn pause(&mut self) {
        if self.lifecycle == TeachLifecycle::Recording {
            self.lifecycle = TeachLifecycle::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.lifecycle == TeachLifecycle::Paused {
            self.lifecycle = TeachLifecycle::Recording;
        }
    }

    /// Freeze the recording for review. Resume and push are rejected; the
    /// actions stay available for `save_draft`.
    pub fn stop(&mut self) {
        if self.lifecycle != TeachLifecycle::Discarded {
            self.lifecycle = TeachLifecycle::Stopped;
        }
    }

    pub fn discard(&mut self) {
        self.lifecycle = TeachLifecycle::Discarded;
        self.recording.actions.clear();
    }

    /// Clear the session and delete any persisted `draft.json` for `routine_id`.
    ///
    /// # Errors
    /// Storage failure while removing the draft.
    pub fn discard_saved(&mut self, registry: &RoutineRegistry, routine_id: Uuid) -> Result<(), RoutineError> {
        remove_draft(registry, routine_id)?;
        self.discard();
        Ok(())
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.lifecycle == TeachLifecycle::Paused
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.lifecycle == TeachLifecycle::Stopped
    }

    #[must_use]
    pub fn is_discarded(&self) -> bool {
        self.lifecycle == TeachLifecycle::Discarded
    }

    #[must_use]
    pub fn recording(&self) -> &SemanticRecording {
        &self.recording
    }

    /// Append one grouped semantic action. Consecutive same-direction scrolls that
    /// share a target and have no assertion between them are coalesced.
    ///
    /// # Errors
    /// Paused, stopped, or discarded session, invalid action, or action bound exceeded.
    pub fn push(&mut self, action: RecordedAction) -> Result<(), RoutineError> {
        if self.lifecycle != TeachLifecycle::Recording {
            return Err(RoutineError::TeachInactive);
        }
        action.validate()?;
        if let Some(previous) = self.recording.actions.last_mut()
            && can_coalesce(previous, &action)
        {
            let previous_kind = previous.kind.clone();
            let previous_postcondition = previous.postcondition.clone();
            coalesce_scroll(previous, action)?;
            return match self.recording.validate() {
                Ok(()) => Ok(()),
                Err(error) => {
                    if let Some(restored) = self.recording.actions.last_mut() {
                        restored.kind = previous_kind;
                        restored.postcondition = previous_postcondition;
                    }
                    Err(error)
                }
            };
        }
        self.recording.actions.push(action);
        match self.recording.validate() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.recording.actions.pop();
                Err(error)
            }
        }
    }

    /// Write `draft.json` under the routine directory. Empty drafts are allowed
    /// so an interrupted session can be recovered.
    ///
    /// # Errors
    /// Discarded session or storage failure.
    pub fn save_draft(&self, registry: &RoutineRegistry, routine_id: Uuid) -> Result<(), RoutineError> {
        if self.lifecycle == TeachLifecycle::Discarded {
            return Err(RoutineError::TeachInactive);
        }
        let dir = registry.directory().join(routine_id.to_string());
        create_private_dir(&dir)?;
        let _lock = registry.lock(routine_id)?;
        let path = dir.join("draft.json");
        let encoded = if self.recording.actions.is_empty() {
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "recording_id": self.recording.recording_id,
                "actions": []
            }))
            .map_err(|_| RoutineError::Json("malformed routine JSON".into()))?
        } else {
            self.recording.to_redacted_json()?.into_bytes()
        };
        write_private(&path, &encoded)
    }

    /// # Errors
    /// Missing or malformed draft.
    pub fn load_draft(registry: &RoutineRegistry, routine_id: Uuid) -> Result<Self, RoutineError> {
        let dir = registry.directory().join(routine_id.to_string());
        match std::fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(RoutineError::Storage);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RoutineError::RoutineNotFound);
            }
            Err(_) => return Err(RoutineError::Storage),
        }
        let _lock = registry.lock(routine_id)?;
        let path = dir.join("draft.json");
        let bytes = read_private_file(&path)?;
        let recording: SemanticRecording =
            serde_json::from_slice(&bytes).map_err(|_| RoutineError::Json("malformed routine JSON".into()))?;
        if recording.actions.is_empty() {
            if recording.schema_version != SCHEMA_VERSION {
                return Err(RoutineError::UnsupportedSchema(recording.schema_version));
            }
        } else {
            recording.validate()?;
        }
        Ok(Self {
            recording,
            lifecycle: TeachLifecycle::Paused,
        })
    }
}

fn remove_draft(registry: &RoutineRegistry, routine_id: Uuid) -> Result<(), RoutineError> {
    let dir = registry.directory().join(routine_id.to_string());
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(RoutineError::Storage);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(RoutineError::Storage),
    }
    let _lock = registry.lock(routine_id)?;
    match std::fs::remove_file(dir.join("draft.json")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RoutineError::Storage),
    }
}

fn can_coalesce(previous: &RecordedAction, next: &RecordedAction) -> bool {
    match (&previous.kind, &next.kind) {
        (
            RecordedKind::Scroll {
                delta_x: previous_x,
                delta_y: previous_y,
            },
            RecordedKind::Scroll {
                delta_x: next_x,
                delta_y: next_y,
            },
        ) => {
            previous.target == next.target
                && previous.page_origin == next.page_origin
                && previous.url_pattern == next.url_pattern
                && previous.mutation_class == next.mutation_class
                && previous.postcondition.is_none()
                && next.precondition.is_none()
                && same_direction(*previous_x, *next_x)
                && same_direction(*previous_y, *next_y)
        }
        _ => false,
    }
}

fn same_direction(previous: f64, next: f64) -> bool {
    previous == 0.0 || next == 0.0 || previous.signum() == next.signum()
}

fn coalesce_scroll(previous: &mut RecordedAction, next: RecordedAction) -> Result<(), RoutineError> {
    let RecordedKind::Scroll { delta_x, delta_y } = &mut previous.kind else {
        return Err(RoutineError::InvalidRecording);
    };
    let RecordedKind::Scroll {
        delta_x: next_x,
        delta_y: next_y,
    } = next.kind
    else {
        return Err(RoutineError::InvalidRecording);
    };
    let summed_x = *delta_x + next_x;
    let summed_y = *delta_y + next_y;
    if !summed_x.is_finite() || !summed_y.is_finite() {
        return Err(RoutineError::InvalidRecording);
    }
    *delta_x = summed_x;
    *delta_y = summed_y;
    previous.postcondition = next.postcondition;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TeachSession;
    use crate::Origin;
    use crate::RoutineError;
    use crate::fingerprint::{FrameContext, RankedCandidate, TargetCandidate, TargetFingerprint};
    use crate::recording::{MutationClass, RecordedAction, RecordedKind};
    use crate::registry::RoutineRegistry;
    use crate::value::FieldClassification;
    use uuid::Uuid;

    fn origin() -> Origin {
        Origin::parse("https://reports.example").expect("origin")
    }

    fn click(id: &str) -> RecordedAction {
        RecordedAction {
            action_id: id.to_string(),
            recorded_at_millis: 1,
            kind: RecordedKind::Click { count: 1 },
            target: Some(TargetFingerprint {
                candidates: vec![RankedCandidate {
                    identity: TargetCandidate::RoleName {
                        role: "button".to_string(),
                        name: "Generate report".to_string(),
                        reviewed: false,
                    },
                    match_count: 1,
                    unique: true,
                }],
                selected: Some(0),
                frame: FrameContext {
                    top_level: true,
                    origin: origin(),
                    chain: Vec::new(),
                },
                digest: "el-1".to_string(),
            }),
            page_origin: origin(),
            url_pattern: "https://reports.example/app".to_string(),
            navigation: None,
            value_source: None,
            field_classification: FieldClassification::Ordinary,
            mutation_class: MutationClass::ReadOnly,
            precondition: None,
            postcondition: None,
        }
    }

    fn scroll(id: &str, delta_y: f64) -> RecordedAction {
        RecordedAction {
            action_id: id.to_string(),
            recorded_at_millis: 1,
            kind: RecordedKind::Scroll { delta_x: 0.0, delta_y },
            target: None,
            page_origin: origin(),
            url_pattern: "https://reports.example/app".to_string(),
            navigation: None,
            value_source: None,
            field_classification: FieldClassification::Ordinary,
            mutation_class: MutationClass::ReadOnly,
            precondition: None,
            postcondition: None,
        }
    }

    #[cfg(unix)]
    fn privatize_temp(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(path).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }

    #[cfg(not(unix))]
    fn privatize_temp(_path: &std::path::Path) {}

    #[test]
    fn pause_rejects_new_actions_and_resume_accepts_them() {
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        session.pause();
        assert_eq!(session.push(click("again")), Err(RoutineError::TeachInactive));
        session.resume();
        session.push(click("later")).expect("resume");
        assert_eq!(session.recording().actions.len(), 2);
    }

    #[test]
    fn scroll_bursts_are_coalesced_and_pointer_moves_are_not_an_action_kind() {
        let mut session = TeachSession::start();
        session.push(scroll("s1", 80.0)).expect("s1");
        session.push(scroll("s2", 40.0)).expect("s2");
        assert_eq!(session.recording().actions.len(), 1);
        assert!(matches!(
            session.recording().actions[0].kind,
            RecordedKind::Scroll { delta_y: 120.0, .. }
        ));
    }

    #[test]
    fn stop_rejects_resume_and_push_but_retains_the_recording() {
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        session.stop();
        assert!(session.is_stopped());
        assert!(!session.is_paused());
        session.resume();
        assert!(session.is_stopped());
        assert_eq!(session.push(click("again")), Err(RoutineError::TeachInactive));
        assert_eq!(session.recording().actions.len(), 1);
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        session.save_draft(&registry, Uuid::from_u128(9)).expect("save stopped");
    }

    #[test]
    fn empty_draft_rejects_unknown_fields() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let session = TeachSession::start();
        let id = Uuid::from_u128(10);
        session.save_draft(&registry, id).expect("save");
        let path = temp.path().join("routines").join(id.to_string()).join("draft.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":1,"recording_id":"{}","actions":[],"extra":true}}"#,
                session.recording().recording_id
            ),
        )
        .expect("overwrite");
        assert_eq!(
            TeachSession::load_draft(&registry, id).err(),
            Some(RoutineError::Json("malformed routine JSON".into()))
        );
    }

    #[test]
    fn discarded_session_cannot_save_and_clears_actions() {
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        session.discard();
        assert!(session.recording().actions.is_empty());
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        assert_eq!(
            session.save_draft(&registry, Uuid::nil()),
            Err(RoutineError::TeachInactive)
        );
    }

    #[test]
    fn discard_saved_removes_the_persisted_draft() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(8);
        session.save_draft(&registry, id).expect("save");
        session.discard_saved(&registry, id).expect("discard");
        assert!(session.recording().actions.is_empty());
        assert_eq!(
            TeachSession::load_draft(&registry, id).err(),
            Some(RoutineError::RoutineNotFound)
        );
    }

    #[test]
    fn opposite_scroll_is_not_coalesced() {
        let mut session = TeachSession::start();
        session.push(scroll("s1", 80.0)).expect("s1");
        session.push(scroll("s2", -40.0)).expect("s2");
        assert_eq!(session.recording().actions.len(), 2);
    }

    #[test]
    fn targetless_scrolls_do_not_coalesce_across_pages() {
        let mut session = TeachSession::start();
        let mut first = scroll("s1", 80.0);
        first.page_origin = Origin::parse("https://reports.example").expect("origin");
        first.url_pattern = "https://reports.example/app".to_string();
        let mut second = scroll("s2", 40.0);
        second.page_origin = Origin::parse("https://idp.example").expect("origin");
        second.url_pattern = "https://idp.example/login".to_string();
        session.push(first).expect("s1");
        session.push(second).expect("s2");
        assert_eq!(session.recording().actions.len(), 2);
    }

    #[test]
    fn rejected_push_does_not_leave_the_session_invalid() {
        let mut session = TeachSession::start();
        for index in 0..256 {
            session.push(click(&format!("a{index}"))).expect("fill");
        }
        assert_eq!(session.push(click("overflow")), Err(RoutineError::InvalidRecording));
        assert_eq!(session.recording().actions.len(), 256);
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        session.save_draft(&registry, Uuid::from_u128(5)).expect("save");
    }

    #[cfg(unix)]
    #[test]
    fn draft_load_rejects_symlinks() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(6);
        session.save_draft(&registry, id).expect("save");
        let draft = temp.path().join("routines").join(id.to_string()).join("draft.json");
        let target = temp.path().join("outside.json");
        std::fs::write(&target, b"{}").expect("outside");
        std::fs::remove_file(&draft).expect("remove");
        std::os::unix::fs::symlink(&target, &draft).expect("symlink");
        assert_eq!(
            TeachSession::load_draft(&registry, id).err(),
            Some(RoutineError::Storage)
        );
    }

    #[cfg(unix)]
    #[test]
    fn draft_load_rejects_symlinked_routine_directories() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(7);
        session.save_draft(&registry, id).expect("save");
        let real = temp.path().join("routines").join(id.to_string());
        let outside = temp.path().join("outside-dir");
        std::fs::rename(&real, &outside).expect("move");
        std::os::unix::fs::symlink(&outside, &real).expect("symlink");
        assert_eq!(
            TeachSession::load_draft(&registry, id).err(),
            Some(RoutineError::Storage)
        );
    }

    #[test]
    fn empty_draft_round_trips() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let session = TeachSession::start();
        let id = Uuid::from_u128(4);
        session.save_draft(&registry, id).expect("save");
        let loaded = TeachSession::load_draft(&registry, id).expect("load");
        assert!(loaded.recording().actions.is_empty());
        assert!(loaded.is_paused());
        assert_eq!(loaded.recording().recording_id, session.recording().recording_id);
    }

    #[test]
    fn recovered_draft_stays_paused_until_resume() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(11);
        session.save_draft(&registry, id).expect("save");
        let mut loaded = TeachSession::load_draft(&registry, id).expect("load");
        assert!(loaded.is_paused());
        assert_eq!(loaded.push(click("again")), Err(RoutineError::TeachInactive));
        loaded.resume();
        loaded.push(click("later")).expect("resume");
        assert_eq!(loaded.recording().actions.len(), 2);
    }

    #[test]
    fn discard_saved_keeps_the_session_when_draft_removal_fails() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(12);
        session.save_draft(&registry, id).expect("save");
        let draft = temp.path().join("routines").join(id.to_string()).join("draft.json");
        std::fs::remove_file(&draft).expect("remove");
        std::fs::create_dir(&draft).expect("dir");
        assert_eq!(session.discard_saved(&registry, id).err(), Some(RoutineError::Storage));
        assert!(!session.is_discarded());
        assert_eq!(session.recording().actions.len(), 1);
    }

    #[test]
    fn draft_round_trip_uses_private_mode() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let mut session = TeachSession::start();
        session.push(click("run")).expect("push");
        let id = Uuid::from_u128(3);
        session.save_draft(&registry, id).expect("save");
        let loaded = TeachSession::load_draft(&registry, id).expect("load");
        assert_eq!(loaded.recording().actions.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let file = temp.path().join("routines").join(id.to_string()).join("draft.json");
            let mode = std::fs::metadata(file).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
