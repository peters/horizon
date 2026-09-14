use super::TeachMode;
use horizon_browser::{
    RankedCandidate, TeachFingerprint, TeachFrameContext, TeachFrameLink, TeachObservation, TeachTargetCandidate,
};
use horizon_browser_routines::RoutineError;

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
    let mut teach = TeachMode::start_in(
        temp.path().join("routines"),
        "monthly",
        horizon_browser::BackendKind::ChromiumCdp,
    )
    .expect("start");
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
    let mut teach = TeachMode::start_in(
        temp.path().join("routines"),
        "monthly",
        horizon_browser::BackendKind::ChromiumCdp,
    )
    .expect("start");
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
    let mut teach = TeachMode::start_in(
        temp.path().join("routines"),
        "monthly",
        horizon_browser::BackendKind::ChromiumCdp,
    )
    .expect("start");
    teach.ingest(TeachObservation::Captured(fingerprint()));
    teach.stop();
    teach.set_completion_heading("Report ready");
    assert_eq!(teach.save_reviewed("ignored"), Err(RoutineError::InvalidRecording));
    teach.set_identities_reviewed(true);
    let rows = teach.compile_review("ignored").expect("compile");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, "click");
    assert_eq!(rows[0].mutation, "Mutating");
    let compiled = teach.compile_plan("ignored").expect("plan");
    let definition = teach
        .build_reviewed_definition("ignored", compiled)
        .expect("definition");
    definition.validate().expect("valid");
    let encoded = serde_json::to_vec(&definition).expect("encode");
    let loaded: horizon_browser_routines::RoutineDefinition = serde_json::from_slice(&encoded).expect("decode");
    assert_eq!(loaded.name, "monthly");
    assert_eq!(loaded.plan_version, 1);
    assert_eq!(loaded.steps.len(), 1);
    assert_eq!(loaded.completion_assertions.len(), 1);
}

fn css_fingerprint() -> TeachFingerprint {
    let mut fingerprint = fingerprint();
    fingerprint.selected = None;
    fingerprint.candidates[0].identity = TeachTargetCandidate::CssFallback {
        value: "button.primary".to_string(),
        reviewed: false,
    };
    fingerprint
}

fn duplicate_role_fingerprint() -> TeachFingerprint {
    let mut fingerprint = fingerprint();
    fingerprint.selected = None;
    fingerprint.candidates[0].unique = false;
    fingerprint.candidates[0].match_count = 2;
    fingerprint
}

#[test]
fn unique_css_fallback_can_be_selected_and_non_unique_is_rejected() {
    let temp = tempfile::tempdir().expect("temp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(temp.path()).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(temp.path(), permissions).expect("chmod");
    }
    let mut teach = TeachMode::start_in(
        temp.path().join("routines"),
        "monthly",
        horizon_browser::BackendKind::ChromiumCdp,
    )
    .expect("start");
    teach.ingest(TeachObservation::Captured(css_fingerprint()));
    teach.stop();
    teach.set_completion_heading("Report ready");
    teach.select_step_candidate("a0", 0);
    assert!(teach.last_error().is_none());
    teach.set_identities_reviewed(true);
    teach.compile_plan("ignored").expect("compile after unique css");

    let mut other = TeachMode::start_in(
        temp.path().join("other"),
        "other",
        horizon_browser::BackendKind::ChromiumCdp,
    )
    .expect("start");
    other.ingest(TeachObservation::Captured(duplicate_role_fingerprint()));
    other.stop();
    other.select_step_candidate("a0", 0);
    assert_eq!(other.last_error(), Some("target fingerprint is missing or malformed"));
}
