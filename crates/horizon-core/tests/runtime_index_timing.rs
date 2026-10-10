//! Measures how long a save and a load of a large synthetic board take. The test
//! uses only the public session store, so the same file measures a build without
//! the runtime index. Run it in release:
//!
//! `cargo test --release -p horizon-core --test runtime_index_timing -- --ignored --nocapture`
use std::time::{Duration, Instant};

use horizon_core::{
    AgentSessionBinding, CanvasViewState, HorizonHome, PanelKind, PanelState, RuntimeState, SessionStore, WindowConfig,
    WorkspaceState,
};

const WORKSPACES: usize = 50;
const PANELS_PER_WORKSPACE: usize = 12;
const ROUNDS: usize = 51;

fn panel(workspace: usize, index: usize) -> PanelState {
    let local_id = format!("panel-{workspace}-{index}");
    let kind = if index.is_multiple_of(3) {
        PanelKind::Claude
    } else {
        PanelKind::Shell
    };
    let session_binding = (kind == PanelKind::Claude).then(|| {
        AgentSessionBinding::new(
            kind,
            format!("synthetic-session-{workspace}-{index}"),
            Some("/synthetic/repository".into()),
            Some("Continue the synthetic task with the synthetic fixture".into()),
            Some(42),
        )
    });
    #[allow(clippy::cast_precision_loss)]
    let offset = (index * 540) as f32;
    PanelState {
        name: format!("Synthetic {local_id}"),
        local_id,
        kind,
        command: Some("synthetic-shell".into()),
        args: vec!["--isolated".into(), "--login".into()],
        cwd: Some(format!("/synthetic/repository-{workspace}")),
        position: Some([offset, 20.0]),
        size: Some([520.0, 500.0]),
        session_binding,
        ..PanelState::default()
    }
}

fn board() -> RuntimeState {
    RuntimeState {
        window: Some(WindowConfig::default()),
        canvas_view: Some(CanvasViewState::new([24.0, -12.0], 1.0)),
        workspaces: (0..WORKSPACES)
            .map(|workspace| {
                #[allow(clippy::cast_precision_loss)]
                let row = (workspace * 600) as f32;
                WorkspaceState {
                    local_id: format!("workspace-{workspace}"),
                    name: format!("Synthetic workspace {workspace}"),
                    cwd: Some(format!("/synthetic/repository-{workspace}")),
                    position: Some([0.0, row]),
                    panels: (0..PANELS_PER_WORKSPACE).map(|index| panel(workspace, index)).collect(),
                    ..WorkspaceState::default()
                }
            })
            .collect(),
        ..RuntimeState::default()
    }
}

/// The shortest and the median time of `ROUNDS` runs after one warm-up run. The
/// shortest time is the least disturbed by other work on a busy machine.
fn measure(mut operation: impl FnMut(usize)) -> (Duration, Duration) {
    operation(usize::MAX);
    let mut samples: Vec<Duration> = (0..ROUNDS)
        .map(|round| {
            let start = Instant::now();
            operation(round);
            start.elapsed()
        })
        .collect();
    samples.sort();
    (samples[0], samples[ROUNDS / 2])
}

#[test]
#[ignore = "a measurement, not a check; run it in release with --ignored --nocapture"]
fn save_and_load_time_of_a_large_board() {
    let root = tempfile::Builder::new()
        .prefix("horizon-runtime-timing-")
        .tempdir()
        .expect("temp root");
    let home = HorizonHome::from_root(root.path().to_path_buf());
    let store = SessionStore::new(home.clone(), home.config_path());
    let mut state = board();
    let session = store
        .create_session_from_runtime(state.clone())
        .expect("create session")
        .session_id;
    let yaml_bytes = std::fs::metadata(home.session_runtime_path(&session))
        .expect("runtime.yaml")
        .len();

    let unchanged = measure(|_| store.save_runtime_state(&session, &state).expect("save"));
    let moved = measure(|round| {
        #[allow(clippy::cast_precision_loss)]
        let x = (round % 7) as f32;
        state.workspaces[round % WORKSPACES].panels[0].position = Some([x, 20.0]);
        store.save_runtime_state(&session, &state).expect("save");
    });
    let load = measure(|_| {
        let loaded = store.resume_session(&session).expect("load");
        assert_eq!(loaded.runtime_state.workspaces.len(), WORKSPACES);
    });

    println!(
        "board: {WORKSPACES} workspaces, {} panels, runtime.yaml {yaml_bytes} bytes; shortest and median of {ROUNDS} rounds",
        WORKSPACES * PANELS_PER_WORKSPACE
    );
    println!("save, unchanged board: {unchanged:?}");
    println!("save, one panel moved: {moved:?}");
    println!("load: {load:?}");
}
