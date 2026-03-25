use super::{Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision};

#[test]
fn empty_store_creates_new_session() {
    let root = test_root("empty-store");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());

    let decision = store.prepare_startup(&Config::default()).expect("startup decision");

    match decision {
        StartupDecision::Open {
            disposition: SessionOpenDisposition::New,
            session,
        } => {
            assert!(session.runtime_state_path.exists());
            assert!(session.transcript_root.starts_with(root.join("sessions")));
        }
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn second_startup_resumes_previous_session() {
    let root = test_root("resume-store");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let created = store.create_new_session(&Config::default()).expect("create session");
    store
        .save_runtime_state(&created.session_id, &RuntimeState::from_config(&Config::default()))
        .expect("save state");

    let decision = store.prepare_startup(&Config::default()).expect("startup decision");

    match decision {
        StartupDecision::Open {
            disposition: SessionOpenDisposition::Resume,
            session,
        } => assert_eq!(session.session_id, created.session_id),
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn list_profile_sessions_returns_saved_session_summaries() {
    let root = test_root("list-store");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let created = store.create_new_session(&Config::default()).expect("create session");

    let sessions = store.list_profile_sessions().expect("list sessions");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, created.session_id);
    assert_eq!(sessions[0].label, "Horizon session");
}

#[test]
fn delete_session_removes_saved_state_and_updates_index() {
    let root = test_root("delete-store");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let first = store
        .create_new_session(&Config::default())
        .expect("create first session");
    let second = store
        .create_new_session(&Config::default())
        .expect("create second session");

    store.delete_session(&first.session_id).expect("delete session");

    let sessions = store.list_profile_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, second.session_id);
    assert!(!home.session_dir(&first.session_id).exists());

    let decision = store.prepare_startup(&Config::default()).expect("startup decision");
    match decision {
        StartupDecision::Open { session, .. } => assert_eq!(session.session_id, second.session_id),
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn delete_session_rejects_live_sessions() {
    let root = test_root("delete-live-store");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let created = store.create_new_session(&Config::default()).expect("create session");
    let _lease = store.acquire_lease(&created.session_id).expect("acquire lease");

    let error = store
        .delete_session(&created.session_id)
        .expect_err("live session should not delete");

    assert!(error.to_string().contains("cannot delete live session"));
    assert!(home.session_dir(&created.session_id).exists());
}

fn test_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("horizon-session-store-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create temp root");
    root
}
