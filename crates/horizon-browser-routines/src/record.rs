use uuid::Uuid;

use crate::RoutineError;
use crate::SCHEMA_VERSION;
use crate::recording::{RecordedAction, RecordedKind, SemanticRecording};
use crate::registry::{RoutineRegistry, create_private_dir, read_private_file, write_private};

/// Explicit Teach-mode session: grouped semantic actions, never pointer-move samples.
pub struct TeachSession {
    recording: SemanticRecording,
    paused: bool,
    discarded: bool,
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
            paused: false,
            discarded: false,
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        if !self.discarded {
            self.paused = false;
        }
    }

    pub fn discard(&mut self) {
        self.discarded = true;
        self.paused = true;
        self.recording.actions.clear();
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    #[must_use]
    pub fn is_discarded(&self) -> bool {
        self.discarded
    }

    #[must_use]
    pub fn recording(&self) -> &SemanticRecording {
        &self.recording
    }

    /// Append one grouped semantic action. Consecutive same-direction scrolls that
    /// share a target and have no assertion between them are coalesced.
    ///
    /// # Errors
    /// Paused/discarded session, invalid action, or action bound exceeded.
    pub fn push(&mut self, action: RecordedAction) -> Result<(), RoutineError> {
        if self.paused || self.discarded {
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
        if self.discarded {
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
        let recording = match SemanticRecording::from_json(
            std::str::from_utf8(&bytes).map_err(|_| RoutineError::Json("malformed routine JSON".into()))?,
        ) {
            Ok(recording) => recording,
            Err(RoutineError::InvalidRecording) => empty_recording_from_bytes(&bytes)?,
            Err(error) => return Err(error),
        };
        Ok(Self {
            recording,
            paused: false,
            discarded: false,
        })
    }
}

fn empty_recording_from_bytes(bytes: &[u8]) -> Result<SemanticRecording, RoutineError> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        schema_version: u32,
        recording_id: Uuid,
        actions: Vec<serde_json::Value>,
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|_| RoutineError::Json("malformed routine JSON".into()))?;
    if envelope.schema_version != SCHEMA_VERSION || !envelope.actions.is_empty() {
        return Err(RoutineError::InvalidRecording);
    }
    Ok(SemanticRecording {
        schema_version: SCHEMA_VERSION,
        recording_id: envelope.recording_id,
        actions: Vec::new(),
    })
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
    fn opposite_scroll_is_not_coalesced() {
        let mut session = TeachSession::start();
        session.push(scroll("s1", 80.0)).expect("s1");
        session.push(scroll("s2", -40.0)).expect("s2");
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
        assert_eq!(loaded.recording().recording_id, session.recording().recording_id);
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
