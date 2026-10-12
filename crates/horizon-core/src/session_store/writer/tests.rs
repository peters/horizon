use super::*;
use crate::HorizonHome;

fn store() -> (tempfile::TempDir, SessionStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(
        HorizonHome::from_root(temp.path().join("home")),
        temp.path().join("config.yaml"),
    );
    (temp, store)
}

fn board(active: &str) -> RuntimeState {
    RuntimeState {
        active_workspace_local_id: Some(active.into()),
        ..RuntimeState::default()
    }
}

fn on_disk(store: &SessionStore) -> Option<String> {
    let text = std::fs::read_to_string(store.home().session_runtime_path("session")).ok()?;
    serde_yaml::from_str::<RuntimeState>(&text)
        .unwrap()
        .active_workspace_local_id
}

#[test]
#[cfg(unix)]
fn a_queued_board_is_read_back_before_it_is_written_and_then_is_durable_on_disk() {
    let (_temp, store) = store();
    store.save_runtime_state("session", &board("first")).unwrap();
    let generation = store.queue_runtime_state("session", board("second"));
    // Whatever the worker did yet, a reader never gets the older board.
    let read = store.load_runtime("session").unwrap().unwrap();
    assert_eq!(read.active_workspace_local_id.as_deref(), Some("second"));
    assert_eq!(
        store.wait_runtime_state_durable("session", generation, Duration::from_secs(30)),
        Ok(())
    );
    assert_eq!(on_disk(&store).as_deref(), Some("second"));
    assert!(store.writer.unwritten("session").is_none(), "nothing is left queued");
}

#[test]
#[cfg(unix)]
fn a_newer_board_replaces_one_that_is_not_written_yet() {
    let (_temp, store) = store();
    let boards: Vec<u64> = (0..20)
        .map(|step| store.queue_runtime_state("session", board(&format!("step-{step}"))))
        .collect();
    let newest = *boards.last().unwrap();
    assert_eq!(store.queued_runtime_state("session"), Some(newest));
    assert!(store.flush_runtime_states(Duration::from_secs(30)));
    assert_eq!(
        on_disk(&store).as_deref(),
        Some("step-19"),
        "the newest board is on disk"
    );
    // An older generation counts as written once a newer board of the session is.
    assert_eq!(
        store.wait_runtime_state_durable("session", boards[0], Duration::from_secs(1)),
        Ok(())
    );
}

#[test]
fn a_session_switch_reads_the_board_the_writer_still_holds() {
    let (_temp, store) = store();
    store.save_runtime_state("session", &board("old")).unwrap();
    let _ = store.queue_runtime_state("session", board("new"));
    let resumed = store.resume_session("session").unwrap();
    assert_eq!(resumed.runtime_state.active_workspace_local_id.as_deref(), Some("new"));
    assert!(store.flush_runtime_states(Duration::from_secs(30)));
}

#[test]
#[cfg(unix)]
fn a_session_that_cannot_be_saved_does_not_hold_up_the_others() {
    let (_temp, store) = store();
    store.save_runtime_state("broken", &board("old")).unwrap();
    // A folder where the board goes refuses every save of this session.
    let runtime = store.home().session_runtime_path("broken");
    std::fs::remove_file(&runtime).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let broken = store.queue_runtime_state("broken", board("never"));
    assert!(matches!(
        store.wait_runtime_state_durable("broken", broken, Duration::from_secs(30)),
        Err(Some(_))
    ));
    let healthy = store.queue_runtime_state("session", board("saved"));
    assert_eq!(
        store.wait_runtime_state_durable("session", healthy, Duration::from_secs(30)),
        Ok(())
    );
    assert_eq!(on_disk(&store).as_deref(), Some("saved"));
    assert!(
        !store.flush_runtime_states(Duration::from_secs(30)),
        "the exit does not wait for a session that keeps failing"
    );
    assert!(!store.runtime_states_pending(), "a failed board is not pending");
    // The writer may try the broken board again just now.
    while store.writer.reserve("broken").map(|direct| direct.forget()).is_err() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(store.flush_runtime_states(Duration::ZERO));
}

