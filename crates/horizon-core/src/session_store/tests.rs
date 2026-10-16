use super::{Config, HorizonHome, RuntimeState, SessionOpenDisposition, SessionStore, StartupDecision};
use crate::{BrowserProfileState, PanelKind, PanelState, WorkspaceState};

#[test]
fn empty_store_creates_new_session() {
    let root_dir = test_root("empty-store");
    let root = root_dir.path().to_path_buf();
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
    let root_dir = test_root("resume-store");
    let root = root_dir.path().to_path_buf();
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
    let root_dir = test_root("list-store");
    let root = root_dir.path().to_path_buf();
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
    let root_dir = test_root("delete-store");
    let root = root_dir.path().to_path_buf();
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
    let root_dir = test_root("delete-browser-profiles");
    let root = root_dir.path().to_path_buf();
    let home = HorizonHome::from_root(root.clone());
    let store = SessionStore::new(home.clone(), home.config_path());
    let launched_profile_root = root.join("launched-browser-profiles");
    let current_profile_root = root.join("current-browser-profiles");
    let browser_id = "saved/browser-id";
    let runtime_state = RuntimeState {
        browser: crate::browser::BrowserConfig {
            profile_root: Some(current_profile_root),
            ..crate::browser::BrowserConfig::default()
        },
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                local_id: browser_id.to_string(),
                name: "Browser".to_string(),
                kind: PanelKind::Browser,
                browser_profile: Some(BrowserProfileState {
                    root: Some(launched_profile_root.clone()),
                    backend: None,
                    hidden: false,
                    remote_target: None,
                    session_id: None,
                }),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let session = store
        .create_session_from_runtime(runtime_state.clone())
        .expect("create browser session");
    let launched_config = crate::browser::BrowserConfig {
        profile_root: Some(launched_profile_root),
        ..crate::browser::BrowserConfig::default()
    };
    let profile_dir = crate::browser::profile_dir_for_home(&launched_config, &home, browser_id);
    std::fs::create_dir_all(&profile_dir).expect("create browser profile");
    std::fs::write(profile_dir.join("Preferences"), b"browser state").expect("write browser profile");

    store
        .delete_session(&session.session_id)
        .expect("delete browser session");

    assert!(!profile_dir.exists());
    assert!(!home.session_dir(&session.session_id).exists());
}

#[test]
#[cfg(unix)]
fn a_delete_that_fails_keeps_the_newest_board_that_is_not_written() {
    let root_dir = test_root("failed-delete");
    let home = HorizonHome::from_root(root_dir.path().to_path_buf());
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store
        .create_session_from_runtime(RuntimeState::default())
        .expect("create session");
    let id = session.session_id;
    // A folder where the board goes refuses every save, so the newest board stays queued.
    std::fs::remove_file(&session.runtime_state_path).expect("remove board");
    std::fs::create_dir(&session.runtime_state_path).expect("block board");
    let board = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                local_id: "browser".into(),
                kind: PanelKind::Browser,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let config = board.workspaces[0].panels[0].browser_config_for_restore(&board.browser);
    let profile = crate::browser::profile_dir_for_home(&config, &home, "browser");
    // A file where the profile of the browser goes fails the delete.
    std::fs::create_dir_all(profile.parent().unwrap()).unwrap();
    std::fs::write(&profile, b"not a folder").unwrap();
    let generation = store.queue_runtime_state(&id, board);
    let wait = std::time::Duration::from_secs(30);
    assert!(matches!(
        store.wait_runtime_state_durable(&id, generation, wait),
        Err(Some(_))
    ));
    // The writer may try the board again just now; then the delete is refused.
    let delete = || loop {
        match store.delete_session(&id) {
            Err(error) if error.to_string().contains("still saving") => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            deleted => return deleted,
        }
    };
    assert!(delete().is_err(), "the profile is not a folder");
    let kept = store.writer.unwritten(&id).expect("the board is still queued");
    assert_eq!(kept.workspaces[0].panels[0].local_id, "browser");
    assert!(home.session_dir(&id).exists());
    std::fs::remove_file(&profile).unwrap();
    delete().expect("delete session");
    assert!(store.writer.unwritten(&id).is_none());
    assert!(!home.session_dir(&id).exists());
}

#[test]
fn delete_session_rejects_live_sessions() {
    let root_dir = test_root("delete-live-store");
    let root = root_dir.path().to_path_buf();
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
    let root_dir = test_root("duplicate-browser-ids");
    let root = root_dir.path().to_path_buf();
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

/// The returned guard removes the directory when the test ends; bind it for
/// the whole test rather than letting it drop at the end of the statement.
fn test_root(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("horizon-session-store-{label}-"))
        .tempdir()
        .expect("create temp root")
}

#[test]
#[cfg(unix)]
fn a_queued_save_is_durable_on_disk_even_after_its_file_was_removed() {
    let root_dir = test_root("durable-queued-save");
    let home = HorizonHome::from_root(root_dir.path().into());
    let store = SessionStore::new(home.clone(), home.config_path());
    let created = store.create_new_session(&Config::default()).unwrap();
    std::fs::remove_file(&created.runtime_state_path).unwrap();
    let generation = store.queue_runtime_state(&created.session_id, RuntimeState::default());
    assert_eq!(
        store.wait_runtime_state_durable(&created.session_id, generation, std::time::Duration::from_secs(30)),
        Ok(())
    );
    assert!(RuntimeState::load(&created.runtime_state_path).unwrap().is_some());
}

#[test]
fn session_create_copy_and_delete_never_wait_for_the_index_lock() {
    use std::time::{Duration, Instant};
    let root_dir = test_root("index-lock");
    let home = HorizonHome::from_root(root_dir.path().into());
    let store = SessionStore::new(home.clone(), home.config_path());
    let first = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    let doomed = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(store.flush_runtime_states(Duration::from_secs(30)));
    // Another Horizon process holds the index lock, so the writer cannot change the index.
    let held = std::fs::File::create(home.sessions_dir().join("index.lock")).unwrap();
    held.lock().unwrap();
    // Only an index change waits for the index locks. The calls must start none here,
    // however long their own session files take to write on a slow disk.
    let started = || super::index_file::CHANGES_STARTED.with(std::cell::Cell::get);
    let before = started();
    let second = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    let copy = store.duplicate_session(&first.session_id).unwrap();
    store.delete_session(&doomed.session_id).unwrap();
    let during = started() - before;
    let listed = store.list_profile_sessions().unwrap();
    // The exit wait for the writer keeps its bound while the change cannot be written.
    let started = Instant::now();
    let flushed = store.flush_runtime_states(Duration::from_millis(200));
    let exit_wait = started.elapsed();
    drop(held);
    assert_eq!(during, 0, "create, copy and delete waited for the index lock");
    assert_eq!(listed[0].session_id, copy.session_id, "a read sees the queued changes");
    assert!(
        !flushed && exit_wait < Duration::from_secs(2),
        "the exit wait took {exit_wait:?}"
    );
    assert!(
        store.flush_runtime_states(Duration::from_secs(30)),
        "the exit wait writes them"
    );
    let index = super::IndexFile::at(&home.session_index_path()).load().unwrap();
    let profile = index.profile(store.profile_id()).unwrap();
    assert_eq!(profile.last_session_id.as_deref(), Some(copy.session_id.as_str()));
    assert!(index.names(store.profile_id(), &second.session_id));
    assert!(index.names(store.profile_id(), &first.session_id));
    assert!(!index.names(store.profile_id(), &doomed.session_id));
}

#[test]
fn an_index_change_that_fails_is_tried_again() {
    let root_dir = test_root("index-retry");
    let home = HorizonHome::from_root(root_dir.path().into());
    let store = SessionStore::new(home.clone(), home.config_path());
    store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
    // A damaged index refuses each change until it is repaired.
    let saved = std::fs::read(home.session_index_path()).unwrap();
    std::fs::write(home.session_index_path(), "\0").unwrap();
    let second = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(
        !store.flush_runtime_states(std::time::Duration::from_secs(30)),
        "the change failed and is still queued"
    );
    std::fs::write(home.session_index_path(), saved).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let index_path = home.session_index_path();
    let index = super::IndexFile::at(&index_path);
    while !index.load().unwrap().names(store.profile_id(), &second.session_id) {
        assert!(
            std::time::Instant::now() < deadline,
            "the change is written once it can be"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
}

#[test]
fn a_board_save_never_overtakes_an_index_change_queued_before_it() {
    let root_dir = test_root("index-order");
    let home = HorizonHome::from_root(root_dir.path().into());
    let store = SessionStore::new(home.clone(), home.config_path());
    let later = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
    // The touch of `earlier` fails and waits for its retry.
    let saved = std::fs::read(home.session_index_path()).unwrap();
    std::fs::write(home.session_index_path(), "\0").unwrap();
    let earlier = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(!store.flush_runtime_states(std::time::Duration::from_secs(30)));
    std::fs::write(home.session_index_path(), saved).unwrap();
    // `later` is used after `earlier`, so it stays the last session.
    let _ = store.queue_runtime_state(&later.session_id, RuntimeState::default());
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
    let index_path = home.session_index_path();
    let index = super::IndexFile::at(&index_path).load().unwrap();
    let profile = index.profile(store.profile_id()).unwrap();
    assert_eq!(profile.last_session_id.as_deref(), Some(later.session_id.as_str()));
    assert!(index.names(store.profile_id(), &earlier.session_id));
}

#[test]
fn boards_and_index_changes_are_written_in_queue_order() {
    let root_dir = test_root("index-queue-order");
    let home = HorizonHome::from_root(root_dir.path().into());
    let store = SessionStore::new(home.clone(), home.config_path());
    let board = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    let touched = store.create_session_from_runtime(RuntimeState::default()).unwrap();
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
    // Another Horizon process holds the index lock, so the writer waits in the next change
    // while a board and then an index change are queued behind it.
    let held = std::fs::File::create(home.sessions_dir().join("index.lock")).unwrap();
    held.lock().unwrap();
    store.create_session_from_runtime(RuntimeState::default()).unwrap();
    let _ = store.queue_runtime_state(&board.session_id, RuntimeState::default());
    store
        .save_runtime_state(&touched.session_id, &RuntimeState::default())
        .unwrap();
    drop(held);
    assert!(store.flush_runtime_states(std::time::Duration::from_secs(30)));
    let index_path = home.session_index_path();
    let index = super::IndexFile::at(&index_path).load().unwrap();
    let profile = index.profile(store.profile_id()).unwrap();
    assert_eq!(profile.last_session_id.as_deref(), Some(touched.session_id.as_str()));
}
