use std::path::PathBuf;

use horizon_core::{
    Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision, StartupPromptReason,
};
use uuid::Uuid;

fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!("horizon-session-startup-{}", Uuid::new_v4()))
}

fn temp_store() -> (PathBuf, SessionStore) {
    let root = temp_root();
    let home = HorizonHome::from_root(root.join(".horizon"));
    let config_path = root.join("config.yaml");
    let store = SessionStore::new(home, config_path);
    (root, store)
}

#[test]
fn prepare_startup_prompts_when_last_session_is_still_live() {
    let (root, store) = temp_store();

    let session = store
        .create_session_from_runtime(RuntimeState::default())
        .expect("session should be created");
    let _lease = store
        .acquire_lease(&session.session_id)
        .expect("lease should be created");

    let decision = store.prepare_startup(&Config::default()).expect("startup decision");

    match decision {
        StartupDecision::Choose(chooser) => {
            assert_eq!(chooser.reason, StartupPromptReason::LiveConflict);
            assert_eq!(chooser.sessions.len(), 1);
            assert_eq!(chooser.sessions[0].session_id, session.session_id);
            assert!(chooser.sessions[0].is_live);
        }
        other => panic!("expected chooser for live session conflict, got {other:?}"),
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prepare_startup_recovers_when_only_session_has_stale_lease() {
    let (root, store) = temp_store();

    let session = store
        .create_session_from_runtime(RuntimeState::default())
        .expect("session should be created");
    let mut lease = store
        .acquire_lease(&session.session_id)
        .expect("lease should be created");
    lease.last_heartbeat_at -= 60_000;
    let lease_json = serde_json::to_vec_pretty(&lease).expect("serialize lease");
    std::fs::write(store.home().session_lease_path(&session.session_id), lease_json).expect("write stale lease");

    let decision = store.prepare_startup(&Config::default()).expect("startup decision");

    match decision {
        StartupDecision::Open {
            disposition,
            session: reopened,
        } => {
            assert_eq!(disposition, SessionOpenDisposition::Recover);
            assert_eq!(reopened.session_id, session.session_id);
        }
        other => panic!("expected recoverable startup decision, got {other:?}"),
    }

    std::fs::remove_dir_all(root).ok();
}
