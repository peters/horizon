//! Teach-mode controller owned by one browser panel.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use horizon_browser::{TeachFingerprint, TeachObservation};
use horizon_browser_routines::{
    FieldClassification, MutationClass, RecordedAction, RecordedKind, RoutineError, RoutineRegistry, TargetCandidate,
    TargetFingerprint, TeachSession,
};
use uuid::Uuid;

use crate::horizon_home::HorizonHome;

/// In-panel Teach recording: latch, session, and private draft.
pub struct TeachMode {
    session: TeachSession,
    registry: RoutineRegistry,
    routine_id: Uuid,
    name: String,
    next_action: u32,
    last_error: Option<String>,
    completion_heading: String,
    use_title_outcome: bool,
}

impl TeachMode {
    /// Start a named session under `~/.horizon/browser-routines`.
    ///
    /// # Errors
    /// Private registry storage failure.
    pub fn start(name: impl Into<String>) -> Result<Self, RoutineError> {
        let root = HorizonHome::resolve().root().join("browser-routines");
        Self::start_in(root, &name.into())
    }

    fn start_in(root: PathBuf, name: &str) -> Result<Self, RoutineError> {
        let name = bounded_name(name);
        Ok(Self {
            session: TeachSession::start(),
            registry: RoutineRegistry::open(root)?,
            routine_id: Uuid::new_v4(),
            name,
            next_action: 0,
            last_error: None,
            completion_heading: String::new(),
            use_title_outcome: true,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = bounded_name(name);
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.session.is_paused()
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.session.is_stopped()
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    #[must_use]
    pub fn action_previews(&self) -> Vec<String> {
        self.session.recording().actions.iter().map(preview_action).collect()
    }

    #[must_use]
    pub fn use_title_outcome(&self) -> bool {
        self.use_title_outcome
    }

    pub fn set_use_title_outcome(&mut self, selected: bool) {
        self.use_title_outcome = selected;
    }

    #[must_use]
    pub fn completion_heading(&self) -> &str {
        &self.completion_heading
    }

    pub fn set_completion_heading(&mut self, heading: &str) {
        self.completion_heading = heading.chars().filter(|ch| !ch.is_control()).take(256).collect();
    }

    pub fn pause(&mut self) {
        self.session.pause();
        self.persist_draft();
    }

    pub fn resume(&mut self) {
        self.session.resume();
    }

    pub fn stop(&mut self) {
        self.session.stop();
        self.persist_draft();
    }

    /// Discard memory and the private draft.
    ///
    /// # Errors
    /// Draft unlink failure.
    pub fn discard(&mut self) -> Result<(), RoutineError> {
        self.session.discard_saved(&self.registry, self.routine_id)
    }

    pub fn ingest(&mut self, observation: TeachObservation, page_url: Option<&str>) {
        match observation {
            TeachObservation::Failed { message, .. } => {
                self.last_error = Some(message);
            }
            TeachObservation::Captured(fingerprint) => match recorded_click(self.next_action, &fingerprint, page_url) {
                Ok(action) => match self.session.push(action) {
                    Ok(()) => {
                        self.next_action += 1;
                        self.last_error = None;
                        self.persist_draft();
                    }
                    Err(error) => self.last_error = Some(error.to_string()),
                },
                Err(error) => self.last_error = Some(error.to_string()),
            },
        }
    }

    fn persist_draft(&mut self) {
        if let Err(error) = self.session.save_draft(&self.registry, self.routine_id) {
            self.last_error = Some(error.to_string());
        }
    }
}

fn bounded_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.chars().filter(|ch| !ch.is_control()).take(64).collect()
    }
}

fn preview_action(action: &RecordedAction) -> String {
    let label = action
        .target
        .as_ref()
        .and_then(candidate_label)
        .unwrap_or_else(|| "page".to_string());
    match &action.kind {
        RecordedKind::Click { count } if *count > 1 => format!("click ×{count} {label}"),
        RecordedKind::Click { .. } => format!("click {label}"),
        RecordedKind::Fill => format!("fill {label}"),
        RecordedKind::Scroll { .. } => "scroll".to_string(),
        RecordedKind::Navigate => "navigate".to_string(),
        RecordedKind::Wait { .. } => "wait".to_string(),
        RecordedKind::Reload => "reload".to_string(),
        RecordedKind::Back => "back".to_string(),
        RecordedKind::Forward => "forward".to_string(),
        RecordedKind::Handoff { .. } => "handoff".to_string(),
    }
}

fn candidate_label(target: &TargetFingerprint) -> Option<String> {
    let selected = target
        .selected
        .and_then(|index| target.candidates.get(index as usize))?;
    match &selected.identity {
        TargetCandidate::RoleName { name, .. } | TargetCandidate::UniqueId { value: name, .. } => Some(name.clone()),
        TargetCandidate::LabelControl { label, .. } => Some(label.clone()),
        TargetCandidate::TestId { value, .. } | TargetCandidate::CssFallback { value, .. } => Some(value.clone()),
        TargetCandidate::VisibleText { text, .. } => Some(text.clone()),
    }
}

fn recorded_click(
    index: u32,
    fingerprint: &TeachFingerprint,
    page_url: Option<&str>,
) -> Result<RecordedAction, RoutineError> {
    let target = convert_fingerprint(fingerprint)?;
    let page_origin = target.frame.origin.clone();
    let url_pattern = match page_url {
        Some(url) if !url.contains(['?', '#', '@']) && !url.is_empty() => url.to_string(),
        _ => page_origin.to_string(),
    };
    Ok(RecordedAction {
        action_id: format!("a{index}"),
        recorded_at_millis: now_millis(),
        kind: RecordedKind::Click { count: 1 },
        target: Some(target),
        page_origin,
        url_pattern,
        navigation: None,
        value_source: None,
        field_classification: FieldClassification::Ordinary,
        mutation_class: MutationClass::Mutating,
        precondition: None,
        postcondition: None,
    })
}

fn convert_fingerprint(fingerprint: &TeachFingerprint) -> Result<TargetFingerprint, RoutineError> {
    let encoded = serde_json::to_vec(fingerprint).map_err(|_| RoutineError::InvalidFingerprint)?;
    serde_json::from_slice(&encoded).map_err(|_| RoutineError::InvalidFingerprint)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::TeachMode;
    use horizon_browser::{
        RankedCandidate, TeachFingerprint, TeachFrameContext, TeachFrameLink, TeachObservation, TeachTargetCandidate,
    };

    fn fingerprint() -> TeachFingerprint {
        TeachFingerprint {
            candidates: vec![RankedCandidate {
                identity: TeachTargetCandidate::RoleName {
                    role: "button".to_string(),
                    name: "Generate report".to_string(),
                    reviewed: false,
                },
                match_count: 1,
                unique: true,
            }],
            selected: Some(0),
            frame: TeachFrameContext {
                top_level: true,
                origin: "https://reports.example".to_string(),
                chain: Vec::<TeachFrameLink>::new(),
            },
            digest: "el-1".to_string(),
        }
    }

    #[test]
    fn captured_clicks_are_previewed_and_failed_observations_do_not_record() {
        let temp = tempfile::tempdir().expect("temp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(temp.path()).expect("meta").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(temp.path(), permissions).expect("chmod");
        }
        let mut teach = TeachMode::start_in(temp.path().join("routines"), "monthly").expect("start");
        teach.ingest(
            TeachObservation::Captured(fingerprint()),
            Some("https://reports.example/app"),
        );
        assert_eq!(teach.action_previews(), vec!["click Generate report".to_string()]);
        teach.ingest(
            TeachObservation::Failed {
                code: "cross_origin_frame".to_string(),
                message: "frame is cross-origin".to_string(),
            },
            Some("https://reports.example/app"),
        );
        assert_eq!(teach.action_previews().len(), 1);
        assert_eq!(teach.last_error(), Some("frame is cross-origin"));
        teach.stop();
        assert!(teach.is_stopped());
        teach.resume();
        assert!(teach.is_stopped());
    }
}
