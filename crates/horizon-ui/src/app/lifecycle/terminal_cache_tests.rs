use std::time::{Duration, Instant};

use egui::Shape;
use horizon_core::{PanelKind, PanelOptions, RuntimeState, StartupDecision};

use super::super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::terminal_widget::TerminalGridCache;

fn append_text(shape: &Shape, text: &mut String) {
    match shape {
        Shape::Text(shape) => text.push_str(&shape.galley.job.text),
        Shape::Vec(shapes) => {
            for shape in shapes {
                append_text(shape, text);
            }
        }
        _ => {}
    }
}

#[test]
fn offscreen_output_invalidates_terminal_cache_before_a_quiet_poll() {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("cache-test", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.root_viewport_stabilizer = None;
    let workspace = app.board.workspaces[0].id;
    let panel_id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    r"stty -echo; read -r line; printf 'OFFSCREEN-UPDATE'; read -r line".into(),
                ],
                cwd: Some(temp.path().to_path_buf()),
                transcript_root: Some(temp.path().join("transcripts")),
                position: Some([100_000.0, 100_000.0]),
                size: Some([600.0, 300.0]),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("synthetic terminal");
    app.panel_render_caches
        .terminal_grid_cache
        .insert(panel_id, TerminalGridCache::default());
    app.board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("terminal")
        .write_input(b"update\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.drain_panel_output();
        let updated = app
            .board
            .panel(panel_id)
            .expect("panel")
            .terminal()
            .expect("terminal")
            .last_lines_text(20)
            .contains("OFFSCREEN-UPDATE");
        if updated && !app.panel_render_caches.terminal_grid_cache.contains_key(&panel_id) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "offscreen output must invalidate the retained grid"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.drain_panel_output();
    assert!(!app.panel_render_caches.terminal_grid_cache.contains_key(&panel_id));
    app.board.panel_mut(panel_id).expect("panel").layout.position = [40.0, 60.0];
    app.board.focused = Some(panel_id);
    app.canvas_view = horizon_core::CanvasViewState::default();
    let mut text = String::new();
    for _ in 0..3 {
        for shape in run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None)).shapes {
            append_text(&shape.shape, &mut text);
        }
    }
    assert!(
        text.contains("OFFSCREEN-UPDATE"),
        "returning to the panel must paint its latest output"
    );
}