#[test]
fn a_save_outside_the_queue_replaces_the_queued_board() {
    let (_temp, store) = store();
    let _ = store.queue_runtime_state("session", board("queued"));
    // The writer may write the queued board just now.
    while store.save_runtime_state("session", &board("direct")).is_err() {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(store.flush_runtime_states(Duration::from_secs(30)));
    assert_eq!(on_disk(&store).as_deref(), Some("direct"));
    let read = store.load_runtime("session").unwrap().unwrap();
    assert_eq!(read.active_workspace_local_id.as_deref(), Some("direct"));
}

#[test]
fn a_save_outside_the_queue_is_refused_while_park_records_of_the_session_are_written() {
    let (_temp, store) = store();
    store.writer.lock().writing_parks = Some("session".into());
    assert!(store.save_runtime_state("session", &board("direct")).is_err());
    assert!(store.save_runtime_state("other", &board("direct")).is_ok());
    store.writer.lock().writing_parks = None;
    assert!(store.save_runtime_state("session", &board("direct")).is_ok());
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn the_writer_leaves_a_session_alone_while_a_save_outside_the_queue_writes_it() {
    let (_temp, store) = store();
    let direct = store.writer.supersede("session", &board("direct")).unwrap();
    assert!(
        store.writer.supersede("session", &board("again")).is_err(),
        "one direct save at a time"
    );
    assert!(
        store.writer.reserve("session").is_err(),
        "a delete waits for the direct save"
    );
    let generation = store.queue_runtime_state("session", board("queued"));
    assert_eq!(
        store.wait_runtime_state_durable("session", generation, Duration::from_millis(200)),
        Err(None),
        "the queued board waits"
    );
    drop(direct);
    assert_eq!(
        store.wait_runtime_state_durable("session", generation, Duration::from_secs(30)),
        Ok(())
    );
    assert_eq!(on_disk(&store).as_deref(), Some("queued"));
}

#[cfg(all(unix, feature = "cloud-workspaces"))]
fn park(parked: bool, line: Option<&str>) -> Park {
    Park {
        local_id: "panel".into(),
        parked,
        status: line.map(|line| crate::cloud_runtime::session_status::SessionStatus {
            id: "agent".into(),
            activity: crate::cloud_runtime::session_status::SessionActivity::Idle,
            quiet_for: None,
            lines: vec![line.into()],
        }),
    }
}

#[test]
#[cfg(all(unix, feature = "cloud-workspaces"))]
fn park_records_of_a_session_that_cannot_be_saved_do_not_pile_up() {
    let (_temp, store) = store();
    store.save_runtime_state("broken", &board("old")).unwrap();
    let runtime = store.home().session_runtime_path("broken");
    std::fs::remove_file(&runtime).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    let broken = store.queue_runtime_state("broken", board("never"));
    assert!(matches!(
        store.wait_runtime_state_durable("broken", broken, Duration::from_secs(30)),
        Err(Some(_))
    ));
    // The failed board stays queued, so its park records wait behind it.
    store
        .writer
        .queue_parks(&store, "broken", vec![park(true, Some("first"))]);
    for _ in 0..50 {
        store.writer.queue_parks(&store, "broken", vec![park(false, None)]);
    }
    let state = store.writer.lock();
    let parks = &state.parks["broken"];
    assert_eq!(parks.len(), 1, "one record per panel waits");
    assert!(!parks[0].parked, "the newest park state waits");
    assert_eq!(
        parks[0].status.as_ref().and_then(|status| status.last_line()),
        Some("first"),
        "a record without a status keeps the queued status"
    );
    drop(state);
    let mut with_panel = board("again");
    with_panel.workspaces.push(crate::WorkspaceState {
        panels: vec![crate::PanelState {
            local_id: "panel".into(),
            ..crate::PanelState::default()
        }],
        ..crate::WorkspaceState::default()
    });
    let _ = store.queue_runtime_state("broken", with_panel);
    assert!(
        store.writer.lock().parks.contains_key("broken"),
        "the panel is on the newer board"
    );
    let _ = store.queue_runtime_state("broken", board("without the panel"));
    assert!(
        !store.writer.lock().parks.contains_key("broken"),
        "a record of a panel that left the board is dropped, so it cannot add the panel back"
    );
    while store.writer.reserve("broken").map(|direct| direct.forget()).is_err() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[cfg(not(unix))]
fn a_durable_save_is_refused_where_folders_cannot_be_synced() {
    let (_temp, store) = store();
    let generation = store.queue_runtime_state("session", board("queued"));
    assert!(matches!(
        store.wait_runtime_state_durable("session", generation, Duration::from_secs(30)),
        Err(Some(_))
    ));
    assert!(store.flush_runtime_states(Duration::from_secs(30)));
    assert_eq!(on_disk(&store).as_deref(), Some("queued"), "the save itself still runs");
}
