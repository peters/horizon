use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use super::{APPLICATION_ID, MIGRATIONS, SCHEMA_VERSION, read_board};
use crate::panel::PanelResume;
use crate::{
    AgentSessionBinding, CanvasViewState, HorizonHome, PanelKind, PanelState, RuntimeState, SessionStore, WindowConfig,
    WorkspaceState,
};

struct Fixture {
    _root: tempfile::TempDir,
    home: HorizonHome,
    store: SessionStore,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("horizon-runtime-index-")
            .tempdir()
            .expect("temp root");
        let home = HorizonHome::from_root(root.path().to_path_buf());
        let store = SessionStore::new(home.clone(), home.config_path());
        Self {
            _root: root,
            home,
            store,
        }
    }

    /// A second store on the same home, as another Horizon process has.
    fn other_process(&self) -> SessionStore {
        SessionStore::new(self.home.clone(), self.home.config_path())
    }

    fn create(&self, state: RuntimeState) -> String {
        self.store
            .create_session_from_runtime(state)
            .expect("create session")
            .session_id
    }

    fn yaml(&self, session: &str) -> PathBuf {
        self.home.session_runtime_path(session)
    }

    fn index(&self, session: &str) -> PathBuf {
        self.home.session_runtime_index_path(session)
    }

    fn loaded(&self, session: &str) -> RuntimeState {
        self.store
            .resume_session(session)
            .expect("resume session")
            .runtime_state
    }
}

fn panel(local_id: &str, kind: PanelKind) -> PanelState {
    PanelState {
        local_id: local_id.into(),
        name: format!("Synthetic {local_id}"),
        kind,
        command: Some("synthetic-shell".into()),
        args: vec!["--isolated".into()],
        cwd: Some("/synthetic/repository".into()),
        position: Some([10.0, 20.0]),
        size: Some([520.0, 500.0]),
        ..PanelState::default()
    }
}

fn workspace(local_id: &str, panels: Vec<PanelState>) -> WorkspaceState {
    WorkspaceState {
        local_id: local_id.into(),
        name: format!("Synthetic {local_id}"),
        cwd: Some("/synthetic/repository".into()),
        position: Some([0.0, 40.0]),
        panels,
        ..WorkspaceState::default()
    }
}

fn board() -> RuntimeState {
    let mut agent = panel("agent", PanelKind::Claude);
    agent.resume = PanelResume::Session {
        session_id: "saved-session".into(),
    };
    agent.session_binding = Some(AgentSessionBinding::new(
        PanelKind::Claude,
        "saved-session".into(),
        Some("/synthetic/repository".into()),
        Some("Continue the synthetic task".into()),
        Some(42),
    ));
    let mut state = RuntimeState {
        window: Some(WindowConfig::default()),
        canvas_view: Some(CanvasViewState::new([24.0, -12.0], 1.5)),
        active_workspace_local_id: Some("second".into()),
        focused_panel_local_id: Some("agent".into()),
        workspaces: vec![
            workspace("first", vec![panel("shell", PanelKind::Shell), agent]),
            workspace("second", vec![panel("editor", PanelKind::Editor)]),
            workspace("empty", Vec::new()),
        ],
        ..RuntimeState::default()
    };
    add_cloud(&mut state, "second");
    state
}

#[cfg(feature = "cloud-workspaces")]
fn add_cloud(state: &mut RuntimeState, workspace: &str) {
    let mut group = crate::cloud_panel::CloudGroup::new(
        104,
        "Synthetic cloud".into(),
        workspace.into(),
        "/synthetic/repository".into(),
        [24.0, 128.0],
    );
    group.panels = vec!["editor".into()];
    state.cloud_groups.0.push(group);
}

#[cfg(not(feature = "cloud-workspaces"))]
fn add_cloud(state: &mut RuntimeState, workspace: &str) {
    state.cloud_groups.push(serde_json::json!({
        "issue": 104,
        "workspace": workspace,
        "environment": {"id": "issue-104", "image": "synthetic", "connection": "LocalPrototype"},
        "panels": ["editor"],
    }));
}

fn value(state: &RuntimeState) -> Value {
    serde_json::to_value(state).expect("state value")
}

fn indexed(path: &Path) -> RuntimeState {
    read_board(path)
        .expect("readable index")
        .expect("index has a board")
        .state
}

fn query<T: rusqlite::types::FromSql>(path: &Path, sql: &str) -> T {
    Connection::open(path)
        .expect("open index")
        .query_row(sql, [], |row| row.get(0))
        .expect("query index")
}

fn index_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("session dir")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("runtime.sqlite"))
        .collect();
    names.sort();
    names
}

