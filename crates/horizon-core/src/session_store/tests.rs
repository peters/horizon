use super::{Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision};
use crate::{PanelKind, PanelState, WorkspaceState};

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
fn delete_session_removes_browser_profiles_from_the_saved_profile_root() {
    let root = test_root("delete-browser-profiles");
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let configured_profile_root = root.join("custom-browser-profiles");
    let browser_id = "saved/browser-id";
    let runtime_state = RuntimeState {
        browser: crate::browser::BrowserConfig {
            profile_root: Some(configured_profile_root),
            ..crate::browser::BrowserConfig::default()
        },
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                local_id: browser_id.to_string(),
                name: "Browser".to_string(),
                kind: PanelKind::Browser,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let session = store
        .create_session_from_runtime(runtime_state.clone())
        .expect("create browser session");
    let profile_dir = crate::browser::profile_dir_for_home(&runtime_state.browser, &home, browser_id);
    std::fs::create_dir_all(&profile_dir).expect("create browser profile");
    std::fs::write(profile_dir.join("Preferences"), b"browser state").expect("write browser profile");

    store
        .delete_session(&session.session_id)
        .expect("delete browser session");

    assert!(!profile_dir.exists());
    assert!(!home.session_dir(&session.session_id).exists());
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

#[test]
fn duplicated_sessions_regenerate_browser_artifact_ids() {
    let root = test_root("duplicate-browser-ids");
    let home = HorizonHome::from_root(root);
    let store = SessionStore::new(home.clone(), home.config_path());
    let browser_id = "shared-browser-id".to_string();
    let editor_id = "copied-editor-id".to_string();
    let runtime_state = RuntimeState {
        focused_panel_local_id: Some(browser_id.clone()),
        workspaces: vec![WorkspaceState {
            local_id: "workspace-id".to_string(),
            name: "Workspace".to_string(),
            panels: vec![
                PanelState {
                    local_id: browser_id.clone(),
                    name: "Browser".to_string(),
                    kind: PanelKind::Browser,
                    browser_url: Some("https://example.com".to_string()),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: editor_id.clone(),
                    name: "Editor".to_string(),
                    kind: PanelKind::Editor,
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let source = store
        .create_session_from_runtime(runtime_state)
        .expect("create source session");
    std::fs::write(
        source.transcript_root.join(format!("{editor_id}.bin")),
        b"editor transcript",
    )
    .expect("write source transcript");

    let duplicated = store.duplicate_session(&source.session_id).expect("duplicate session");
    let panels = &duplicated.runtime_state.workspaces[0].panels;
    let duplicated_browser = panels
        .iter()
        .find(|panel| panel.kind == PanelKind::Browser)
        .expect("duplicated browser");
    let duplicated_editor = panels
        .iter()
        .find(|panel| panel.kind == PanelKind::Editor)
        .expect("duplicated editor");

    assert_ne!(duplicated_browser.local_id, browser_id);
    assert_eq!(duplicated_browser.browser_url.as_deref(), Some("https://example.com"));
    assert_eq!(
        duplicated.runtime_state.focused_panel_local_id.as_deref(),
        Some(duplicated_browser.local_id.as_str())
    );
    assert_eq!(duplicated_editor.local_id, editor_id);
    assert_eq!(
        std::fs::read(duplicated.transcript_root.join(format!("{editor_id}.bin"))).expect("read copied transcript"),
        b"editor transcript"
    );
}

fn test_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("horizon-session-store-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create temp root");
    root
}
