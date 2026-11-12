use super::*;
use crate::HorizonHome;
use crate::session_store::test_support::SAVE_WAIT;

fn store() -> (tempfile::TempDir, SessionStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(
        HorizonHome::from_root(temp.path().join("home")),
        temp.path().join("config.json"),
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
        store.wait_runtime_state_durable("session", generation, SAVE_WAIT),
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
    assert!(store.flush_runtime_states(SAVE_WAIT));
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
    assert!(store.flush_runtime_states(SAVE_WAIT));
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
    assert_eq!(store.wait_runtime_state_durable("session", healthy, SAVE_WAIT), Ok(()));
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
    assert!(store.flush_runtime_states(SAVE_WAIT));
    assert_eq!(on_disk(&store).as_deref(), Some("direct"));
    let read = store.load_runtime("session").unwrap().unwrap();
    assert_eq!(read.active_workspace_local_id.as_deref(), Some("direct"));
}

#[cfg(unix)]
#[test]
fn a_reserved_session_does_not_wake_the_worker_for_its_retry() {
    let now = Instant::now();
    let mut state = State::default();
    let failed = |retry: Duration| Queued {
        board: board("failed"),
        generation: 1,
        retry: Some(now + retry),
    };
    state.queued.insert("reserved".into(), failed(Duration::ZERO));
    state.queued.insert("other".into(), failed(Duration::from_secs(2)));
    assert_eq!(state.next_retry(now), Duration::ZERO);
    state.direct.insert("reserved".into());
    assert_eq!(
        state.next_retry(now),
        Duration::from_secs(2),
        "no wait of zero while it is reserved"
    );
    state.queued.remove("other");
    assert_eq!(state.next_retry(now), IDLE);
}

#[test]
#[cfg(unix)]
fn the_writer_leaves_a_session_alone_while_a_save_outside_the_queue_writes_it() {
    let (_temp, store) = store();
    let direct = store.writer.supersede("session").unwrap();
    assert!(store.writer.supersede("session").is_err(), "one direct save at a time");
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
        store.wait_runtime_state_durable("session", generation, SAVE_WAIT),
        Ok(())
    );
    assert_eq!(on_disk(&store).as_deref(), Some("queued"));
}