#[test]
fn every_schema_version_has_one_migration_and_a_new_index_is_marked() {
    let fixture = Fixture::new();
    let session = fixture.create(board());

    assert_eq!(MIGRATIONS.len(), usize::try_from(SCHEMA_VERSION).expect("version"));
    assert_eq!(
        query::<i32>(&fixture.index(&session), "PRAGMA application_id"),
        APPLICATION_ID
    );
    assert_eq!(
        query::<i32>(&fixture.index(&session), "PRAGMA user_version"),
        SCHEMA_VERSION
    );
    assert_eq!(
        query::<String>(&fixture.index(&session), "PRAGMA journal_mode").to_lowercase(),
        "wal"
    );
}

#[test]
fn a_saved_board_reads_back_from_the_index_with_its_environments() {
    let fixture = Fixture::new();
    let state = board();
    let session = fixture.create(state.clone());

    assert_eq!(value(&indexed(&fixture.index(&session))), value(&state));
    assert_eq!(value(&fixture.loaded(&session)), value(&state));
    let environments: Option<String> = query(
        &fixture.index(&session),
        "SELECT environments FROM workspaces WHERE local_id = 'second'",
    );
    let environments: Value = serde_json::from_str(&environments.expect("cloud environments")).expect("json");
    assert_eq!(environments[0]["id"], "issue-104");
    let local: Option<String> = query(
        &fixture.index(&session),
        "SELECT environments FROM workspaces WHERE local_id = 'first'",
    );
    assert_eq!(local, None);
    assert_eq!(
        query::<i64>(
            &fixture.index(&session),
            "SELECT count(*) FROM panels WHERE kind = 'claude'"
        ),
        1
    );
}

#[test]
fn a_board_without_runtime_yaml_loads_from_the_index() {
    let fixture = Fixture::new();
    let state = board();
    let session = fixture.create(state.clone());
    std::fs::remove_file(fixture.yaml(&session)).expect("remove runtime.yaml");

    assert_eq!(value(&fixture.loaded(&session)), value(&state));
}

#[test]
fn every_edit_of_the_board_reads_back_from_the_index() {
    let fixture = Fixture::new();
    let mut state = board();
    let session = fixture.create(state.clone());
    let edits: [fn(&mut RuntimeState); 6] = [
        |state| state.workspaces[0].panels[0].position = Some([300.0, 400.0]),
        |state| state.workspaces[1].panels.push(panel("added", PanelKind::Shell)),
        |state| {
            state.workspaces[0].panels.remove(0);
        },
        |state| state.workspaces.swap(0, 2),
        |state| {
            state.workspaces.remove(1);
        },
        |state| state.workspaces[0].name = "Renamed".into(),
    ];

    for edit in edits {
        edit(&mut state);
        fixture.store.save_runtime_state(&session, &state).expect("save edit");

        assert_eq!(value(&indexed(&fixture.index(&session))), value(&state));
    }
    assert_eq!(
        query::<i64>(&fixture.index(&session), "SELECT count(*) FROM workspaces"),
        2
    );
}

#[test]
fn a_session_without_an_index_loads_runtime_yaml_untouched_and_the_next_save_imports_it() {
    let fixture = Fixture::new();
    let state = board();
    let session = fixture.create(state.clone());
    let session_dir = fixture.home.session_dir(&session);
    fixture.store.index.forget(&session);
    for name in index_files(&session_dir) {
        std::fs::remove_file(session_dir.join(name)).expect("remove index file");
    }
    let yaml = std::fs::read(fixture.yaml(&session)).expect("runtime.yaml");

    assert_eq!(value(&fixture.loaded(&session)), value(&state));
    assert!(index_files(&session_dir).is_empty(), "a load must not create an index");
    assert_eq!(std::fs::read(fixture.yaml(&session)).expect("runtime.yaml"), yaml);

    fixture
        .store
        .save_runtime_state(&session, &state)
        .expect("save after the upgrade");

    assert_eq!(value(&indexed(&fixture.index(&session))), value(&state));
    assert_eq!(std::fs::read(fixture.yaml(&session)).expect("runtime.yaml"), yaml);
}

#[test]
fn runtime_yaml_written_by_an_earlier_horizon_wins_over_the_index() {
    let fixture = Fixture::new();
    let session = fixture.create(board());
    let mut earlier = board();
    earlier.workspaces[0].name = "Changed by an earlier Horizon".into();
    std::fs::write(fixture.yaml(&session), earlier.to_yaml().expect("yaml")).expect("write runtime.yaml");

    assert_eq!(value(&fixture.loaded(&session)), value(&earlier));
}

#[test]
fn a_damaged_index_falls_back_to_runtime_yaml_and_a_save_sets_it_aside() {
    const DAMAGE: &[u8] = b"synthetic damage, not a database";
    let fixture = Fixture::new();
    let state = board();
    let session = fixture.create(state.clone());
    fixture.store.index.forget(&session);
    std::fs::write(fixture.index(&session), DAMAGE).expect("damage index");

    assert_eq!(value(&fixture.loaded(&session)), value(&state));

    fixture
        .store
        .save_runtime_state(&session, &state)
        .expect("save over a damaged index");

    assert_eq!(value(&indexed(&fixture.index(&session))), value(&state));
    let session_dir = fixture.home.session_dir(&session);
    let aside: Vec<String> = index_files(&session_dir)
        .into_iter()
        .filter(|name| name.contains(".damaged-") && name.ends_with(|last: char| last.is_ascii_digit()))
        .collect();
    assert_eq!(aside.len(), 1, "{:?}", index_files(&session_dir));
    assert_eq!(
        std::fs::read(session_dir.join(&aside[0])).expect("damaged copy"),
        DAMAGE
    );
}

