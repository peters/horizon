//! Teach-mode controller owned by one browser panel.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use horizon_browser::BackendKind;
use horizon_browser::{TeachFingerprint, TeachObservation};
use horizon_browser_routines::{
    Assertion, CompiledAction, CompiledStep, CredentialMode, CredentialPolicy, FieldClassification, MutationClass,
    RecordedAction, RecordedKind, RoutineDefinition, RoutineError, RoutineRegistry, RoutineStep, SCHEMA_VERSION,
    SemanticRecording, TargetCandidate, TargetFingerprint, TeachSession, compile,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::horizon_home::HorizonHome;

/// In-panel Teach recording: latch, session, and private draft.
pub struct TeachMode {
    session: TeachSession,
    registry: RoutineRegistry,
    root: PathBuf,
    routine_id: Uuid,
    name: String,
    next_action: u32,
    last_error: Option<String>,
    completion_heading: String,
    use_title_outcome: bool,
    identities_reviewed: bool,
}

/// One compiled step shown in the Teach reviewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewRow {
    pub summary: String,
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
            registry: RoutineRegistry::open(root.clone())?,
            root,
            routine_id: Uuid::new_v4(),
            name,
            next_action: 0,
            last_error: None,
            completion_heading: String::new(),
            use_title_outcome: true,
            identities_reviewed: false,
        })
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = bounded_name(name);
        self.persist_draft();
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
        self.persist_draft();
    }

    #[must_use]
    pub fn completion_heading(&self) -> &str {
        &self.completion_heading
    }

    pub fn set_completion_heading(&mut self, heading: &str) {
        self.completion_heading = heading.chars().filter(|ch| !ch.is_control()).take(256).collect();
        self.persist_draft();
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
        match self.session.discard_saved(&self.registry, self.routine_id) {
            Ok(()) => {
                let _ = std::fs::remove_file(self.root.join(self.routine_id.to_string()).join("ui.json"));
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    #[must_use]
    pub fn identities_reviewed(&self) -> bool {
        self.identities_reviewed
    }

    pub fn set_identities_reviewed(&mut self, reviewed: bool) {
        self.identities_reviewed = reviewed;
        self.persist_draft();
    }

    /// Compile the stopped recording into reviewer rows.
    ///
    /// # Errors
    /// Missing outcome, empty recording, or compiler validation failure.
    pub fn compile_review(&self, page_title: &str) -> Result<Vec<ReviewRow>, RoutineError> {
        Ok(self.compile_plan(page_title)?.steps.iter().map(review_row).collect())
    }

    /// Save a reviewed routine. Requires an explicit review.
    ///
    /// # Errors
    /// Compiler or registry validation failure.
    pub fn save_reviewed(&mut self, backend: BackendKind, page_title: &str) -> Result<(), RoutineError> {
        if !self.session.is_stopped() {
            return Err(RoutineError::TeachInactive);
        }
        let mut compiled = match self.compile_plan(page_title) {
            Ok(compiled) => compiled,
            Err(error) => {
                self.last_error = Some(error.to_string());
                return Err(error);
            }
        };
        if self.identities_reviewed {
            for step in &mut compiled.steps {
                if let Some(target) = &mut step.target {
                    mark_fingerprint_reviewed(target);
                }
            }
        }
        let definition = match self.reviewed_definition(backend, page_title, compiled) {
            Ok(definition) => definition,
            Err(error) => {
                self.last_error = Some(error.to_string());
                return Err(error);
            }
        };
        if let Err(error) = definition.validate().and_then(|()| self.registry.save(&definition)) {
            self.last_error = Some(error.to_string());
            return Err(error);
        }
        self.last_error = None;
        Ok(())
    }

    fn reviewed_definition(
        &self,
        backend: BackendKind,
        page_title: &str,
        compiled: horizon_browser_routines::CompiledRoutine,
    ) -> Result<RoutineDefinition, RoutineError> {
        let now = rfc3339_now()?;
        Ok(RoutineDefinition {
            schema_version: SCHEMA_VERSION,
            routine_id: self.routine_id,
            name: self.name.clone(),
            backend_requirement: backend,
            profile_id: self.routine_id,
            allowed_origins: unique_origins(self.session.recording()),
            credential_policy: CredentialPolicy {
                mode: CredentialMode::None,
                slot: None,
                allowed_origins: Vec::new(),
            },
            variables: Vec::new(),
            steps: compiled
                .steps
                .into_iter()
                .map(|step| RoutineStep {
                    step_id: step.step_id,
                    target_fingerprint: step.target,
                    action: step.action,
                    value_source: step.value_source,
                    mutation_class: step.mutation_class,
                    resume_policy: step.resume_policy,
                    precondition: step.precondition,
                    postcondition: step.postcondition,
                })
                .collect(),
            completion_assertions: self.completion_assertions(page_title)?,
            plan_version: 1,
            verified_plan_version: None,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    fn compile_plan(&self, page_title: &str) -> Result<horizon_browser_routines::CompiledRoutine, RoutineError> {
        compile(self.session.recording(), self.completion_assertions(page_title)?)
    }

    fn completion_assertions(&self, page_title: &str) -> Result<Vec<Assertion>, RoutineError> {
        let mut assertions = Vec::new();
        if self.use_title_outcome {
            let title = page_title.trim();
            if !title.is_empty() {
                assertions.push(Assertion::Heading {
                    value: title.chars().filter(|ch| !ch.is_control()).take(256).collect(),
                });
            }
        }
        let heading = self.completion_heading.trim();
        if !heading.is_empty() {
            assertions.push(Assertion::Heading {
                value: heading.to_string(),
            });
        }
        if assertions.is_empty() {
            return Err(RoutineError::InvalidAssertion);
        }
        Ok(assertions)
    }

    pub fn ingest(&mut self, observation: TeachObservation) {
        match observation {
            TeachObservation::Failed { message, .. } => {
                self.last_error = Some(message);
            }
            TeachObservation::Captured(fingerprint) => {
                if fingerprint_is_focused_text(&fingerprint) {
                    return;
                }
                match recorded_click(self.next_action, &fingerprint) {
                    Ok(action) => match self.session.push(action) {
                        Ok(()) => {
                            self.next_action += 1;
                            self.last_error = None;
                            self.persist_draft();
                        }
                        Err(error) => self.last_error = Some(error.to_string()),
                    },
                    Err(error) => self.last_error = Some(error.to_string()),
                }
            }
        }
    }

    fn persist_draft(&mut self) {
        if let Err(error) = self.session.save_draft(&self.registry, self.routine_id) {
            self.last_error = Some(error.to_string());
            return;
        }
        if let Err(error) = self.persist_ui_state() {
            self.last_error = Some(error.to_string());
        }
    }

    fn persist_ui_state(&self) -> Result<(), RoutineError> {
        let _lock = self.registry.lock(self.routine_id)?;
        let path = self.root.join(self.routine_id.to_string()).join("ui.json");
        let encoded = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "name": self.name,
            "use_title_outcome": self.use_title_outcome,
            "completion_heading": self.completion_heading,
            "identities_reviewed": self.identities_reviewed,
        }))
        .map_err(|_| RoutineError::Json("malformed routine JSON".into()))?;
        std::fs::write(&path, encoded).map_err(|_| RoutineError::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&path)
                .map_err(|_| RoutineError::Storage)?
                .permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&path, permissions).map_err(|_| RoutineError::Storage)?;
        }
        Ok(())
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

fn fingerprint_is_focused_text(fingerprint: &TeachFingerprint) -> bool {
    fingerprint
        .candidates
        .iter()
        .any(|candidate| match &candidate.identity {
            horizon_browser::TeachTargetCandidate::RoleName { role, .. } => {
                matches!(
                    role.to_ascii_lowercase().as_str(),
                    "textbox" | "searchbox" | "combobox" | "spinbutton"
                )
            }
            horizon_browser::TeachTargetCandidate::LabelControl { control, .. } => {
                let control = control.to_ascii_lowercase();
                control.contains("input") || control.contains("textarea") || control.contains("textbox")
            }
            _ => false,
        })
}

fn recorded_click(index: u32, fingerprint: &TeachFingerprint) -> Result<RecordedAction, RoutineError> {
    let target = convert_fingerprint(fingerprint)?;
    let page_origin = target.frame.origin.clone();
    let url_pattern = page_origin.to_string();
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

fn review_row(step: &CompiledStep) -> ReviewRow {
    let action = match &step.action {
        CompiledAction::Click { count } if *count > 1 => format!("click ×{count}"),
        CompiledAction::Click { .. } => "click".to_string(),
        CompiledAction::Fill => "fill".to_string(),
        CompiledAction::CredentialFill => "credential fill".to_string(),
        CompiledAction::Scroll { .. } => "scroll".to_string(),
        CompiledAction::Navigate { .. } => "navigate".to_string(),
        CompiledAction::Wait { .. } => "wait".to_string(),
        CompiledAction::Reload => "reload".to_string(),
        CompiledAction::Back => "back".to_string(),
        CompiledAction::Forward => "forward".to_string(),
        CompiledAction::Handoff { .. } => "handoff".to_string(),
    };
    let target = step
        .target
        .as_ref()
        .and_then(candidate_label)
        .unwrap_or_else(|| "page".to_string());
    let mcp = step.mcp.as_ref().map_or("no MCP", |call| call.tool.as_str());
    let mutation = format!("{:?}", step.mutation_class);
    let resume = format!("{:?}", step.resume_policy);
    ReviewRow {
        summary: format!("{action} {target} · {mutation} · {resume} · {mcp}"),
    }
}

fn mark_fingerprint_reviewed(target: &mut TargetFingerprint) {
    for candidate in &mut target.candidates {
        match &mut candidate.identity {
            TargetCandidate::RoleName { reviewed, .. }
            | TargetCandidate::LabelControl { reviewed, .. }
            | TargetCandidate::TestId { reviewed, .. }
            | TargetCandidate::UniqueId { reviewed, .. }
            | TargetCandidate::VisibleText { reviewed, .. }
            | TargetCandidate::CssFallback { reviewed, .. } => *reviewed = true,
        }
    }
}

fn unique_origins(recording: &SemanticRecording) -> Vec<horizon_browser_routines::Origin> {
    let mut origins = Vec::new();
    for action in &recording.actions {
        if !origins.contains(&action.page_origin) {
            origins.push(action.page_origin.clone());
        }
    }
    origins
}

fn rfc3339_now() -> Result<String, RoutineError> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| RoutineError::InvalidRecording)
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
        teach.ingest(TeachObservation::Captured(fingerprint()));
        assert_eq!(teach.action_previews(), vec!["click Generate report".to_string()]);
        teach.ingest(TeachObservation::Failed {
            code: "cross_origin_frame".to_string(),
            message: "frame is cross-origin".to_string(),
        });
        assert_eq!(teach.action_previews().len(), 1);
        assert_eq!(teach.last_error(), Some("frame is cross-origin"));
        teach.stop();
        assert!(teach.is_stopped());
        teach.resume();
        assert!(teach.is_stopped());
    }

    #[test]
    fn focused_text_fingerprints_are_not_recorded_as_clicks() {
        let temp = tempfile::tempdir().expect("temp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(temp.path()).expect("meta").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(temp.path(), permissions).expect("chmod");
        }
        let mut teach = TeachMode::start_in(temp.path().join("routines"), "monthly").expect("start");
        let mut focused = fingerprint();
        focused.candidates[0].identity = TeachTargetCandidate::RoleName {
            role: "textbox".to_string(),
            name: "Email".to_string(),
            reviewed: false,
        };
        teach.ingest(TeachObservation::Captured(focused));
        assert!(teach.action_previews().is_empty());
    }

    #[test]
    fn stopped_session_compiles_and_saves_after_review() {
        let temp = tempfile::tempdir().expect("temp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(temp.path()).expect("meta").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(temp.path(), permissions).expect("chmod");
        }
        let mut teach = TeachMode::start_in(temp.path().join("routines"), "monthly").expect("start");
        teach.ingest(TeachObservation::Captured(fingerprint()));
        teach.stop();
        teach.set_identities_reviewed(true);
        let rows = teach.compile_review("Report ready").expect("compile");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].summary.contains("click"));
        assert!(rows[0].summary.contains("Mutating"));
    }
}
