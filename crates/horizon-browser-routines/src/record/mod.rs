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

    pub fn recording_mut(&mut self) -> &mut SemanticRecording {
        &mut self.recording
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
        action.validate_payload()?;
        if let Some(previous) = self.recording.actions.last_mut()
            && can_coalesce(previous, &action)
        {
            let previous_kind = previous.kind.clone();
            let previous_postcondition = previous.postcondition.clone();
            coalesce_scroll(previous, action)?;
            return match self.recording.validate() {
                Ok(()) | Err(RoutineError::UndurableTarget) => Ok(()),
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
            Ok(()) | Err(RoutineError::UndurableTarget) => Ok(()),
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
        for action in &self.recording.actions {
            action.validate_payload()?;
        }
        let encoded =
            if self.recording.actions.is_empty() || self.recording.validate() == Err(RoutineError::UndurableTarget) {
                serde_json::to_vec_pretty(&serde_json::json!({
                    "schema_version": SCHEMA_VERSION,
                    "recording_id": self.recording.recording_id,
                    "actions": self.recording.actions,
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
        } else if let Err(error) = recording.validate()
            && error != RoutineError::UndurableTarget
        {
            return Err(error);
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
mod tests;