#[test]
fn an_index_from_a_newer_horizon_is_refused_and_never_changed() {
    let fixture = Fixture::new();
    let state = board();
    let session = fixture.create(state.clone());
    fixture.store.index.forget(&session);
    Connection::open(fixture.index(&session))
        .expect("open index")
        .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("mark newer");
    let newer = std::fs::read(fixture.index(&session)).expect("newer index");

    let error = fixture.store.resume_session(&session).expect_err("newer index");
    assert!(error.to_string().contains("newer"), "{error}");
    fixture
        .store
        .save_runtime_state(&session, &state)
        .expect("runtime.yaml is still saved");

    assert_eq!(std::fs::read(fixture.index(&session)).expect("newer index"), newer);
}

#[test]
fn a_value_that_json_cannot_carry_is_kept_as_yaml() {
    let fixture = Fixture::new();
    let mut state = board();
    state.workspaces[0].panels[0].position = Some([f32::NAN, 5.0]);
    let session = fixture.create(state);

    let format: String = query(
        &fixture.index(&session),
        "SELECT format FROM panels WHERE local_id = 'shell'",
    );
    assert_eq!(format, "yaml");
    let position = indexed(&fixture.index(&session)).workspaces[0].panels[0]
        .position
        .expect("position");
    assert!(position[0].is_nan());
    assert!((position[1] - 5.0).abs() < f32::EPSILON);
}

#[test]
fn two_processes_that_save_the_same_session_keep_the_last_board() {
    let fixture = Fixture::new();
    let mut state = board();
    let session = fixture.create(state.clone());
    let other = fixture.other_process();

    for round in 0..4_u8 {
        state.workspaces[0].name = format!("Round {round}");
        let store = if round % 2 == 0 { &other } else { &fixture.store };
        store.save_runtime_state(&session, &state).expect("save");
    }

    assert_eq!(value(&indexed(&fixture.index(&session))), value(&state));
    assert_eq!(value(&fixture.loaded(&session)), value(&state));
}

#[test]
fn deleting_a_session_closes_and_removes_its_index() {
    let fixture = Fixture::new();
    let session = fixture.create(board());
    fixture.create(board());

    fixture.store.delete_session(&session).expect("delete session");

    assert!(!fixture.home.session_dir(&session).exists());
}

#[cfg(feature = "cloud-workspaces")]
#[test]
fn cloud_panels_keep_their_park_state_and_last_status_line() {
    use std::time::Duration;

    use crate::cloud_runtime::session_status::{SessionActivity, SessionStatus};
    use crate::{CloudPanelStatus, ParkedPanel};

    let fixture = Fixture::new();
    let mut state = board();
    let session = fixture.create(state.clone());
    let read = SessionStatus {
        id: "synthetic-session".into(),
        activity: SessionActivity::Exited(Some(2)),
        quiet_for: Some(Duration::from_secs(30)),
        lines: vec!["first line".into(), "last line".into()],
    };
    let record = |panels: &[ParkedPanel<'_>]| {
        fixture
            .store
            .record_cloud_panels(&session, panels)
            .expect("record cloud panels");
    };

    record(&[
        ParkedPanel {
            local_id: "editor",
            parked: true,
            status: Some(&read),
        },
        ParkedPanel {
            local_id: "shell",
            parked: true,
            status: None,
        },
    ]);
    record(&[ParkedPanel {
        local_id: "editor",
        parked: false,
        status: None,
    }]);

    let statuses = fixture.store.cloud_panel_statuses(&session).expect("statuses");
    assert_eq!(statuses.len(), 2);
    let CloudPanelStatus {
        panel_local_id,
        parked,
        activity,
        quiet_for,
        last_line,
        read_at,
    } = &statuses[0];
    assert_eq!(panel_local_id, "editor");
    assert!(!parked);
    assert_eq!(*activity, Some(SessionActivity::Exited(Some(2))));
    assert_eq!(*quiet_for, Some(Duration::from_secs(30)));
    assert_eq!(last_line.as_deref(), Some("last line"));
    assert!(read_at.is_some());
    assert!(statuses[1].parked);
    assert_eq!(statuses[1].activity, None);

    state.workspaces[0].panels.remove(0);
    fixture.store.save_runtime_state(&session, &state).expect("save");

    let statuses = fixture.store.cloud_panel_statuses(&session).expect("statuses");
    assert_eq!(statuses.len(), 1, "a panel that left the board keeps no status");
    assert_eq!(statuses[0].panel_local_id, "editor");
}
