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
